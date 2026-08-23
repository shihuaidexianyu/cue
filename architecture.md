# CUE — Product & Architecture Specification v0.2

## 0. 文档状态

**目标平台：** Windows  
**技术栈：** Rust + GPUI + Windows API  
**当前阶段：** V0 / V1 架构定义  
**核心原则：** 克制、低延迟、模块化、不过度抽象

本 Spec 描述第一阶段 Launcher 的：

- 产品边界
- Core 职责
- Module 职责
- Core ↔ Module 协议
- 输入与路由模型
- 结果展示模型
- 生命周期
- Settings
- 数据存储
- Usage
- App Module
- File Module
- Windows Host
- GPUI UI
- 异步任务模型
- 错误处理
- 推荐项目结构
- V1 开发范围
- 输入法
- Result Row 布局规则
- Module 自发事件
- 跨平台
- Launcher 控制流（CoreEffect）
- 单实例
- 性能测量契约
- UX 不变量
- 托盘图标与退出路径

明确不讨论：

- 第三方插件
- 动态模块加载
- DLL ABI
- WASM
- Module sandbox
- 插件市场
- 第三方权限系统

所有 Module 在当前版本中均为**可信内置 Rust Module，并静态编译进入 Launcher**。

---

# 1. 产品定义

Launcher 是一个：

> **通过统一输入界面，快速进入不同功能模态，并执行用户高频操作的轻量 Windows 工具。**

Launcher 本身不追求成为万能命令中心。

第一阶段的核心任务只有：

1. 快速启动应用
2. 快速打开文件 / 文件夹

未来可以增加：

3. Page / Browser 内容
4. 其他新的内置 Module

但新增功能必须满足明确的高频需求，不因“架构支持”而自动加入产品。

---

# 2. 产品核心交互

Launcher 默认通过：

```text
Alt + Space
```

唤醒。

默认进入：

```text
App Module
```

例如：

```text
Alt + Space
zed
Enter
```

启动：

```text
Zed
```

文件使用显式模态前缀：

```text
Alt + Space
/paper
Enter
```

打开：

```text
paper.pdf
```

当前建议：

```text
无前缀    App Module
/         File Module
```

未来：

```text
@         Page Module
```

但 V1 不要求实现 Page。

---

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

# 13. Result Presentation

Module 决定：

> **展示什么。**

Core / GPUI 决定：

> **怎么画。**

统一协议：

```rust
pub struct ResultPresentation {
    pub title: Arc<str>,

    pub subtitle: Option<Arc<str>>,

    pub icon: Option<ResultIcon>,

    pub badges: Vec<ResultBadge>,

    pub accessory: Option<ResultAccessory>,
}
```

v0.2 修订：`SharedString` 是 GPUI re-export 类型，protocol 使用它同样违反 §71（当初换掉 `ImageHandle` 时漏了它）。protocol 统一用 `Arc<str>`（clone 廉价），到 GPUI 类型的转换留在 ui crate。V1 结果数量很小，不为字符串做更多提前优化。

---

# 14. ResultIcon

v0.1 修订：`ImageHandle` 是 GPUI 类型，会让 protocol 依赖 GPUI，违反 §71。改为协议自有位图：

```rust
pub enum ResultIcon {
    Raster(IconImage),
    SystemIcon(SystemIconId),
}

pub struct IconImage {
    pub rgba: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}
```

约定：

```text
单尺寸 96px，UI 永远降采样到行内槽位
RGBA8、row-major、sRGB、straight（非预乘）alpha
rgba.len() == width * height * 4
若 GPUI 纹理需要预乘 alpha 或其他通道序，由 ui 在上传时转换一次
UI 按 Arc 指针缓存 GPU 纹理；
Module 对同一张缓存图标必须复用同一个 Arc<IconImage> 实例，
否则指针缓存失效、同图标重复上传
SystemIcon 是没有专属图标时的通用字形逃生口
```

不要允许 Module：

```text
直接返回 GPUI View
直接写 Render Tree
返回 GPUI 类型
```

---

# 15. Badge

例如：

```text
PDF
Folder
Bookmark
Running
```

定义：

```rust
pub struct ResultBadge {
    pub text: Arc<str>,
}
```

视觉样式由 Core/UI 决定。

---

# 16. Accessory

右侧辅助信息：

```rust
pub enum ResultAccessory {
    Text(Arc<str>),
    Shortcut(Arc<str>),
}
```

例如：

```text
2.4 MB
PDF
↵
Ctrl+Enter
```

---

# 17. Result Row 示例

### App

Module 返回：

```text
title:
Visual Studio Code

subtitle:
Microsoft

icon:
VS Code icon
```

Core 画：

```text
┌─────────────────────────────────────────┐
│ [icon]  Visual Studio Code              │
│         Microsoft                    ↵  │
└─────────────────────────────────────────┘
```

---

### File

Module 返回：

```text
title:
paper.pdf

subtitle:
D:\Research\Papers

badge:
PDF

accessory:
2.4 MB
```

UI：

```text
┌─────────────────────────────────────────┐
│ [PDF]   paper.pdf              2.4 MB   │
│         D:\Research\Papers       PDF    │
└─────────────────────────────────────────┘
```

---

# 18. Action Model

Module 决定某个 Result 能做什么。

统一：

```rust
pub struct ActionDescriptor {
    pub id: ActionId,

    pub label: Arc<str>,

    pub shortcut: Option<Shortcut>,
}
```

例如 App：

```text
Launch
Run as administrator
Open location
```

File：

```text
Open
Open containing folder
Copy path
```

---

# 19. Primary Action

每个结果至少有一个 Primary Action。

通常：

```text
Enter
```

执行 Primary Action。

例如：

```text
App
→ Launch

File
→ Open
```

---

# 20. Module Activation

Core：

```text
selected Result
      ↓
resolve Module
      ↓
module.activate(item, action_id)
```

Module 执行真实业务。

例如：

```text
AppModule
→ ShellExecute / launch
```

或者：

```text
FileModule
→ open file
```

---

# 21. ModuleOutcome

Module 不直接控制 Core：

```text
module.close_launcher()
```

は禁止。

Module 返回：

```rust
pub struct ModuleOutcome {
    pub status: OutcomeStatus,

    pub session: SessionDisposition,

    pub usage: Option<UsageRecordRequest>,
}
```

---

# 22. OutcomeStatus

```rust
pub enum OutcomeStatus {
    Success,
    Failed(ModuleError),
}
```

---

# 23. SessionDisposition

```rust
pub enum SessionDisposition {
    Close,
    KeepOpen,
}
```

例如 App 启动：

```text
Success
Close
```

Core 收到后：

```text
记录 usage
关闭 Launcher
```

Module 无需操作 GPUI。

---

# 24. Core ↔ Module 完整交互

例如：

```text
Alt+Space
```

Core：

```text
open session
```

用户：

```text
yjwj
```

Core：

```text
无 trigger
→ app
```

调用：

```rust
AppModule.query("yjwj")
```

AppModule 内部：

```text
search
rank
Top results
```

返回：

```text
ItemId 32
ItemId 81
...
```

Core 对每一个调用：

```rust
module.present(item)
```

得到：

```text
永劫无间
...
```

GPUI 展示。

用户 Enter。

Core：

```rust
module.activate(
    &item,               // ResultState 中保存的 ModuleItem
    ActionId::PRIMARY
)
```

Module：

```text
launch
```

返回：

```text
Success
Close
RecordUsage
```

Core：

```text
usage store
↓
close session
```

---

# 25. 模糊搜索的位置

这是一个明确决定：

> **Core 不规定 Module 如何搜索。**

因此：

```text
AppModule
```

可以使用：

- nucleo
- pinyin
- initials
- alias
- usage ranking

FileModule 可以：

- Everything
- Everything native ranking

未来 PageModule 可以：

- SQLite FTS
- browser history
- fuzzy
- custom ranking

它们完全可以不同。

---

# 26. 不建立 Universal Search Framework

V1 禁止提前设计：

```text
SearchDocument
UniversalRanker
CandidateProvider
UniversalIndex
SearchStrategy
RankingStrategy
```

除非真实实现中已经证明这些抽象有明确价值。

---

# 27. AppModule 搜索模型

AppModule 初始化时发现应用，并建立自己的搜索结构。

例如：

```text
永劫无间
```

可以预处理：

```text
永劫无间
yongjiewujian
yjwj
naraka
```

这些都属于：

```text
AppModule internal state
```

Core 不知道。

用户：

```text
yjwj
```

AppModule：

```text
fuzzy / exact
↓
永劫无间
```

---

# 28. AppModule Ranking

建议初始版本：

\[
Score =
StringMatch
+
UsageBonus
+
RecencyBonus
+
AliasBonus
\]

但具体公式属于 AppModule。

Core 不参与。

---

# 29. App Discovery

V1 推荐：

```text
User Start Menu
Common Start Menu
UWP / MSIX
```

后续：

```text
App Paths
PATH
custom sources
```

v0.2 确认 UWP/MSIX 实现路线（具体 API 路径）：

```text
PackageManager
→ FindPackagesForUserWithPackageTypes(...)
→ Package.GetAppListEntriesAsync()
→ AppListEntry
    ├─ AppUserModelId
    └─ DisplayInfo（名称、图标资源）

启动：
IApplicationActivationManager::ActivateApplication(AUMID)
```

注意：**Package ≠ App**。一个 package 可含 0..n 个 application，枚举单位是 AppListEntry，不是 Package，也不是自己解析每个 manifest。

不采用：

```text
shell:AppsFolder 枚举
```

理由：AppsFolder 枚举省实现，但返回系统组件等脏数据，过滤规则脆弱；AppListEntry 语义精确。

不要一开始扫描系统所有 `.exe`。

---

# 30. App 去重

原则：

> 不判断两个入口是不是“同一个软件”。

只判断：

> 是否具有相同 launch semantics。

### Packaged Apps

优先：

```text
AUMID
```

### Win32

优先：

```text
canonical executable
+
normalized arguments
```

宁可重复，也不要 aggressive dedup。

---

# 31. FileModule

> **v0.1 修订：FileModule 整体延后出 V1。**
> ~~原因：对 Everything 第三方依赖的策略未最终确认。~~
> **v0.2 定案（已落地，实现记录见 §118）：依赖本机已安装并运行的
> Everything 1.4——Flow Launcher 同款策略。不自建索引(§76 不变)、
> 不随包分发 Everything、不链 Everything.dll;直连 WM_COPYDATA IPC。
> Everything 未运行 → 模块报 Unavailable,行内错误文案,优雅降级。**

FileModule 内部负责：

```text
query Everything
convert results
ranking
presentation
open
secondary actions
```

Core 不知道 Everything 的存在。

---

# 32. FileModule Result

业务对象可能：

```rust
struct FileEntry {
    path: PathBuf,
    is_dir: bool,
    size: Option<u64>,
    modified_at: Option<SystemTime>,
}
```

但只保存在 Module 内部。

Core 只持有：

```text
ItemId
```

---

# 33. 文件和文件夹

FileModule 同一个模态返回：

```text
files
folders
```

不单独设计：

```text
FolderModule
```

除非后续真实需求证明需要。

---

# 34. PageModule

不属于 V1 必须范围。

未来可以包含：

```text
browser history
bookmarks
tabs
known pages
```

但对 Core 来说仍然只是一个 LauncherModule。

---

# 35. Settings Architecture

Settings 分两类：

```text
Core Settings
Module Settings
```

---

# 36. Core Settings

例如：

```text
hotkey
start_on_boot
appearance
window_position
hide_on_focus_loss
```

---

