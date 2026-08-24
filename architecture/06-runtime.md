> CUE 架构规格分卷。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

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

用单调递增的 HOTKEY_ID 做事务式替换（实现定型，替代本节
原稿的 ACTIVE / STAGING 双 id 角色互换——语义相同，无需
角色状态）：新组合在新 id 注册成功，再注销旧 id 的旧注册
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
- 置顶（HWND_TOPMOST，随唤起时的放置设置，§130）：launcher 语义
  是覆盖在当前工作之上的一层，不该被普通窗口遮住

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

