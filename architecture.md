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
> 原因：对 Everything 第三方依赖的策略未最终确认。
> 本章及 §32、§33 的设计保留，作为 V1.x 的实现依据。

V1.x 计划使用：

```text
Everything
```

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

冷启动回退：图标提取已异步（§109），load 不含图标。
若实测 load P95 仍无法满足 §77 的 < 500 ms，
V1 退回"加载缓存 catalog → 后台刷新"；
在此之前保持同步实现，不提前建 background indexer。
```

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

- File Module 整体（`/` trigger、Everything 集成、文件/文件夹打开；设计保留于 §31–33）
- App Paths
- PATH executable discovery
- aliases UI
- Run as administrator
- Open containing folder
- secondary actions UI
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

> v0.1 修订：本阶段整体延后出 V1（见 §31）。V1 范围为 Phase 1–4 与 Phase 6。

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

# 89. V1 成功标准

当以下体验稳定时，可以认为 V1 产品成立：

```text
Alt+Space
z
Enter
```

能够可靠快速启动用户想要的应用。

（文件模态的验收随 FileModule 延后至 V1.x，见 §31。）

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
icon 槽位永远占固定宽度（如 32px），None 即留空
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

唯一的同步例外：§53 热键的 try-apply 是设置事务的一部分，必须同步——Core 持有注入的 `apply_hotkey` 回调直接调用，失败不 commit（§42）。这是一个函数，不是 HostPlatform trait（§110）。

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
右键菜单  → 显示 / 退出
退出      → 删除托盘图标（不留幽灵图标）、注销热键、进程退出
```

不为托盘做更多：不弹气泡通知、不做双击行为、不做开机启动开关
（开机启动是 Settings 候选，Phase 6 评估）。

托盘回调消息投递到 host window（§113 的 Win32 消息入口）；
host window 因此是隐藏顶层窗口而非 message-only——托盘菜单要求
owner 可设为前台，message-only 窗口做不到（MSDN 托盘菜单模式）。