# 37. Module Settings

例如 App：

```text
include_packaged_apps
aliases
```

File：

```text
Everything connection
result limit
```

---

# 38. Settings Schema

Module 声明：

```rust
pub struct SettingSpec {
    pub key: SettingKey,
    pub label: Arc<str>,
    pub description: Option<Arc<str>>,
    pub kind: SettingKind,
    pub default: SettingValue,
    pub apply_policy: ApplyPolicy,
}
```

`apply_policy` 是每个设置的挂载点（§42），不可省略。

---

# 39. SettingKind

V1：

```rust
pub enum SettingKind {
    Bool,
    Integer,
    String,
    Enum,
    Path,
    Hotkey,
}
```

不要一开始建立复杂表单 framework。

---

# 40. Settings Namespace

Core：

```text
core.*
```

Module：

```text
module.<module-id>.*
```

例如：

```text
core.hotkey
core.start_on_boot

module.app.include_packaged_apps

module.file.result_limit
```

---

# 41. Settings UI

Module：

```text
提供 schema
```

Core：

```text
统一渲染 GPUI settings
```

禁止：

```text
AppModule.render_settings_gpui(...)
```

除非未来确实出现无法表达的特殊设置。

v0.2 补充（V1 落地形态）：

```text
入口：托盘右键菜单"设置"（§116）。设置不是 module session——
打开时搜索会话静默退场（其未完成的 query 由 §96 ticket 自然失效），
关闭（Esc / 热键 / 失焦）后回到隐藏态，不恢复搜索输入。

视图：Core 出模型（行 = label + description + 当前值 + kind），
cue-ui 只渲染。键盘语义：↑↓ 选择，Enter/Space 修改
（Bool 直接切换；Hotkey 进入捕获态，下一次组合键为候选，
Esc 取消捕获），Esc 返回。try-apply 失败：错误显示在视图内，
旧值保留（§42）。

V1 的编辑 UI 只覆盖 Bool 与 Hotkey 两种 kind；Integer / String /
Enum / Path 出现时按 §39 再加，不提前建表单 framework。
```

---

# 42. Apply Policy

每个 `SettingSpec` 声明自己的 `apply_policy`（§38）：

```rust
pub enum ApplyPolicy {
    Immediate,
    ReloadModule,
    RestartApplication,
}
```

v0.2 新增：Settings 变更的 **commit 顺序是事务性的**，所有设置走同一条路径，实现者不得自行选择别的顺序：

```text
Immediate：
  candidate value
  → 类型 / 取值校验
  → try-apply（Module.try_apply_settings，§7；core.* 由对应所有者执行，如 §53 的 apply_hotkey）
  → 成功：commit 内存值 → 持久化
  → 失败：不 commit，UI 恢复旧值并显示错误

ReloadModule：
  校验 → commit → 持久化 → reload 目标 Module（§65）

RestartApplication：
  校验 → commit → 持久化 → 标记 restart_required，UI 提示
```

try-apply 失败时 Core 保留的永远是旧值（§53 热键即一例）。

V1 可以只实现 `Immediate` 与 `RestartApplication`，但 commit 顺序从第一天起就按上文执行。

---

# 43. Module Storage

当前 Module 都是可信内置代码，因此不实现安全 sandbox。

但必须遵守统一目录规范。

建议：

```text
%LOCALAPPDATA%\CUE\
│
├── core\
│
├── modules\
│   ├── app\
│   │   ├── data\
│   │   ├── state\
│   │   └── cache\
│   │
│   └── file\
│       ├── data\
│       ├── state\
│       └── cache\
│
└── logs\
```

---

# 44. Storage Scope

```rust
pub enum StorageScope {
    Data,
    State,
    Cache,
}
```

Temp 可以走系统：

```text
%TEMP%\CUE\
```

---

# 45. Data

真正需要持久保存的 Module 数据：

```text
aliases
manual app entries
internal database
```

---

# 46. State

可恢复但不是用户核心数据：

```text
last index time
schema state
internal cursor
```

---

# 47. Cache

完全可以删除并重建：

```text
icons
favicons
metadata
```

---

# 48. Settings 不放 Module Storage

Settings 必须统一属于：

```text
Settings Host
```

禁止 Module：

```text
modules/app/config.json
```

另起自己的设置体系。

---

# 49. ModuleContext

Core 加载 Module 时提供：

```rust
pub struct ModuleContext {
    pub module_id: ModuleId,

    pub storage: ModuleStorage,

    pub settings: ModuleSettings,

    pub usage: UsageReader,

    pub logger: ModuleLogger,

    pub events: ModuleEventSink,
}
```

`events` 用于 Module 自发事件，见 §109。除此之外 V1 不需要更多 capability。

`events` sink 在 `load` 时绑定 `(ModuleId, ModuleEpoch)`。`ModuleEpoch` 由 Core 在每次 load 时分配、单调递增；unload / reload 之后，旧 sink 发出的事件一律丢弃（§109）。同一组 `(session_id, module_id, module_epoch, generation)` 概念同时用于 query ticket（§96）。

---

# 50. Usage Store

v0.2 修订：V1 不存完整 event log，存聚合统计。

```rust
pub struct UsageStat {
    pub count: u64,
    pub last_used: SystemTime,
}
```

存储 key：

```text
(ModuleId, ItemKey, ActionId)
```

理由：排序只需要 frequency + recency（§52），event log 无限增长且当前没有任何消费者（无时间线 UI、无 analytics）。这是刻意取舍：未来若确需 decay frecency 或历史，做数据迁移，而不是现在为假想需求建 event 表（§72）。

---

# 51. Stable Item Key

Module 激活成功后可以返回：

```text
item_key
```

item_key 必须是**稳定标识**，与 §30 的 launch semantics 对齐，不得用显示名称或 slug：

```text
Packaged App：
AUMID（持久化，与 package 版本、架构无关）

Win32 App：
canonical exe + normalized args

File：
<canonical path>
```

具体格式由 Module 决定。

Core 只当 opaque string。不需要 `app:` 之类前缀——Usage 记录的 key 已含 `module_id`（§50）。

---

# 52. Usage 数据如何使用

Core：

```text
只保存
```

Module：

```text
决定如何解释
```

例如 AppModule：

```text
launch count
last used
```

用于 ranking。

FileModule 可以完全忽略 Usage。

---

# 53. Global Hotkey

全局热键分两层：

```text
Core（OS-neutral）：
core.hotkey 设置（§36、§40）
SettingKind::Hotkey（§39）
toggle 语义

Windows Host（§111 隔离区）：
RegisterHotKey / UnregisterHotKey
键位描述的 Win32 翻译（MOD_*、VK_*）
```

默认：

```text
Alt + Space
```

Hotkey 触发 toggle：

```text
窗口隐藏           → Core.open_session()
窗口可见且聚焦     → 关闭（等价于 Esc）
窗口可见但未聚焦   → 聚焦（§54 允许失焦不隐藏的情况）
```

toggle 行为固定，不设为配置项。

热键组合的存储值必须是 OS-neutral 描述：

```rust
pub struct Hotkey {
    pub modifiers: Modifiers,   // Alt / Ctrl / Shift / Super
    pub key: Key,               // 协议自定义键位枚举
}
```

禁止把 Win32 常量（`MOD_*`、`VK_*`）作为设置值存进 Core（§111）。翻译到平台 API 属于 Host。Key 枚举 V1 只需覆盖常见可注册键位。

变更即时生效（ApplyPolicy::Immediate，commit 顺序见 §42）。Core 通过注入的**同步**回调调用 Host——这是一个函数，不是 HostPlatform trait：

```rust
fn apply_hotkey(hotkey: &Hotkey) -> Result<(), HotkeyError>
```

Host 执行：

```text
先 RegisterHotKey 新的
成功 → UnregisterHotKey 旧的 → Ok，Core commit 新值
失败 → 旧注册不动 → Err，Core 保留旧值，Settings UI 显示错误
```

先注册新的再注销旧的：失败时旧热键仍然有效。设置事务必须同步，因此热键不走 §112 的 CoreEffect 异步路径。

Host 实现注记：

```text
注册必须带 MOD_NOREPEAT：
避免长按自动重复触发 toggle，造成窗口 show/hide 闪烁

用两个 HOTKEY_ID（ACTIVE / STAGING）做事务式替换：
新组合先在 STAGING id 注册成功，
再注销 ACTIVE 的旧注册并交换两个 id 的角色
```

V1 只支持单一全局热键组合（修饰键 + 普通键）。
不支持多热键、序列热键、per-module 热键。

---

# 54. Window 行为

Launcher Window：

- 常驻但默认隐藏
- 不频繁 destroy / recreate
- Hotkey → show
- Escape → hide
- Activation success → hide
- Lost focus 可配置是否 hide
- 显示在当前用户活跃 monitor

---

# 55. Window 性能目标

从：

```text
Hotkey event
```

到：

```text
可输入
```

目标：

```text
< 50 ms ideal
< 100 ms acceptable
```

避免唤醒时：

- 扫描 App
- 查询 registry
- 读取浏览器 DB
- 初始化 Everything
- 做 icon extraction

这些都应提前完成或 lazy cache。

---

# 56. AppModule 初始化

启动应用时：

```text
Launcher boot
↓
AppModule load
↓
discover
↓
normalize
↓
deduplicate
↓
build search state
↓
ready
```

输入 hot path 只做：

```text
query
match
rank
Top N
```

v0.2 补充两条边界：

```text
刷新策略：V1 的 App catalog 只在进程启动时构建。
运行期间安装 / 卸载的应用，重启 CUE 后才反映。
不做 FileSystemWatcher / PackageCatalog 事件 / registry 监听。
```

冷启动回退（v0.2 原案）曾约定：若实测 load P95 无法满足 §77 的
< 500 ms，退回"加载缓存 catalog → 后台刷新"。

**Spike 实测结论（2026-08，debug build）：**

```text
首次发现（WinRT 冷初始化）≈ 6.7 s；热路径 ≈ 0.55 s
（Start Menu 0.12 s + Packaged 0.43 s，148 entries）。
同步 load 必然阻塞热键注册，违反 §77——回退条款触发。
```

落地决策（取代原回退案）：

```text
catalog 构建移出 load()，由 AppModule 自有线程一次性完成（§99：
module 自行约束资源）。load() 只做廉价初始化（图标管线、
usage 句柄、构建线程 spawn）。

查询侧不设缓存文件、不加新事件：QueryFuture 在 module 内部的
一次性就绪门（CatalogCell）上挂起等待 catalog 就绪，
不阻塞 UI 线程；过期完成由 Core 的 QueryTicket 判定丢弃
（§91/§96），无需取消机制。§109 事件模型不变。
```

原回退案（catalog 落盘缓存）被否决：V1 无 watcher，缓存的唯一
收益是进程启动后首秒内的查询；为它引入持久化格式与版本迁移
不值（§72）。若未来实测后台构建仍频繁慢于可接受阈值，再重新
评估落盘缓存。

---

# 57. GPUI UI

V1 主界面只需要：

```text
Input
Result List
Result Row
Optional Action Menu
```

不要一开始做复杂 Dashboard。

---

# 58. UI Layout

建议：

```text
┌────────────────────────────────────────────┐
│ Search...                                  │
├────────────────────────────────────────────┤
│ Result                                     │
│ Result                                     │
│ Result                                     │
│ Result                                     │
└────────────────────────────────────────────┘
```

没有结果时：

```text
No results
```

Module unavailable 时：

```text
由 Module 提供错误文案
```

---

# 59. Keyboard

Core 统一：

