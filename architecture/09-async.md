> CUE 架构规格分卷。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

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

