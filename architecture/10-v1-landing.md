> CUE 架构规格分卷。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

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

托盘"退出"（§116）是另一条绕过 hide 的路径：quit 分支在结束
消息循环前同样执行恢复——可见状态下退出时不恢复，后果与
漏掉 hide 恢复相同（其他应用被留在英文）。
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
文字起点（横向）永不移动
行高固定，icon 在槽位内垂直居中
文字列作为一个整体在行内垂直居中：subtitle 为 None 时不渲染
  第二行（不是留一行空白），title 随块居中——行高不变、
  无 reflow、无跳动的结论不受影响
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
items 与当前结果无交集 → 忽略
有交集 → 对当前结果重跑 present()
```

模块不精确追踪可见行：允许广播式失效（items 里混着已滚走、
上一代查询的陈旧 id），交集判定由 Core 一侧做。事件不影响
generation，不触发重新 query。

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

FileModule：Windows 依赖 Everything（§31 定案）
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

同步例外（函数注入，不是 HostPlatform trait，§110）——Core 持有注入回调在 UI 线程直接同步调用：

```text
apply_hotkey / apply_start_on_boot
  core.* 设置事务的 try-apply（§42，失败不 commit）
open_path
  Path 行的"打开路径"激活（§122——非值变更，不走事务）
fullscreen_probe
  免打扰模式门控（§127——按键瞬间的纯查询，无 IO、无锁）
notify_dnd_mode
  core.dnd_mode 的 commit 后通知(§127——托盘状态图标;
  通知不是 try-apply,无返回值,不参与事务成败)
```

UIEvent 不经 launcher 逐条翻译：view 持有 Core，按键在 cue-ui 内直接调 Core 方法；launcher 只注入 CoreEffect 的执行器（effect_handler）。上图的"接收并翻译"覆盖的是 HostEvent（WM_HOTKEY / 托盘 / 失焦 → CoreEvent 队列）。

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

第二实例的 show / focus 与托盘左键唤起（§116）汇合为同一个 ShowRequested 事件——多条唤起路径，一份处理。

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

> 当前口径：hotkey latency 以预热后的 22/15 ms 为准；上段
> 92–98 ms 保留为预热落地前的历史测量。

**启动序列重构（2026-08-24，§131）**：host window 与热键注册
抢在 GPUI 初始化之前完成，release 启用 strip + thin LTO +
codegen-units=1（exe 12 MB → 8.4 MB）。复测（release 构建,
stderr 探针）：**热键就绪 3–5 ms**（GPUI 初始化期间按下的键经
backlog 补发，不再被吞）、GPUI 就绪 117–132 ms、Core 就绪
121–137 ms、窗口创建 139–159 ms。上段 Cold start 113–125 ms
保留为重构前的历史测量（当时探针在 GPUI 初始化之后）。

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

增补（2026-08-24，失焦定义补全）：锁屏 / 快速用户切换离开控制台也
视为失焦。锁屏切到安全桌面时 `EVENT_SYSTEM_FOREGROUND` 是否投递给
out-of-context 钩子是未文档行为——实测 Win11 26200 显式锁屏有
LockApp 前台事件（钩子能藏），但随构建与锁屏路径（如灭屏自动锁）
而异，不能只依赖它。宿主注册 `WM_WTSSESSION`（WTS_SESSION_LOCK /
WTS_CONSOLE_DISCONNECT，文档化通道）补投 FocusLost，与前台事件
互补、重复无害（visible=false 时 no-op），是否隐藏仍由
`core.hide_on_focus_loss` 裁决；解锁不自动唤起。

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

不为托盘做更多：不弹气泡通知、不做双击行为、~~不做开机启动开关
（开机启动是 Settings 候选，Phase 6 评估）~~（开机自启已落地为
设置 core.start_on_boot（§36）——在设置页，不在托盘菜单）。

托盘回调消息投递到 host window（§113 的 Win32 消息入口）；
host window 因此是隐藏顶层窗口而非 message-only——托盘菜单要求
owner 可设为前台，message-only 窗口做不到（MSDN 托盘菜单模式）。

图标同时是免打扰的状态灯(§127,2026-08-24 增补):红 = 我在
工作(热键可用),灰 = 免打扰生效(热键被压制)。前台切换钩子
上重估、翻转才 NIM_MODIFY——见 §127 的「托盘」决议。

---

