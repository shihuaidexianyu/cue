> CUE 架构规格分卷。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

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

编辑面的后续落地（仍无表单 framework）：String 行内编辑随
§128 的触发词设置落地（Enter 进编辑态、Enter 提交、Esc 放弃）；
Path 行随 §122 落地，其 Enter = 用系统默认程序打开路径值，
不是值变更、不走 §42 事务。
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

