> CUE 架构规格分卷。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 3. 最重要的架构原则

整个软件遵循：

> **Core 管如何运行功能；Module 管功能本身如何实现。**

Core 不理解：

- 什么是 `.lnk`
- 什么是 Everything
- 什么是拼音
- 什么是 fuzzy score
- 什么是 App 去重
- 什么是文件路径 ranking

Module 不控制：

- Launcher Window
- 输入框
- 当前 Session
- Core 状态
- 全局快捷键
- Settings UI 的视觉样式
- Result Row 的 GPUI 实现

---

# 4. 顶层架构

```text
                         Windows
                            │
                      Global Hotkey
                            │
                            ▼
┌────────────────────────────────────────────────────┐
│                  Windows Host                      │
│                                                    │
│  RegisterHotKey / foreground / monitor / startup  │
└───────────────────────┬────────────────────────────┘
                        │
                        ▼
┌────────────────────────────────────────────────────┐
│                    Core Runtime                    │
│                                                    │
│  Session                                            │
│  Input Routing                                      │
│  Module Registry                                    │
│  Query Lifecycle                                    │
│  Result State                                       │
│  Settings                                           │
│  Usage                                              │
└──────────────┬──────────────────────┬──────────────┘
               │                      │
               │ Module Protocol      │ UI State
               │                      │
       ┌───────▼─────────┐      ┌────▼────────────┐
       │    Modules      │      │    GPUI UI      │
       │                 │      │                 │
       │ AppModule       │      │ Search Box      │
       │ FileModule      │      │ Results         │
       │ PageModule(?)   │      │ Actions         │
       └─────────────────┘      │ Settings        │
                                └─────────────────┘
```

---

# 5. Core 的职责

Core 是一个薄的 Host Runtime。

Core 负责以下七类事情。

---

## 5.1 Launcher Session

Core 管理一次 Launcher 从打开到关闭的完整生命周期。

包括：

```text
open
input
module switch
query
selection
activation
close
```

Core 保存：

```rust
struct SessionState {
    session_id: SessionId,

    raw_input: String,

    active_module: ModuleId,

    results: Vec<ResultEntry>,

    selected_index: Option<usize>,

    query_generation: u64,
}
```

Session 关闭后：

- 输入清空
- Result 清空
- Selection 清空
- 在途 query 不再被接受（§96 ticket 校验；Core 不做物理取消，§98）

---

## 5.2 Input Routing

Core 根据输入决定调用哪个 Module。

例如：

```text
zed
```

解析为：

```text
module = app
input  = "zed"
```

而：

```text
/paper
```

解析为：

```text
module = file
input  = "paper"
```

重要约束：

> Core 只解析 Module Trigger。

如果：

```text
/report ext:pdf
```

Core 只做：

```text
/
→ FileModule

remaining input:
report ext:pdf
```

Core 不知道：

```text
ext:pdf
```

意味着什么。

词边界规则（v0.2 增补，随 BookmarkModule 引入）：

以字母/数字结尾的触发词（`b`）要求**词边界**——trigger 之后必须是
空白或输入结束才命中，边界空白不进查询；以标点结尾的触发词（`/`）
逐字前缀匹配，查询原样传递。

```text
b github   → module = bookmark, input = "github"
b          → module = bookmark, input = ""
baidu      → module = app,      input = "baidu"（不被 b 吞掉）
/paper     → module = file,     input = "paper"
```

---

## 5.3 Module Registry

Core 维护所有内置 Module。

V1 Registry 直接存：

```rust
HashMap<ModuleId, Box<dyn LauncherModule>>
```

stable Rust 无法在 `dyn Module` 上运行时查询"是否同时是 LauncherModule"（无 trait upcasting），而 V1 所有 Module 都是 LauncherModule —— 不解决不存在的问题。未来首次出现非 launcher 的 Module 时再设计第二种 hosting（§72）。

当前注册方式静态完成：

```rust
registry.register(AppModule::new(...));
```

不支持：

```text
扫描 plugins/
LoadLibrary
动态下载安装
```

---

## 5.4 Query Lifecycle

Core 统一解决：

- 快速输入
- stale result
- Module 切换时的旧结果

例如用户：

```text
y
yj
yjw
yjwj
```

对应：

```text
Generation 41
Generation 42
Generation 43
Generation 44
```

即使：

```text
Generation 42
```

比 44 更晚返回，也不能更新 UI。

v0.2 修订：接受条件不只是 generation。每个在途 query 由 Core 绑定一个 QueryTicket（§96）：

```text
session_id / module_id / module_epoch / generation
```

四项全部匹配当前状态才接受：

- `session_id`：跨 Session 的旧结果必死（不存在"新 Session generation 恰好相同而误收"的窗口）
- `module_epoch`：Module unload / reload 后，旧实例的在途结果必死
- `generation`：Session 内被更新输入取代的结果必死

generation 由 Core 持有，不由 Module 回显（§95）。Core 不物理取消在途 query（§98）。

---

## 5.5 Result State

Module 提供 Result。

Core 负责：

- 保存当前 Result 集合
- Selection
- ↑ / ↓
- Enter
- Result refresh
- Result rendering dispatch

Core 不理解 Result 的业务实体。

---

## 5.6 Settings Host

Core：

- 收集 Module Settings Schema
- 保存 Settings
- 提供统一 Settings UI
- 将设置变更通知 Module

Module 不自己建立完整 Settings Window。

---

## 5.7 Usage

Core 统一记录：

```text
什么时候
哪个 Module
哪个 Item
执行了哪个 Action
```