```text
Alt+Space
Open / Close（toggle，见 §53）

Esc
Close

↑ ↓
Selection

Enter
Primary Action
```

Secondary Action 快捷键后续再定义。

---

# 60. Module 不能直接处理 Launcher Keyboard

禁止：

```text
FileModule intercepts ↑
AppModule handles Esc
```

这些都属于 Core。

Module 只提供：

```text
actions
```

---

# 61. Error Model

```rust
pub enum ModuleError {
    Unavailable(String),
    QueryFailed(String),
    ActivationFailed(String),
    InvalidState(String),
    Internal(String),
}
```

V1 不需要极复杂的 error hierarchy。

---

# 62. Query Error

Module query 失败：

```text
QueryResult::Err(ModuleError)（§93）
```

Core 使用统一 Error Row / Empty State。

---

# 63. Module Crash

因为 V1 Module 与 Core 同进程：

> Rust panic 理论上仍有可能影响 Core。

所以内置 Module 必须：

- 避免 `unwrap()` 处理外部数据
- IO failure 正常返回 error
- Windows API failure 正常处理

第三方隔离以后再考虑。

---

# 64. Logging

统一 logger。

例如：

```text
core
module.app
module.file
windows
ui
```

Module 不自己建立日志目录。

---

# 65. Module Enable / Disable

虽然全是 built-in，也建议支持：

```text
Enabled
Disabled
```

例如用户可以完全关闭 File Module。

Disable：

```text
remove trigger
在途 query 与事件随 module_epoch 失效（§96、§109）
unload module
preserve data/settings
```

Enable：

```text
load
register trigger
```

v0.2 约束：**AppModule 在 V1 为 required module，不允许 disable。** 它是唯一 default module（§82），disable 后无 prefix 输入将没有路由出口。Settings UI 对 AppModule 不展示 disable 开关。未来存在其他 default 候选时再开放。

---

# 66. 不做 Module Uninstall

由于 Module 静态编译：

```text
V1 没有真正 uninstall。
```

Settings 中最多：

```text
Enable / Disable
```

这可以进一步降低复杂度。

---

# 67. Future Compatibility

V1 不设计插件 API。

但保持一个边界：

```text
Core
↓
Module trait
↓
Built-in modules
```

将来如果需要第三方 Module：

```text
可以重新设计实现方式。
```

不要求 V1 Trait 成为永久 ABI。

这一点非常重要：

> Rust trait 是当前内部架构接口，不是第三方 ABI contract。

---

# 68. 推荐 Workspace

v0.2 修订：crate 统一 `cue-` 前缀。原名 `core` / `windows` 分别与 Rust 内置 `core` 和官方 `windows` crate（Win32 binding）撞名——本地 path dependency 与 registry 包同名会造成引用与别名混乱。Rust import 名相应为 `cue_core` / `cue_protocol` / `cue_ui` / `cue_windows` / `cue_module_app`。

```text
cue/
│
├── Cargo.toml
│
└── crates/
    │
    ├── cue/                    # binary,composition root
    │   └── main.rs
    │
    ├── cue-core/
    │   ├── session.rs
    │   ├── registry.rs
    │   ├── routing.rs
    │   ├── result.rs
    │   ├── settings.rs
    │   └── usage.rs
    │
    ├── cue-protocol/
    │   ├── module.rs
    │   ├── launcher_module.rs
    │   ├── presentation.rs
    │   ├── action.rs
    │   ├── outcome.rs
    │   └── settings.rs
    │
    ├── cue-ui/
    │   ├── app.rs
    │   ├── launcher_window.rs
    │   ├── input.rs
    │   ├── result_list.rs
    │   ├── result_row.rs
    │   └── settings.rs
    │
    ├── cue-windows/
    │   ├── hotkey.rs
    │   ├── activation.rs
    │   ├── shell.rs
    │   ├── startup.rs
    │   ├── single_instance.rs
    │   └── monitor.rs
    │
    ├── cue-module-app/
    │   ├── lib.rs
    │   ├── discovery.rs
    │   ├── shortcut.rs
    │   ├── packaged.rs
    │   ├── dedup.rs
    │   ├── search.rs
    │   ├── ranking.rs
    │   ├── icon.rs
    │   └── launch.rs
    │
    └── cue-module-file/        # V1.x，暂不创建（§31）
        ├── lib.rs
        ├── everything.rs
        ├── search.rs
        ├── presentation.rs
        └── open.rs
```

---

# 69. 不创建的 crate

当前明确不要：

```text
plugin-sdk
plugin-runtime
module-sandbox
wasm-host
search-framework
universal-index
universal-ranking
capability-runtime
dynamic-loader
```

---

# 70. Composition Root

只有最终 executable 知道有哪些具体 Module。

例如：

```rust
fn compose_app() -> Launcher {
    let mut registry = ModuleRegistry::new();

    registry.register(
        Box::new(AppModule::new())
    );

    // FileModule 随 V1.x 加入（§31）

    Launcher::new(registry)
}
```

`cue-core` crate 不应：

```rust
use cue_module_app::AppModule;
```

具体模块只由顶层 composition root 引入。

---

# 71. 依赖方向

正确：

```text
cue (binary)
├── cue-core
├── cue-ui
├── cue-windows
├── cue-module-app
└── cue-module-file (V1.x)

cue-module-app
└── cue-protocol

cue-module-file
└── cue-protocol

cue-core
└── cue-protocol

cue-ui
├── cue-core
└── cue-protocol
```

错误：

```text
cue-core
→ cue-module-app
```

Core 不允许依赖具体 Module。

---

# 72. 什么时候抽公共库

采用：

> Rule of Three

第一次：

```text
直接写
```

第二次：

```text
允许重复
```

第三次：

```text
确认语义真的相同以后再抽
```

例如：

```text
AppModule
PageModule
CommandModule
```

都需要：

```text
pinyin_initials()
```

才考虑：

```text
text-utils
```

---

# 73. 抽取方向

共享实现只能：

```text
向下沉淀
```

例如：

```text
App ───┐
       ├── text-utils
Page ──┘
```

不要：

```text
App
Page
 ↓
Core 增加拼音 API
```

Core 不应成为杂物库。

---

# 74. V1 必须实现

## Core

- Session lifecycle
- Module Registry
- Input Routing
- Query ticket 与 stale 结果判定（§96）
- Result selection
- Action dispatch
- Settings store
- Usage store
- CoreEffect 输出（§112）

## UI

- GPUI Launcher Window
- Search Input
- Result List
- Result Row
- Keyboard navigation
- Settings Window

## Windows

- Global hotkey
- Show / hide
- monitor placement
- shell open / launch
- Single instance（§113）
- startup support可后置

## App Module

- Start Menu discovery
- packaged app discovery
- dedup
- icon
- fuzzy matching
- 中文拼音支持
- 拼音首字母支持
- usage ranking
- launch

> v0.1 修订：File Module 整体延后出 V1（见 §31）。V1 交付范围为以上 Core / UI / Windows / App Module 四块。

---

# 75. V1 可以延期

- ~~File Module 整体（`/` trigger、Everything 集成、文件/文件夹打开；设计保留于 §31–33）~~（已落地,§118）
- App Paths
- PATH executable discovery
- aliases UI
- ~~Run as administrator~~（已落地,§119）
- ~~Open containing folder~~（已落地,§119）
- ~~secondary actions UI~~（已落地,§119）
- Page Module
- custom trigger configuration
- advanced settings
- themes
- animation polish

---

# 76. 明确不做

- Third-party module
- Plugin marketplace
- Dynamic module install
- Module permission sandbox
- Module filesystem ACL
- JSON-RPC
- WASM
- DLL loading
- AI
- Clipboard manager
- Calculator
- Shell runner
- Network search
- Native filesystem indexer
- Cloud sync

---

# 77. 非功能需求：启动速度

Launcher 本体冷启动应该尽量：

```text
< 500 ms
```

常驻情况下：

```text
hotkey → usable
< 100 ms
```

理想：

```text
< 50 ms
```

---

# 78. 非功能需求：搜索延迟

App 搜索：

```text
P50 < 5 ms
P95 < 15 ms
```

文件搜索依赖 Everything：

```text
目标 UI first result < 50 ms
```

不要求等待所有结果后一次性更新。

---

# 79. 非功能需求：内存

Launcher 常驻内存应保持克制。

V1 暂定目标：

```text
< 100 MB
```

长期可优化至更低。

不要因为预加载大量无用数据导致驻留膨胀。

---

# 80. 非功能需求：稳定性

Launcher 是高频基础工具，因此：

> 稳定性优先级高于功能数量。

任何新增能力如果：

- 增加启动延迟
- 增加常驻资源
- 增加输入延迟
- 增加默认 UI 噪声

都需要明确收益证明。

---

# 81. 产品准入原则

新增 Module 或重大能力前问：

### A. 是否高频？

用户是否每周甚至每天使用？

### B. 是否明显降低 intent → action 成本？

### C. 是否需要成为独立模态？

### D. 是否污染默认路径？

如果不能明确回答，默认：

```text
不加。
```

---

# 82. 默认路径原则

默认输入永远优先满足最常见行为：

```text
打开应用
```

因此：

```text
无 prefix
```

永远对应 App，除非未来有非常充分的理由改变。

文件通过：

```text
/
```

显式进入。

不要把所有结果混排成：

```text
App
File
Page
Folder
History
Web
...
```

---

# 83. 模态隔离原则

```text
App 搜 App

File 搜 File / Folder

Page 搜 Page
```

每个 Module 自己控制结果质量。

Core 不做跨 Module universal ranking。

---

# 84. Core 的最终边界

Core 可以回答：

```text
什么时候打开 Launcher？
现在调用谁？
给 Module 什么输入？
哪个 query 还是有效的？
结果选中了谁？
执行哪个 Action？
什么时候关闭？
Settings 在哪里？
Usage 怎么记？
```

Core不能回答：

```text
哪个 App 更像 yjwj？
哪个 PDF 应该排第一？
Chrome history 怎么搜？
Everything 怎么调用？
Zed 怎么启动？
```

---

# 85. Module 的最终边界

Module 可以回答：

```text
这个输入对我意味着什么？
有哪些结果？
结果怎么排序？
应该显示什么？
结果有哪些 Action？
Action 怎么执行？
哪些业务配置属于我？
```

Module不能：

```text
随意操纵 Launcher Window
直接修改 Core Session
自行绘制完整 GPUI Launcher
控制其他 Module
处理全局快捷键
```

---

# 86. 最终 Core ↔ Module Contract

概念上可以浓缩为：

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

以及：

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

配套关键类型（详见各章）：

```rust
pub struct QueryContext {
    pub query: String,
    pub result_limit: usize,      // Core/UI 请求预算，§94
}

pub struct QueryResponse {
    pub items: Vec<ModuleItem>,   // §95；有效性判定全在 Core（§96）
}

pub struct ModuleItem { /* id + Arc<dyn Any + Send + Sync>，§11 */ }
```

这是 V1 最值得稳定下来的接口。

---

# 87. 一条开发判断规则

以后遇到“不知道代码应该放哪里”的情况，依次问：

### 这个逻辑是否和具体功能语义有关？

例如：

```text
拼音
.lnk
Everything
AUMID
文件路径
browser history
```

是：

```text
→ Module
```

不是，继续。

### 多个 Module 是否都需要？

如果只是一个：

```text
→ 留在 Module
```

如果多个：

```text
→ 先允许重复
```

如果已经稳定重复：

```text
→ 抽 shared utility
```

### 它是否控制整个 Launcher 生命周期？

例如：

```text
window
session
routing
settings
usage
query lifecycle
```

是：

