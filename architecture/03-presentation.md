> CUE 架构规格分卷。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

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

V1 落地注记：badges 保留在协议里（§13 结构不变），但 V1 渲染层
暂忽略该字段——行渲染只画 title / subtitle / icon / accessory。
第一个真实消费者出现时再定视觉样式，不提前画。

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