Module 可以读取 Usage，用于自己的 ranking。

---

# 6. Module 的定义

Module 是：

> **一个独立功能单元。**

当前：

```text
AppModule
FileModule
```

未来：

```text
PageModule
```

Module 本身不强制一定是搜索功能。

但是当前能进入 Launcher 主输入框的 Module，需要额外实现：

```text
LauncherModule
```

---

# 7. 基础 Module Trait

建议：

```rust
pub trait Module {
    fn descriptor(&self) -> &ModuleDescriptor;

    fn load(
        &mut self,
        ctx: ModuleContext,
    ) -> Result<(), ModuleError>;

    fn unload(&mut self);

    fn settings_schema(&self) -> SettingsSchema;

    fn try_apply_settings(
        &mut self,
        changes: SettingsChangeSet,
    ) -> Result<(), ModuleError>;
}
```

命名即语义：先 try-apply，成功才由 Core commit；失败不 commit（§42）。

---

# 8. ModuleDescriptor

```rust
pub struct ModuleDescriptor {
    pub id: ModuleId,
    pub name: &'static str,
    pub version: &'static str,
}
```

例如：

```rust
ModuleDescriptor {
    id: ModuleId::from_static("app"),
    name: "Applications",
    version: "0.1.0",
}
```

Module ID 必须：

- 稳定
- 唯一
- 不依赖显示名称

例如：

```text
app
file
page
```

而不是：

```text
Applications
文件搜索
```

---

# 9. LauncherModule Trait

能参与 Launcher 输入交互的 Module：

```rust
pub trait LauncherModule: Module {
    fn launcher_descriptor(
        &self,
    ) -> LauncherDescriptor;

    fn query(
        &mut self,
        ctx: QueryContext,
    ) -> QueryFuture;

    fn present(
        &self,
        item: &ModuleItem,
    ) -> ResultPresentation;

    fn actions(
        &self,
        item: &ModuleItem,
    ) -> Vec<ActionDescriptor>;

    fn activate(
        &mut self,
        item: &ModuleItem,
        action: ActionId,
    ) -> ActivationFuture;
}
```

它必须回答四个问题：

```text
输入给我之后，我返回什么？

这些结果应该展示什么信息？

这个结果有哪些行为？

执行行为后发生什么？
```

---

# 10. LauncherDescriptor

```rust
pub struct LauncherDescriptor {
    pub trigger: Option<String>,
    pub is_default: bool,
}
```

例如：

### App

```rust
LauncherDescriptor {
    trigger: None,
    is_default: true,
}
```

### File

```rust
LauncherDescriptor {
    trigger: Some("/".into()),
    is_default: false,
}
```

### Bookmark（V1.x,§117）

```rust
LauncherDescriptor {
    trigger: Some("b".into()),
    is_default: false,
}
```

约束：

- 一个 Launcher 只能有一个 default Module
- trigger 不得冲突
- trigger 属于 Core routing
- Module 内部 query syntax 不属于 Core

---

# 11. ModuleItem

这里有一个重要原则：

> Core 不应该理解 App、File 等真实业务对象。

Module 内部可以有：

```rust
struct AppEntry {
    ...
}
```

```rust
struct FileEntry {
    ...
}
```

但暴露给 Core 的是一个 owned opaque handle。

v0.2 修订：

```rust
pub struct ModuleItem {
    id: ItemId,
    payload: Arc<dyn Any + Send + Sync>,
}

impl ModuleItem {
    pub fn new<T>(id: ItemId, payload: T) -> Self
    where
        T: Any + Send + Sync + 'static;

    pub fn id(&self) -> ItemId;

    pub fn downcast_ref<T: Any>(&self) -> Option<&T>;
}
```

ModuleItem 是 `Clone` 的（`Arc` 克隆）。

规则：

- Core 只保存、传递 ModuleItem，**从不调用 `downcast_ref`**——只有创建它的 Module 知道 `T` 是什么。payload 对 Core 完全 opaque，§12 的禁令不破
- item 的生命周期由 Rust ownership 表达：Core 还显示这个结果，`Arc<AppEntry>` 就活着；结果被淘汰，内存随之释放
- 因此**不需要**"Module 维护 `HashMap<ItemId, AppEntry>` 供 Core 回调时查表"：那会引出谁插入（后台 query Future 不持有 `&mut self`）、何时清理、UI 线程 `present()` 与后台 writer 锁竞争三个未定义问题
- Module 当然仍维护自己的搜索索引，但 item 数据随 ModuleItem 走，不依赖回查
- `ItemId` 仍保留：用于 `PresentationInvalidated` 事件的寻址（§109），以及 Core 在 ResultState 内的行标识

Core 持有：

```text
ModuleId + ModuleItem（opaque）
```

而不是：

```text
.exe
PathBuf
Url
```

---

# 12. 为什么不把业务信息塞进 Core Result

禁止这种设计：

```rust
struct Result {
    exe: Option<PathBuf>,
    file_path: Option<PathBuf>,
    url: Option<String>,
    process_id: Option<u32>,
}
```

这会导致：

```text
Core
慢慢知道所有 Module 的业务模型。
```

最终产生：

```rust
if result.exe.is_some() { ... }

if result.file_path.is_some() { ... }
```

这是明确禁止的。

§11 的 payload 不改变本节结论：`Arc<dyn Any>` 对 Core 没有任何可读字段，"opaque" 不等于"opaque integer"，而等于"Core 类型上无法触碰业务数据"。

---