```text
→ Core
```

---

# 88. 第一阶段实现顺序

建议严格按以下顺序开发。

## Phase 1 — Shell

实现：

```text
Alt+Space
GPUI window
input
Esc
Enter
↑ ↓
Single instance（§113）
```

同阶段完成两个 spike：

```text
输入法候选方案验证（§107）
AppModule::load 冷启动耗时实测（§56、§77）
```

此时可以使用 fake Module。

---

## Phase 2 — Module Protocol

实现：

```text
Module
LauncherModule
Registry
Routing
Presentation
Action
Outcome
```

使用：

```text
DemoModule
```

验证 Core 不依赖业务。

---

## Phase 3 — AppModule

实现：

```text
Start Menu
launch
icons
fuzzy
pinyin
ranking
```

这时软件已经具备实际价值。

---

## Phase 4 — Usage

实现：

```text
frequency
recency
ranking feedback
```

优化 Launcher 真正日常手感。

---

## Phase 5 — FileModule

> ~~v0.1 修订：本阶段整体延后出 V1（见 §31）。~~
> **已在 V1.x 落地（§31 定案、§118 实现记录）。**

接入：

```text
/
Everything
file/folder open
```

---

## Phase 6 — Settings

在真正知道 App/File 需要哪些设置之后，再实现 Settings Schema UI。

不要 Phase 1 就写大型 Settings Framework。

---

## V1.x

```text
BookmarkModule（§117,Chromium 书签,触发词 b）— 已落地
FileModule（§31–33、§118,Everything,触发词 /）— 已落地
```

---

# 89. V1 成功标准

当以下体验稳定时，可以认为 V1 产品成立：

```text
Alt+Space
z
Enter
```

能够可靠快速启动用户想要的应用。

（文件模态的验收已随 FileModule 在 V1.x 落地，见 §31、§118。）

要求：

- 输入无明显卡顿
- 排序符合使用习惯
- 中文软件可通过拼音首字母找到
- Launcher 不残留大量业务耦合
- App 实现的修改不影响 Core
- 重复启动 exe 不产生第二实例（§113）

---

# 90. 最终设计哲学

这个项目不追求：

> 提前设计出一个可以支持所有未来能力的框架。

而追求：

> **把当前真实能力实现得直接、清楚，并在系统边界上留下合理扩展余地。**

因此：

```text
抽象边界，
不抽象未来。

统一交互，
不统一业务。

Core 保持无聊，
Module 保持自由。

出现真实重复后再复用，
而不是为了理论上的复用提前制造框架。
```

整个架构最终应该始终可以用一句话解释：

> **Core 提供 Launcher 的运行环境与统一交互；每个内置 Module 自己完成自己的功能，并通过一组很薄的 Trait 告诉 Core：我接受什么输入、返回什么结果、如何展示、能够执行什么。**

---

# 91. 异步任务模型

本章补全 v0.1 缺失的异步任务模型，约束 Core、协议与 Module 的异步边界。

核心模型：

> **Core 是运行在 UI 线程上的单线程状态机。**
> 异步工作以 Future 形式离开 Core，以事件形式回到 Core。

v0.2 修订，一句话模型：

> **Core 不取消异步工作；Core 通过 SessionId、ModuleEpoch 和 Generation 判定异步结果是否仍然有效。Module 自己负责限制后台工作的资源消耗。**

因此：

```text
UI 线程：
Core 状态机
Module 方法调用（load / query / present / actions / activate）
GPUI 渲染

后台：
QueryFuture / ActivationFuture 的轮询与执行
Module 内部线程（如 Everything IPC）
```

Core 自身：

```text
不创建线程
不阻塞等待任何 Future
不为自身状态加锁
```

---

# 92. 异步工作的三种类型

```text
1. Query            高频、可丢弃、可被更新的输入取代
2. Activation       低频、必须完成、结果决定 Session 去向
3. Module 内部任务   索引构建、图标加载等，对 Core 不可见
```

前两种共用同一套 ticket 与事件回流机制（§96）。第三种由 Module 自治。

---

# 93. QueryFuture 与 ActivationFuture

补全 §9 中未定义的类型：

```rust
pub type QueryFuture =
    Pin<Box<dyn Future<Output = QueryResult> + Send>>;

pub type QueryResult = Result<QueryResponse, ModuleError>;

pub type ActivationFuture =
    Pin<Box<dyn Future<Output = ModuleOutcome> + Send>>;
```

等效于 `futures::future::BoxFuture`，但不引入额外依赖。

约束：

```text
必须 Send + 'static
创建 Future 本身必须 < 1 ms
创建时不得触碰 IO / IPC / 磁盘
```

`&mut self` 只用于启动工作。Future 内部持有的是 Module 事先准备好的 `Arc` 状态或 channel，不借用 self。

Activation 的错误在 `ModuleOutcome` 内表达（§22），不单独设 `Err`。

---

# 94. QueryContext

```rust
pub struct QueryContext {
    pub query: String,
    pub result_limit: usize,
}
```

```text
query        trigger 之后的剩余输入（Core 已剥前缀）
result_limit Core/UI 的请求预算：Core 最多展示多少条
             V1 为 Core 内固定值，不来自任何 module.* 设置
```

v0.2 修订：删除 `generation`——staleness 是 Core 的 bookkeeping（§96），不应由 Module 回显。`max_results` 改名 `result_limit` 并明确来源：Core 不知道 `module.file.result_limit` 这类 key 的存在，Module 自己的设置由 Module 经 `ModuleSettings` 自行读取。

---

# 95. QueryResponse

```rust
pub struct QueryResponse {
    pub items: Vec<ModuleItem>,
}
```

v0.2 修订：Module 只回答"结果是什么"。没有 generation 可回显——结果的有效性判定全部在 Core 侧，由 §96 的 QueryTicket 完成。这符合 §3：Core 管如何运行功能，Module 管功能本身。

---

# 96. 完成事件回流与 QueryTicket

v0.2 修订。Core 发起 query 时为它生成一个 ticket——query 的身份完全是 Core runtime 的关注点，不进协议（§94、§95）：

```rust
pub struct QueryTicket {
    pub session_id: SessionId,
    pub module_id: ModuleId,
    pub module_epoch: u64,
    pub generation: u64,
}
```

Core 把 Module 返回的 Future 包装后再 spawn，ticket 由 wrapper 捕获：

```text
Core 记录 ticket
    ↓
spawner.spawn(async move {
    let result = future.await;
    event_sink.send(CoreEvent::QueryCompleted { ticket, result });
})
    ↓ 单一事件队列
UI 线程消费
    ↓
ticket 四项全部匹配 → 更新 ResultState → GPUI 重绘
任一不匹配 → 丢弃
```

事件队列由 Core 创建：

```text
生产端（Send handle）随 spawn 包装进入后台
消费端只在 UI 线程被处理（GPUI foreground task）
```

接受条件：

```text
ticket.session_id   == 当前 session
ticket.module_id    == 当前 active module
ticket.module_epoch == 该 module 当前 epoch
ticket.generation   == 当前 generation
```

`generation` 在每个 session 内从 0 递增即可：跨 session 的旧结果由 `session_id` 保证必死，不存在"新 session generation 恰好相同而误收"的窗口。`module_epoch` 由 Core 在每次 load 时分配、单调递增（§49），unload / reload 后旧实例的在途结果与事件全部失效。

Error 与正常结果走同一 wrapper（`QueryResult` 整体送达），服从同样的 ticket 校验，无需单独规则。

Activation completion 同理绑定 `(session_id, module_id, module_epoch)`，见 §103。

---

# 97. Spawner

Future 的轮询者由外部注入：

```rust
pub trait TaskSpawner: Send + Sync {
    fn spawn(&self, fut: BoxFuture<'static, ()>);
}
```

Core 定义 trait，不提供实现。Core 在 spawn 前把 query future 包装成：等待完成 → 向事件队列投递 CoreEvent（携带 ticket，§96）。

```text
生产环境：ui / launcher 提供（GPUI executor）
测试：手动 pump 的实现
```

Core 因此：

```text
不依赖 GPUI
不依赖 tokio 或任何具体 runtime
可以无窗口单测
```

---

# 98. 关于取消

v0.2 修订：**Core 不提供物理取消。**

`spawner.spawn()` 返回 `()`，Future 的 ownership 即移交 executor，Core 手里没有可以 drop 的 handle——此前"Session 关闭时 drop 所有未完成 Future"的规定在 API 层面不成立，已删除。模型只有一条：

> 在途 query 跑完也没关系；它的结果到达时被 ticket 判定丢弃（§96）。

资源控制的责任在 Module：

```text
App 内存搜索：廉价，随便跑
Everything IPC：Module 内 latest-wins（§99）
```

Module 的 Future 内部仍不得直接执行不可中断的阻塞调用——阻塞工作（Everything IPC、磁盘 IO）放在 Module 自己的线程，Future 只等待完成通知。目的不再是"让 drop 能取消"，而是**保证 executor 线程不被堵死**。

未来出现真正昂贵的 query 时再引入 CancellationToken / AbortHandle，现在不预留（§72）。

---

# 99. Module 内部线程模型（Everything 示例）

以下为 FileModule 内部实现，Core 不知道这些细节：

```text
一个专用 Everything IPC 线程（Win32 IPC 是同步阻塞调用）
一个容量为 1 的请求槽（latest-wins）
```

新 query 到达：

```text
覆盖请求槽
线程总是处理最新请求
被取代的请求结果直接丢弃
```

Everything 单次 IPC 即返回 capped 结果集（按 result_limit 取 Top N），因此 V1：

```text
不需要流式分批
不需要进度态
一次 round trip
```

---

# 100. 并发与顺序

Core：

```text
不序列化不同 generation 的 query
不保证完成顺序
只认 ticket（§96）
```

Module 自行选择内部并发策略。推荐：

```text
昂贵后端（IPC / DB） → 串行 + latest-wins
廉价内存搜索（App）  → 随意，同步完成即可
```

---

# 101. Debounce

V1 不做 debounce。每个按键都发起 query。

理由：

```text
App 查询预算 P95 < 15 ms
FileModule latest-wins 自动吸收输入压力
ticket 机制已保证正确性
```

若实测出现后端压力问题，debounce 只能加在 Module 内部，不进 Core。

---

# 102. 输入变化与 Loading 态

v0.2 修订：**输入变化时立即清空。**

```text
输入改变：
generation++
results.clear()
selection = None
发起新 query
```

旧版本"同 module 内保留旧列表直到新结果到达"已废除：保留期间 ResultState 仍是上一代 query 的结果，此时 Enter 会启动错误的应用——对 launcher 这是致命交互 bug（`z` → Enter 是最典型操作，Enter 经常紧跟最后一个字符）。

clear 通常不可见：App 查询 P95 < 15 ms（§78），在 60Hz 一帧（≈16.7 ms）的预算内，大多数按键不会形成肉眼可见的空帧。不为了避免一个几乎看不到的闪烁，引入 stale result 可激活语义。

V1 不设计 loading 指示。FileModule（V1.x）若确有慢查询需要保留旧结果，届时设计"可见但不可激活"的 stale presentation，现在不预留。

切换 module：立即清空（同前）。

Module 不可用 / 错误仍按 §58 由 Module 提供文案。

---

# 103. Activation 异步

Enter 后：

```text
Core 记录 activation ticket（session_id, module_id, module_epoch）
Core 调用 activate（非阻塞）
UI 保持当前状态
ActivationFuture 完成 → ModuleOutcome 经事件队列回流（§96）
```

Outcome 到达时，处理分两部分：

