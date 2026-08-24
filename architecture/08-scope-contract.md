> CUE 架构规格分卷。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

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
- ~~startup support可后置~~（已落地：开机自启 core.start_on_boot §36；
  托盘图标与唯一退出路径 §116）

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
- ~~custom trigger configuration~~（已落地,§128）
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