```text
usage 记录：总是执行（激活真实发生过）
session 处置（Close / KeepOpen）：
  仅当 ticket.session_id 仍是当前 session 时执行
```

v0.2 修订：旧规则"session 存活即处置"有漏洞——Enter 后 Esc、再 Alt+Space 开出新 session 时，旧 activation 的 `Close` 会误关新 session。处置必须绑定发起它的那个 session。

activation 失败（`OutcomeStatus::Failed`）：默认 `KeepOpen` + 统一错误展示（§61、§115）。普通启动失败不得关掉 Launcher。

---

# 104. Panic

v0.2 修订：**V1 不做 panic 边界**，删除此前的 `catch_unwind` 设计。

`catch_unwind` 只能包住 `module.query()` 这个同步调用本身；Future 在 executor 上 poll 时的 panic、Module worker 线程的 panic 都包不住。声称有边界而实际没有，是最差状态。

V1 的防线是 §63 的纪律：

```text
外部数据不得 unwrap()
IO / Windows API 失败返回 ModuleError
worker 线程错误经事件回流，不跨线程传播 panic
```

所有 Module 是可信静态链接代码（§0）。未来确有隔离需求时，需要连 Future poll 一起包装、并保证 release 为 `panic=unwind`，届时完整实现，不在 V1 预留半成品。

---

# 105. UI 线程时间预算

UI 线程上的单次调用：

```text
query() 调用（创建 Future）    < 1 ms
present() 单行                 < 1 ms
actions()                      < 1 ms
activate() 调用（启动 Future）  < 1 ms
```

`present` 中禁止：

```text
磁盘 IO
图像解码
任何 IPC
```

---

# 106. 可测试性

异步模型的注入点（Spawner、事件队列）同时是测试点：

```text
Core 单测：
手动 Spawner        → 控制 Future 完成顺序
乱序完成            → 验证 ticket 丢弃（§96）
Error 到达          → 验证走同一 ticket 校验
跨 session 同 generation → 验证 session_id 拦截
Module reload       → 验证 module_epoch 拦截
Session 关闭        → 验证 Outcome 的 session 处置被丢弃、usage 仍记录
Session A activation 晚于 Session B 到达 → 验证 B 不被误关（§103）
```

这些测试不启动 GPUI、不创建窗口。

---

# 107. 输入法

V1 决定：

> **Launcher 输入框强制英文输入。**

理由：

```text
GPUI 在 Windows 上的 IME 是全项目最大的开放式技术风险
（组合串、候选窗定位、闪烁）

拼音 / 拼音首字母 / alias 已是设计内的一等输入路径（§27）

强制英文把开放式平台风险换成一段确定的 Win32 集成
```

实现：以下两个 Win32 调用为**首选候选，Phase 1 spike 验证后确认**（§88）：

```text
ActivateKeyboardLayout 将 UI 线程切到英文布局
（键盘布局 per-thread，不影响其他应用；目标 HKL 需已加载）

ImmAssociateContext(hwnd, NULL)
使 IME 不附加到搜索框
```

spike 要验证的风险：GPUI 的 Windows 后端自己维护 IME 状态，从外部操作同一 HWND 的 IMM 上下文可能与 GPUI 内部状态不同步。若证实冲突，候选替代：在 GPUI 层禁用该窗口的 text input 处理。

窗口隐藏时**必须恢复**用户之前的输入法（v0.2 修正，原"无需恢复"判断错误）：

```text
键盘布局的激活是前台线程敏感的：在系统默认设置
（"为每个应用窗口使用不同输入法"关闭）下，Launcher 在前台时切英文
会改写全局输入语言——hide 之后用户的其他应用被留在英文。

因此一次唤醒的完整生命周期：
  show 之前（前台仍是用户之前的应用）→ 记录前台线程的 HKL
  show 之后                            → 切英文（上述两个调用）
  hide 之前（窗口仍在前台）            → ActivateKeyboardLayout(记录的 HKL)

已知边界：失焦隐藏路径（§54）执行恢复时窗口已不在前台，
恢复对该路径不保证生效。这是 Win32 输入法模型的固有限制，
不为此引入更重的机制（如向他人窗口投递 WM_INPUTLANGCHANGEREQUEST）。
```

已接受的代价：

```text
用户不能直接输入汉字
多音字偶尔会匹配错误
中文应用命中完全依赖拼音索引质量（§27、§74）
英文名 / alias 的价值上升
```

粘贴不受此限：IME composition 与剪贴板是两条独立路径，允许粘贴 Unicode（§115）。

未来如需中文直输，作为设置项重新评估。V1 不预留 IME 代码路径。

---

# 108. Result Row 布局规则

v0.1 确认：

> **网格固定，内容可选。一套布局，没有第二套。**

ResultPresentation 除 title 外全部可选（§13），布局不随字段有无而切换：

```text
┌────┬───────────────────────────────┬───────────┐
│icon│ title                    badge│ accessory │
│槽位│ subtitle                        │           │
└────┴───────────────────────────────┴───────────┘
 固定宽    弹性宽                       右对齐
```

规则：

```text
icon 槽位永远占固定宽度（如 44px），None 即留空
文字起点永不移动
行高固定，icon 在槽位内垂直居中
subtitle 为 None 即第二行留空
没有专属图标的 Module 返回 SystemIcon 或 None
```

因此：

```text
V1 无图标 → 后来有图标：布局零改动
有图标的 Module 与无图标的 Module：同一布局
图标异步后到：文字不跳动，无 reflow
```

布局完全属于 ui crate（§13）。Core 不知道网格存在。

---

# 109. Module 自发事件

图标等异步资源完成时，Module 需要主动通知 Core，而不是等 Core 发起调用。

v0.1 新增 Module → Core 的第三条通道（前两条：query response、activation outcome）：

```rust
pub enum ModuleEvent {
    PresentationInvalidated { items: Vec<ItemId> },
}
```

Module 通过 ModuleContext.events（§49）发送：

```rust
pub trait ModuleEventSink: Send + Sync {
    fn send(&self, event: ModuleEvent);
}
```

事件走 §96 的同一事件队列回到 UI 线程。

`ModuleEventSink` 在 `load` 时绑定 `(ModuleId, ModuleEpoch)`（§49）：unload / reload 之后，旧 sink 发出的事件一律丢弃。

Core 收到后：

```text
epoch 不匹配（旧实例事件）→ 丢弃
session 关闭或 module 非 active → 丢弃
items 不在当前结果中 → 忽略
否则对对应可见行重新执行 present() 并更新
```

事件不影响 generation，不触发重新 query。

典型流程（图标）：

```text
present() 返回 icon: None（尚未提取）
Module 后台提取并写入自己的 cache（§47）
完成后 send(PresentationInvalidated { items })
Core 重跑 present()，行内出现图标
```

V1 只有 PresentationInvalidated 一种事件。未来新增事件类型时按 §87 判断归属。

---

# 110. 跨平台立场

架构不阻塞跨平台（macOS / Windows），但 V1 不做任何跨平台抽象。

依据：

```text
core / protocol / ui 中没有 OS 特定概念，可原样移植
平台代码已被隔离在 windows/ crate 与各 Module 内部
GPUI 本身跨平台，且 macOS 是其最成熟的平台
```

Module 内部的平台差异由 Module 自己吸收：

```text
App discovery：Windows 用 Start Menu + Package API
              macOS 用 /Applications + Info.plist
搜索 / 拼音 / ranking / usage：平台中立，复用

FileModule：Windows 依赖 Everything（§31 的延期原因）
            macOS 有系统自带 Spotlight 索引，该 blocker 不存在
```

因此 V1 不做：

```text
不抽象 HostPlatform trait
不引入 cfg 平台分流
不搭建双平台 CI
```

平台版 Rule of Three：第二个平台真实动工时再抽象，届时 windows/ crate 作为 host 接口的参考实现。

产品层差异（热键选择、系统权限、窗口规则 §54、输入源切换）属于新平台的产品决策，动工时逐条重估，不在本 spec 预设答案。

---

# 111. Core 的平台代码规则

> **没有明显收益，不在 core / protocol 中引入任何 platform-specific 代码。**

默认检查项（可 grep / CI）：

```text
core 与 protocol 中禁止出现：
std::os::windows
windows crate 类型
Win32 API 调用
```

例外需要明确收益证明（举证责任在引入方，同 §80）。

`core.*` 的设置值同样受此约束：

```text
设置值必须是 OS-neutral 数据描述
（如 Hotkey 的 modifiers + key 枚举，§53）
平台翻译属于 Host

module.* 设置值由 Module 自定：
Module 本来就可以是平台特定的（如 Everything 连接）
```

此规则不约束：

```text
windows/ crate（平台代码的隔离区）
Module 内部实现（Package API、Everything、图标提取等）
ui crate 中 GPUI 的平台后端
```

规则的目的不是跨平台，而是让 Core 保持可测试、可移植、无平台耦合。跨平台只是副产品。

---

# 112. Launcher 控制流（CoreEffect）

v0.2 新增。§4 画出了组件，但没有规定它们之间的控制流。闭合方式：**launcher crate 是编排层**（composition root，§70）。

```text
Windows Host / GPUI UI 产生事件（WM_HOTKEY、失焦、按键……）
    ↓ HostEvent / UIEvent
launcher 接收并翻译
    ↓
Core 状态迁移（open_session / close_session / input_changed …）
    ↓
Core 返回平台无关的 CoreEffect
    ↓
launcher 调用 cue-ui / cue-windows 执行
```

```rust
pub enum CoreEffect {
    ShowLauncher,
    HideLauncher,
    FocusInput,
}
```

V1 只有这三个。launcher 执行时：

```text
ShowLauncher → cue-windows 计算当前 monitor / 位置（§54）
             → cue-ui show window → FocusInput
HideLauncher → cue-ui hide window
```

约束：

```text
Core 不知道 GPUI / HWND / RegisterHotKey 的存在
cue-ui 不知道 Module 的存在
cue-windows 不知道 Core 内部状态
```

唯一的同步例外：core.* 设置里需要 Host 副作用的 try-apply（§53 热键注册、core.start_on_boot 登录启动项）是设置事务的一部分，必须同步——Core 持有注入的 `apply_hotkey` / `apply_start_on_boot` 回调直接调用，失败不 commit（§42）。这是函数，不是 HostPlatform trait（§110）。

Windows Host 需要的 GPUI HWND（§107 IME、§54 窗口规则）由 launcher 在窗口创建后从 cue-ui 取得并交给 cue-windows。

---

# 113. 单实例（Single Instance）

CUE 是 **single-instance application**。V1 必须实现。

实现（cue-windows）：

```text
第一实例：创建 named mutex（如 Local\CUE.SingleInstance）
第二实例：检测到 mutex 已存在
  → 通过薄 IPC 通知第一实例 show / focus
  → 立即退出
```

不做：

```text
多实例并存
第二实例"安静驻留"
```

理由：全局热键、开机启动、后台常驻的组合下，双实例必然互相打架——第二个进程 `RegisterHotKey` 会失败，但若仍继续初始化 GPUI、扫描 App、驻留内存，纯属浪费；settings / usage 还可能被双写。单实例同时锁定一个全局假设：**settings 与 usage store 永远单写者。**

---

# 114. 性能测量契约

§55、§77–79 的指标按以下统一起止点测量，否则数字无法写成确定性 benchmark：

```text
Hotkey latency：
  WM_HOTKEY 收到 → 输入框聚焦、第一个按键可被接受
  （§55：< 100 ms / ideal < 50 ms）

Search latency：
  InputChanged 事件进入 Core → 新 ResultState 提交
  不含 GPU present
  （§78：P50 < 5 ms / P95 < 15 ms）

Cold start：
  进程入口 → 全局热键已注册且 launcher 可用
  （§77：< 500 ms）

Memory：
  Private Working Set；App catalog 加载完成、窗口隐藏、空闲 60 s 后采样
  （§79：< 100 MB）
```

**V1 实测（2026-08-21,release 构建,Windows 11,注入输入 E2E）**：

```text
Cold start：113–125 ms（稳态;新二进制首跑受 Defender 扫描影响 ~2.2 s,
  属环境噪声)✅ < 500 ms
Hotkey latency：92–98 ms(稳态 N=10,含注入与轮询开销数 ms;
  进程内首次唤起 406 ms —— 首帧 GPU/渲染器初始化,属已知待优化项,
  可考虑启动时离屏预热一帧)✅ < 100 ms
Search latency：P50 = 0.38 ms / P95 = 0.52 ms(N=13,视图侧
  input→rows 上界,含事件泵与 present,比 §114 定义略宽)✅
Memory：63.1 MB(Private Working Set,idle 60 s)✅ < 100 MB
```

**首唤预热落地（2026-08-22）**：启动后 700 ms 空闲点离屏显示 + 强制一帧
（SWP_NOACTIVATE，不抢前台、不开会话;会话一旦开过则跳过），把 DirectX /
DirectWrite / 字形缓存的惰性初始化移出唤起路径。复测（直发 WM_HOTKEY 到
host 窗口、剔除注入管线噪声，该路径当时被本机另一占键应用拖慢到 ~230 ms，
属环境干扰）：**首次唤起 22 ms、稳态 15 ms**,406 ms 尖峰消除。

---

# 115. UX 不变量

v0.2 汇总若干写代码时一定会撞上的 UX 决策：

```text
空查询：
  打开后无输入时显示 usage Top Apps（§52；Phase 4 起有数据）。
  无 usage 数据时显示空列表，不显示任何"推荐内容"。

默认选中：
  新结果到达且非空 → 选中第 0 项；为空 → selection = None。

输入变化：
  立即清空 results 与 selection（§102），不保留上一代结果。

Activation 失败：
  默认 KeepOpen + 统一错误展示（§103）；
  普通启动失败不得关闭 Launcher。

粘贴：
  允许粘贴 Unicode。IME 禁用只针对 composition（§107），
  剪贴板是独立路径——粘贴"微信"应能正常命中拼音索引。

失焦：
  core.hide_on_focus_loss（§36）控制失焦是否隐藏（§54）；
  toggle 第三态（可见但未聚焦 → 聚焦）依赖此设置存在（§53）。
```

---

# 116. 托盘图标与退出路径

Launcher 常驻但窗口默认隐藏（§54），因此托盘图标是进程存活的
**唯一常驻可见信号**，也是 V1 唯一的退出路径——没有它，用户只能
用任务管理器杀进程。

V1 决定：

```text
进程运行期间托盘图标始终存在（不提供"隐藏托盘"设置）
左键点击  → 唤起（等价 §113 的 ShowRequested）
右键菜单  → 显示 / 设置 / 退出
设置      → 打开 §41 的设置视图（Core 出模型，GPUI 渲染）
退出      → 删除托盘图标（不留幽灵图标）、注销热键、进程退出
```

不为托盘做更多：不弹气泡通知、不做双击行为、不做开机启动开关
（开机启动是 Settings 候选，Phase 6 评估）。

托盘回调消息投递到 host window（§113 的 Win32 消息入口）；
host window 因此是隐藏顶层窗口而非 message-only——托盘菜单要求
owner 可设为前台，message-only 窗口做不到（MSDN 托盘菜单模式）。

---

# 117. BookmarkModule（Chromium 书签,V1.x)

浏览器书签搜索。设计参照 Flow Launcher 的 BrowserBookmark 插件,
按 CUE 架构约束裁剪。

## 范围与触发词

```text
module id  = bookmark
trigger    = b（词边界规则见 §5.2:b<空白> 或裸 b 才命中,不吞 baidu)
范围       = Chromium 系(Edge / Chrome),含多 profile
明确不做   = Firefox(places.sqlite 带锁,需拷贝解锁 + SQL——
             不值得为它引入 rusqlite 依赖;本机亦无 Firefox 生态)
```

## 数据源与刷新

```text
数据源 = <User Data>/<profile>/{Bookmarks, AccountBookmarks}
         (JSON,无锁,浏览器运行中可读;Bookmarks = 本地书签,
         AccountBookmarks = 账号书签——登录 Google 账号后 Chrome
         只写后者、本地 Bookmarks 停更,两者同构、可并存,都收)
发现   = 枚举 User Data 下所有 profile 目录的这两个文件
解析   = 递归 roots;凡带 children 数组的对象即容器(folder /
         bookmark_bar / other / synced / workspace / custom_root
         一视同仁),type == "url" 才是书签;坏 JSON / 缺字段跳过(§63)
刷新   = 每次 query 在模块后台 future 里重跑发现 + stat,
         (路径, mtime, 长度) 指纹变了才重解析——无 watcher(§56 精神),
         热路径零 IO;JSON 亚毫秒级,全量重解析不做增量
```

"Default" profile 不标注;其余 profile 名进副标题(Flow 同款)。

## 搜索 / 排序 / 展示

```text
搜索键 = [title(原始大小写), pinyin_full, pinyin_initials, domain]
排序   = score desc,再 title_lower(§27/§28 同款,usage_bonus 同公式)
空查询 = usage Top Bookmarks(§115 一致:无 usage 不显示"推荐")
行展示 = title + 副标题(domain,非 Default profile 追加 "· <profile>")
       + accessory(浏览器名 Edge/Chrome)
图标   = 来源浏览器 exe 图标(load 时后台线程一次性提取,≤2 张);
         提取失败 / 浏览器未装 → SystemIcon::Generic。
         不做网站 favicon(Flow 默认关:需读 Favicons sqlite +
         拷贝解锁,复杂度与依赖不值)
```

## 打开与 usage

```text
打开     = 从哪来回哪开:来源浏览器 exe + URL 参数启动
         (exe 缺失时退回系统默认浏览器,宁可降级不让激活失败)
item_key = {browser}:{url}(§51;不同来源是不同启动动作,分开计数)
session  = Close;失败保持打开并报错(§115)
```

## 依赖决策记录

新增 workspace 依赖 `serde_json`(Chromium Bookmarks 是 JSON;
settings/usage 的手写 TSV 不适合这个量级与嵌套结构)。
matcher / pinyin_index / usage_bonus / 图标提取 复制自
cue-module-app——Rule of Three 第二次使用(§72);第三个消费者
(FileModule)落地后,图标/COM/shell_execute 已下沉 cue-util-win。

---

# 118. FileModule 实现记录(Everything,V1.x)

文件搜索。§31–33 的实现落点;设计裁剪与关键决策记录于此。

## 范围与触发词

```text
module id  = file
trigger    = /(标点触发,§5.2 verbatim 匹配;/ 之后原样进 Everything)
范围       = Everything 索引内的文件 + 文件夹,同一模态(§33)
查询语法   = Everything 原生语法原样透传(子串、ext:、路径过滤……)
             ——query syntax 归 Module,Core 不解析(§3)
明确不做   = 自建索引(§76)、随包分发 Everything、Everything.dll/SDK 进仓库、
             按扩展名的真实类型图标、usage 重排、~~次级 action~~(已落地,§119)
```

## 依赖与 IPC(§31 定案的执行)

```text
依赖   = 本机已安装并运行的 Everything 1.4(QUERY2W 需要 1.4.1+);
         未运行 / 主版本不符 → ModuleError::Unavailable,行内错误文案
协议   = 直连 WM_COPYDATA,逐字节对齐官方 SDK(everything_ipc.h /
         Everything.c,随取随弃,不进仓库):
         FindWindowW("EVERYTHING_TASKBAR_NOTIFICATION") →
         WM_USER 握手主版本 → WM_COPYDATA(dwData=18, QUERY2
         = 7×u32 头 + NUL 结尾 UTF-16 搜索串)→ 应答 WM_COPYDATA
         回 reply_hwnd(LIST2 = 5×u32 头 + ITEM2[n] + 变长数据)
变长   = 按 request flag 位升序;字符串 = u32 字符数(不含 NUL)+
         文本 + NUL;SIZE/DATE = 8 字节。一切读取查边界(§63)
排序   = NAME_ASCENDING 原样展示(SDK 保证该序无性能损失);
         V1 不做 usage 重排(Everything 的序就是预期序)
请求字段 = FULL_PATH_AND_NAME | SIZE | DATE_MODIFIED
         (DATE_MODIFIED V1 不展示——留它让变长解析的位序有第三个
         数据点,测试断言其值域;V1.x 展示/排序直接用)
```

## 线程模型(§99 照办)

```text
专用 IPC 线程(Win32 IPC 同步阻塞)+ 容量 1 的 latest-wins 请求槽:
新输入顶掉旧请求,旧 future 以 Canceled 结束,Core 按 ticket 丢弃(§91)
应答窗口(message-only)在 IPC 线程上创建,GetMessage 泵派发"发送"型
应答(官方 dll 同款流程);2 s WM_TIMER 兜底,不应答不吊死
load() 只起线程,不触碰 IPC(§55:唤烧热路径无 Everything 初始化)
```

## 空查询与展示

```text
空查询 = 空结果。UsageRead 只能按键查、不能枚举(§50),
         给不出 Top Files;不显示任何推荐内容(§115 精神)
行展示 = title(文件名)+ 副标题(父目录)+ accessory
         (文件夹 → "文件夹";文件 → 格式化尺寸)
图标   = 文件夹 / 通用文件各一张,load 时后台线程一次性提取
         (SHGFI_USEFILEATTRIBUTES,不触盘);失败 → SystemIcon 兜底。
         逐类型图标(Flow 同款)需要按扩展名缓存,V1.x 再议
usage  = item_key = 全路径(§51);PRIMARY Open = ShellExecute 默认动词
         (文件走系统关联,文件夹进资源管理器);成功 Close(§115)
```

## 验证

```text
单测 = QUERY2 布局、LIST2 解析(合成缓冲)、畸形输入不 panic(§63)、
       parent/name 拆分、present 兜底、空查询、live 冒烟
       (Everything 不在自动跳过)
实测 = 真机 E2E:/explorer → 8 行(文件夹图标/名称/父目录/配件齐全),
       input→rows 18–40 ms(IPC 往返,逐键),Enter 打开首行文件夹,
       usage.tsv 正确落(file, PRIMARY, 全路径)
注意 = FileModule 的覆盖范围 = Everything 的索引范围。Everything
       服务未运行 / 索引过期时,结果偏少是数据源问题,模块如实展示
```

Rule of Three:icon / com / shell_execute 在此第三次复制(§72),
随后即下沉 cue-util-win(模块共享 Win32 助手,只下沉不上浮)。
---

# 119. 次级动作菜单(Action Menu,V1.x)

§18 Action Model 的 UI 落地;§75 三项(Run as administrator /
Open containing folder / secondary actions UI)随此解封。

## 交互

```text
打开   = Tab(有选中项且模块给出非空动作集时)
菜单键 = ↑↓ 选择 · Enter 执行选中动作 · Esc/Tab 关闭
模态   = 其余按键:关菜单并吞掉(不落进搜索输入)
Esc 分层 = 菜单开着 → 关菜单(结果列表原样恢复);菜单关着 → 关会话
头部   = "动作 · <选中项标题>"(打开时 present 快照)
不做   = 每动作快捷键(§59 维持"后续再定义")、菜单内搜索、鼠标
```

## Core(§5 状态机的第三个子状态,同 §41 设置页模式)

```text
SessionState.action_menu = Option<{ actions 快照, selected, item_title }>
open   = Tab 时对选中项同步调 Module::actions(廉价,一次性快照)
激活   = activate_selected 泛化为 activate_with(ActionId);
         菜单 Enter 先关菜单再以选中 ActionId 激活——失败时
         用户看到结果列表 + 错误横幅(§115)
失效   = 输入变化(§102)或新结果提交 → 关菜单
         (动作快照属于旧选中项,不打到新结果上)
usage  = 按各自动作 ActionId 记账(§50 键本含 ActionId;
         "复制路径" 不污染 PRIMARY 的启动频率)
UI 模型 = action_menu_model()(label + 预格式化 shortcut + selected);
         UI 只渲染,键盘路由按 in_action_menu() 分流
```

## 平台原语(cue-util-win 新住户)

```text
shell_execute_elevated = ShellExecuteEx lpVerb="runas"(UAC;
                        用户取消 = ERROR_CANCELLED,按普通失败展示)
reveal_in_explorer     = explorer.exe /select,"<path>"(文件/夹通用;
                        不引 COM/PIDL——一行动词,Failure 面最小)
clipboard::set_text    = OpenClipboard → EmptyClipboard →
                        SetClipboardData(CF_UNICODETEXT) 单次写入
                        ——是 §18 的 Copy path/link,不是 §76 的
                        clipboard manager(无历史、无监听)
```

## 模块动作集(§18;label 中文,顺序即菜单顺序)

```text
App      = 打开 / 以管理员身份运行 / 打开所在位置
           (后两个仅 Win32 目标:packaged 由系统代理激活,
           没有可提权/可定位的 exe,actions() 不声明)
File     = 打开 / 打开所在文件夹 / 复制路径
Bookmark = 打开 / 复制链接
未知 ActionId = ActivationFailed(明确报错,不静默降级,§63)
```

## 验证

```text
单测 = Core 5 例(打开/导航钳制/ActionId 路由/PRIMARY 不变/§102 关菜单)、
       三模块动作集与顺序、packaged 拒绝次级动作、
       剪贴板真机回环(CF_UNICODETEXT 写入再读回)
E2E  = 真机:Tab 出菜单(截图:文件 3 行/书签 2 行,头部归属正确),
       ↓↓ Enter 复制路径 → 剪贴板 = C:\Windows\explorer.exe,
       会话关闭;Esc 分层(先关菜单回结果页,再 Esc 关会话);
       usage.tsv 落 (file, action_id=2, 全路径)
```

# 120. FileModule 噪声目录排除(V1.x)

`/ds` 之类短查询的结果曾被 `C:\Program Files\...` 安装目录内部文件
淹没——那些不是用户工作时会用的文件。决议:黑名单而非白名单——
白名单砍掉 Everything"全盘随查"的核心价值(无法预知目标文件落在
哪个角落),而噪声高度可预测。

## 决议

```text
形态     = 默认开启的排除名单(黑名单),不是结果后过滤:
           FileModule 把否定子句拼进发给 Everything 的查询串,
           噪声路径根本不占结果位
实现     = Everything 原生语法:含反斜杠的词按全路径子串匹配,
           `!"片段"` 即否定。查询 = `{用户输入} {EXCLUDE_CLAUSE}`
名单     = C:\Windows\、C:\Program Files\、C:\Program Files (x86)\、
           \$Recycle.Bin\、\node_modules\、\.git\
           (保守:AppData、ProgramData 不排除——确有用户文件)
逃生口   = 查询含 `\` 视为显式路径输入,原样发送不加排除——
           刻意找系统文件(如 /C:\Windows\explorer.exe)不被拦
设置     = module.file.exclude_noise_paths(Bool,默认 true,
           Immediate)——首个 module.* 设置;V1 设置 UI 的 Bool
           行直接可编辑,settings.tsv 持久化
           ~~(名单本身不设 String 编辑 UI)~~(已推翻,§121)
明确不做 = ~~用户自定义名单 UI~~(已落地,§121)、
           按名单重排(软黑名单)、白名单模式
```

> 名单的"保守"判断(AppData 不排除)在真机使用中证伪:
> AppData\Local、.vscode\extensions 才是短查询的主要噪声源。
> 名单细化见 §121;可编辑化的最终形态(模块数据文件 +
> 默认编辑器打开)见 §122。

## 验证

```text
单测 = effective_search 三条路径(默认拼接/开关关闭/显式路径逃生)、
       ext: 函数查询仍走排除;schema 声明与 try_apply 回合
       (类型错误 Err 不留半状态);Core 集成:module.* 设置
       事务直达模块、入模型、未知 key 拒绝(首个 module.* 设置
       的端到端覆盖)
E2E  = 真机:/ds 结果从满屏 MATLAB 安装目录变为用户目录文件
       (截图);/C:\Windows\explorer.exe 照常命中(逃生口,截图)
```

# 121. 排除名单细化与可编辑名单(String 设置)

§120 的初版名单过粗:`/d` 的结果仍被 `AppData\Local\MathWorks`、
`.vscode\extensions` 这类"工具内脏"淹没。两处修订:默认名单细化,
名单本身成为可编辑的 module 设置。

## 现成口径的调研结论

```text
VS Code   = files.exclude / search.exclude 默认名单是事实标准:
            **/node_modules、**/bower_components、**/.git、**/.svn、
            **/.hg(+ .DS_Store / Thumbs.db 这类文件级,对目录排除
            不适用)
Windows   = Search 索引器默认范围 = 用户目录但明确排除 AppData——
Search    "AppData 默认不可见"有系统级先例
社区实践  = Everything 用户名单普遍加 __pycache__、.venv、
            go/pkg/mod 等语言包缓存(voidtools 论坛 / 配置分享)
```

## 决议

```text
默认名单 = 两类片段(都以 \ 结尾,锚定"目录"):
           全局片段(任意位置生效,VS Code 口径):
             C:\Windows\、C:\Program Files\、C:\Program Files (x86)\、
             \$Recycle.Bin\、\node_modules\、\.git\、\.svn\、\.hg\、
             \__pycache__\、\.venv\、\bower_components\
           USERPROFILE 展开(工具内脏,Windows Search 口径):
             AppData、.vscode、.cursor、.cargo、.rustup、.gradle、
             .m2、.npm、.nuget、.docker、.android
           (有意保留 .claude / .ssh 等:用户会直接找;
           项目内 .vscode 不在名单——编辑的是项目配置,不是扩展缓存)
设置     = ~~module.file.excluded_paths(String,分号分隔片段,
           Immediate;try_apply 时重建否定子句)~~(已推翻,§122:
           名单搬出 Settings Host,成为模块数据文件)
           + module.file.exclude_noise_paths(Bool 总开关,§120)
UI       = ~~设置页 String 行进入编辑态(视图本地缓冲,
           与搜索页同一套文本录入:命名键 + key_char + space 特判
           + Ctrl+V;Enter 提交走 §42 事务、Esc 放弃)——
           V1 编辑面扩展为 Bool + Hotkey + String
           (首个 String 设置,无独立文本框组件,复用行内渲染)~~
           (已推翻,§122:长名单行内编辑无光标移动,编辑面回退为
           Bool + Hotkey;名单改由默认编辑器打开)
逃生口   = 不变(查询含 \ 原样发送);名单清空 = 不排除
明确不做 = 名单的正则/通配语法(子串足矣)、按名单重排、
           白名单模式、AppData 内细分(整目录排除 + 逃生口已够)
```

## 验证

```text
单测 = build_clause 解析(分号/trim/空段)、默认名单含
       AppData 与 .vscode 根、effective_search 四条路径
       (默认拼接/总开关关闭/空名单/显式路径逃生)、
       try_apply 双键回合(类型错误 Err 不留半状态)
E2E  = 真机:/d 结果不再出现 MathWorks ServiceHost 与
       .vscode\extensions(截图);设置页名单行可编辑(截图)
```

# 122. 名单外置:模块数据文件 + 默认编辑器打开(Path 设置)

§121 的可编辑名单上线即暴露交互短板:400 字符的分号串在无光标
移动的行内编辑里几乎不可改——名单本质是"一份给人编辑的文档",
不是"一个设置值"。两处修订:名单从 Settings Host 搬出为模块
数据文件;设置页改为提供"打开它"的入口。

## 决议

```text
归属     = ~~modules/file/data/excluded-paths.txt~~(格式推翻,
           §123 改 TOML;模块数据文件的定位不变)(§43 data 桶:
           持久用户数据,aliases 同类)。~~每行一个路径片段,
           # 起注释~~;首启播种(注释头 + §121 默认名单);
           清空~~文件~~数组 = 不排除;文件被删 = 下次 load 重新播种
           Settings Host 只留 Bool 总开关——单一事实源,
           无双源同步问题(§48 管辖的是"值",不是文档)
读法     = mtime 指纹(§117 BookmarkModule 已验证的模式,
           无 watcher):query future 在后台线程 stat,
           变了才重读重编译否定子句;读取失败~~保留旧子句~~
           或语法错误保留旧子句(mtime 照记,同一坏版本
           不重复重读重报;编辑器半保存状态不该打烂搜索);
           UI 线程零 IO,§93 查询创建预算不破
设置页   = 新增 module.file.excluded_paths_file(Path 类,
           值 = 名单文件指针;schema 注册先于 load,路径按
           编排层同一公式从环境重算)。Path 行回车 = 用系统
           默认程序打开当前路径值——首个 Path 类设置的
           激活语义,不是值变更,不走 §42 事务
打开通道 = CoreConfig.open_path 同步 host 回调(§53 同款
           同步例外:函数不是 trait,UI 线程调用);
           编排层实现 = explorer.exe <path>(拉起默认关联,
           其返回码不可靠故忽略,只认 spawn 失败)
编辑面   = 回退为 Bool + Hotkey(行内 String 编辑随名单
           外置一并撤除——克制:没有 String 设置就没有编辑面)
逃生口   = 不变(查询含 \ 原样发送)
明确不做 = 编辑器关闭/保存的事件监听(mtime 惰性检查已够)、
           名单值经设置事务回流(双源)、行内 String 编辑组件、
           名单内容的多行 TSV 转义(文件天然支持换行)
```

## 验证

```text
单测 = build_clause 行解析(注释/空行/CRLF)、播种文件
       回合(注释头 + 默认片段可编译)、mtime 指纹三态
       (变了重读/不变不重读/消失保留旧子句)、
       schema Bool+Path 声明与 try_apply 回合;
       Core:open_setting_path 回调收值、不 commit、
       非 Path/未知 key 拒绝、回调失败进模型
E2E  = 真机:设置页 Path 行回车 → 默认编辑器打开名单文件
       (截图);编辑保存后下一次查询生效(mtime 重读)
```

# 123. 给人编辑的配置文件一律 TOML(名单格式修订)

§122 的纯文本行格式能用但不像"配置":无高亮、无结构、无标准。
定下通则:**凡是给用户手工编辑的配置文件,一律 TOML**;
机器单方读写的存储(settings.tsv、usage.tsv)不在此列——
它们不是"给人编辑的配置"。首个适用对象即排除名单。

## 决议

```text
格式     = excluded-paths.toml:`excluded = [ '片段', ... ]`
           路径用 literal string(单引号)——内容逐字,
           Windows 反斜杠免转义,是这类名单的天然容器;
           注释头说明格式与逃生口,数组内行尾也可加注释
解析     = toml crate(标准解析,不手写子集——手搓"简化
           TOML"是最糟糕的半标准);无 excluded 键 = 空名单
错误纪律 = 语法错误/非字符串元素 → 保留旧子句 + Warn 日志,
           mtime 照记(同一坏版本不重复重读重报);load 时
           读取/解析失败 → 内置默认子句,不阻塞 load
迁移     = 无:.txt 行格式只存在于未提交的中间态(未发版),
           直接切换,不写迁移代码(YAGNI)
明确不做 = settings.tsv/usage.tsv 改 TOML(机器单方读写,
           不是给人编辑的);名单内容进 Settings Host(§122 已定:
           文档不是"值");toml 序列化写回(播种只写受控默认值)
```

## 验证

```text
单测 = TOML 解析(literal/基本字符串、注释、空键)、语法错误与
       非字符串元素报 Err;播种文件回合;坏版本保留旧子句且
       mtime 照记、改对后生效
E2E  = 真机:.toml 播种;外部缩减名单后 /d 噪声回归(重读生效)
```

# 124. 文件结果的真实图标(按路径/扩展名异步提取)

初版文件结果只有两枚通用图标(文件夹/文件),`/geek.exe` 这类
可执行文件不显示内嵌图标。补全为 AppModule 同款图标管线:
提取异步化,完成推 `PresentationInvalidated` 让 Core 重画可见行。

## 决议

```text
缓存 key = exe 类(exe/lnk/msi/bat/cmd/com/scr,图标内嵌于文件)
           按全路径;其余按小写扩展名(同类文件共享一枚,
           虚拟名 "x.<ext>" 走 SHGFI_USEFILEATTRIBUTES,不触盘);
           文件夹与无扩展名文件不进队列(通用图标)
线程模型 = module 自有 worker 串行提取(AppModule IconPipeline
           同款;两处相似而非相同——Slot 模型与 key 策略不同,
           按 §72 第二处允许重复,不下沉 util)
失效寻址 = 完成时以当前行快照(last_items)发失效事件;
           Core 只取与当前结果的交集(any 命中即整体重画),
           同扩展名兄弟行天然覆盖,请求登记行滚走也不怕
纪律     = present() 热路径零 IO(锁内查表,未命中登记 Pending);
           失败负缓存不重试;缓存超 512 枚整体清空(≈ 19 MB,
           重建便宜,不做 LRU);通用图标仍是全部兜底,
           再退 SystemIcon
明确不做 = 图标磁盘缓存(提取是毫秒级,重启重建足够)、
           队列优先级/可见行优先(20 行规模无意义)、
           第三处出现前的管线下沉(§72 Rule of Three)
```

## 验证

```text
单测 = icon_key 路由(文件夹/无扩展名 None;exe 类全路径;
       扩展名小写共享)、extract 未知前缀拒绝
E2E  = 真机:/geek.exe 行从通用白图标变为 exe 内嵌图标
       (异步提取 + 失效重画,截图)
```

# 125. 排除名单通用化(目录锚定 `\AppData\` + ProgramData)

§121 的默认名单把 AppData 与工具缓存按 USERPROFILE 展开,真机
证伪:多用户配置与沙箱配置(WsAccount、CodexSandboxOffline/Online、
Default)的 AppData 照样涌入结果;ProgramData 同样是应用数据
而非用户文件(Windows Search 仅为 Start Menu 收录它)。§121 对
AppData 的反转先例在此继续:排除口径从"精确但只管自己"改为
"目录锚定通杀"。

## 决议

```text
新默认 = 系统目录(C:\Windows\、Program Files ×2、ProgramData、
         $Recycle.Bin)+ 通用 '\AppData\'(任意配置通杀)+ 依赖
         目录(node_modules/.git/…,口径不变)+ USERPROFILE 展开
         的工具缓存(.vscode/.cargo/…,保持按户:项目级同名目录
         多是配置而非缓存,不按通用排除)
存量升级 = 一次性:文件内容恰为旧默认名单(用户一个片段都没
         动过)才重写为新默认;增删过任何片段即不触碰;幂等
         (新默认 ≠ 旧默认,不会二次触发);读取/解析/写入失败
         都不致命,沿用现状
```

## 验证

```text
单测 = 旧默认内容触发重写且幂等、自定义内容不动、
       seed 子句含 ProgramData 与通用 \AppData\
```

# 126. 系统动作模块(触发词 `>`)

固定枚举的系统动作——锁屏/睡眠/休眠/注销/重启/关机/清空回收站。
不是 shell runner:不接受任意命令(§26 禁区不动),动作表是
模块内静态数据。

## 决议

```text
动作表 = 7 项静态 ActionSpec(name/pinyin/initials/english/extras
         手校;7 个固定中文名不需要 pinyin 引擎——重启的
         chóng/zhòng 多音两条键都收)
匹配   = 完全相等 > 前缀 > 子串(120/100/60);usage 加分封顶
         40(次数 ≤25 + 7 天新近 15)= 匹配等级差,最多追平、
         由表序决胜——usage 重排同级匹配,压不过更强匹配
空查询 = 列出全部动作,usage 在前,未用过的保持表序
休眠   = load 时 GetPwrCapabilities 探测(S4 + 休眠文件),
         不可用则不出现
破坏性 = 分级:重启/关机走 InitiateSystemShutdownEx 30 秒宽限
         (原生倒计时、shutdown /a 可中止、不强制关应用——应用
         可提示保存/拒绝,是第二道保险);锁屏/睡眠/休眠/注销
         立即执行(可逆或正常会话流程);清空回收站无确认
         (SHERB_NOCONFIRMATION 等三旗标)
特权   = load 时 AdjustTokenPrivileges 启用 SE_SHUTDOWN_NAME
         (交互用户令牌自带、默认禁用);失败仅 Warn,执行时报
         具体错
图标   = SystemIconId 协议新增 7 个动作字形(UI 映射 emoji)
         ——协议 additive,无破坏性变更
明确不做 = 任意命令执行(shell runner 禁区)、模块内第二确认
           对话框(launcher 语义即确认;30 秒宽限是保险)、
           自定义动作配置(V1 固定表)、定时/延时动作
```

## 验证

```text
单测 = 拼音/首字母/英文/别名匹配、空查询全列 + usage 提前、
       usage 同级决胜不压强匹配、休眠能力过滤、item id 稳定、
       present/actions 形状
E2E  = 真机:`>` 列出全部动作(图标/副标题,截图)、`>gj`
       过滤到关机(不执行任何动作)
```
# 127. 游戏模式(全屏时热键不唤起)

前台应用全屏时(游戏、全屏视频),`Alt+Space` 弹窗会抢焦点、
打断沉浸——游戏模式让热键在这种状态下静默失效。

## 决议

```text
定义   = 前台顶层窗口的矩形覆盖其所在显示器的全部物理区域
         (GetWindowRect ⊇ MONITORINFO.rcMonitor,含等于与超出
         ——独占全屏常撑出屏幕边缘几像素,故用覆盖而非相等)
排除   = 桌面 shell 类名白名单(Progman / WorkerW /
         Shell_TrayWnd / Shell_SecondaryTrayWnd):它们可能覆盖
         全屏但不是"全屏应用"
不误判 = 最大化窗口顶到任务栏(rcWork 不含任务栏),矩形盖不满
         rcMonitor——只有真正吃满整块屏幕的才算
探测   = cue-windows::fullscreen::foreground_is_fullscreen();
         GetForegroundWindow → 类名过滤 → GetWindowRect →
         MonitorFromWindow → GetMonitorInfoW。纯查询、无 IO,
         只在热键按下瞬间调用一次(不在轮询循环里)
注入   = CoreConfig.fullscreen_probe(函数,不是 trait——
         同 apply_hotkey / open_path 的 §53/§112 同步例外模式;
         平台代码隔离在 cue-windows,§110 不破)
门控   = Core::hotkey_pressed 的"隐藏→打开"半段:
         core.game_mode 开启且探针为真 → 静默 return。
         只拦热键的唤起半段——隐藏/聚焦半段照常(窗口开着
         总能用同一键关掉),托盘与第二实例唤起不受门控
静默   = 无任何 UI 反馈(用户在游戏里,提示本身就是打扰);
         被吞的键不回注重放(回注会变成半个键盘钩子)
设置   = core.game_mode,Bool,默认 true,Immediate;
         无 try-apply 回调——每次按键时从 SettingsHost 现读
容错   = 探针内任何一步 Win32 失败返回 false(宁可唤起,不错杀)
明确不做 = 手动"游戏模式"开关按钮(设置项即开关)、按进程名/
           路径的应用名单(矩形判据已覆盖真全屏;窗口化游戏
           本就该能唤起)、键回注、轮询监控
```

## 验证

```text
单测 = rect_covers_monitor(等于/超出/最大化/普通窗口/半铺)、
       is_shell_class 白名单;Core 门控:设置开+全屏→静默、
       设置关+全屏→照常唤起、非全屏→照常、可见时热键照常
       关闭、show_requested 不受门控
E2E  = 真机:无边框置顶全屏窗口盖住主屏 → 热键不唤起(截图
       取证);关闭全屏窗口 → 热键恢复唤起;基线(普通前台)
       唤起正常
```
# 128. 触发词自定义(`module.<id>.trigger`)

每个触发模块的触发词(`b` / `/` / `>` …)可在设置页修改——
对标 Flow Launcher 的 per-plugin action keyword。触发词是
**Core 路由状态**,不是模块语义:模块从不解析自己的触发词
(§86),因此它不属于模块 schema。

## 决议

```text
归属   = key 放在模块命名空间下展示(module.<id>.trigger),
         但 spec 由 Core 合成(非模块 schema);Core 用
         trigger_keys 集合登记所有权
生效   = 生效触发词 = 设置值(非空)?? 模块声明值;
         route 每次输入现读 SettingsHost——Immediate,
         无 try-apply 回调、无宿主副作用
事务   = apply_setting_inner 在 Immediate 分支内对
         trigger_keys 做单例处理:trim 归一 + Core 校验,
         跳过 module.try_apply_settings(§42 的 validate →
         commit → persist 链不变)
校验   = 非空(trim 后)、不含空白、≤16 字符、不与其他
         模块的生效触发词冲突;失败不 commit,UI 留在
         编辑态并回显错误
匹配   = 语义不随值变:以字母/数字结尾的触发词要求词边界,
         标点类逐字前缀匹配(match_trigger 按值的尾字符
         自动分流,自定义值天然继承)
默认模块 = App 无触发词(§82),不合成该行
String 编辑 = 设置页第一种行内文本编辑:Enter 进入编辑态
         (buffer 预填当前值),Enter 提交事务(失败留编辑态),
         Esc 放弃;视图本地状态,同热键捕获模式
容错   = 持久化值之间的历史冲突(如新版本引入的模块默认
         触发词撞上用户旧自定义):registry 序先到先赢,
         不 panic、不自动改写用户设置
明确不做 = 空触发词禁用模块(模块启用/禁用是另一个特性)、
           默认模块触发词、大小写归一(保持精确匹配,
           用户设什么就是什么)
```

## 验证

```text
单测 = spec 合成(非默认模块有行、默认模块无行)、自定义
       触发词改路由(旧值不再认领、新值命中、词边界规则
       不变)、校验(空/含空白/超长/冲突全拒、值不被破坏)、
       事务不碰模块 try_apply
E2E  = 真机预写 settings.tsv(module.bookmark.trigger=bm)
       → `bm github` 出书签结果(带 Edge 角标)、
       `b github` 回落应用模块(无结果);用户原设置
       文件备份后还原
```
