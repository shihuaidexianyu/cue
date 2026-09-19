# 142. 第二轮代码审查修正

本节修订 §30、§50、§51、§91、§110、§116、§128、§138、§141 的对应实现约定。

## 退出与窗口生命周期

Launcher 窗口没有"关闭"语义:托盘"退出"是唯一退出路径(§116)。窗口子类化在隐藏态吞 `WM_DISPLAYCHANGE` 之外,一并吞 `WM_CLOSE`。

不吞的后果是进程僵尸化:GPUI 的 `WindowKind::Normal` 带 `WS_SYSMENU` 与任务栏按钮,Alt+F4 / 任务栏关闭走 `DefWindowProc` → 销毁窗口 → `remove_window()` 丢掉 root view,`LauncherView` 与它持有的 `Core` 一起被 drop。事件泵是 `WeakEntity`,视图没了即停;此后热键、托盘、单实例互斥量全部失灵,OSD 窗口又让 GPUI 消息循环继续跑,进程既无 UI 也退不掉(安装包的 `AppMutex` 一并被挡住)。§141 把退出改成经 Core 发 `QuitApplication` effect 后,host 侧不再有独立退出路径,这条链路才变成致命。

退出请求仍走 Core(需要 unload + usage flush),但 host 侧保留兜底:转发失败(接收端已随视图销毁)时直接 `tray::remove` + `restore_saved_layout` + `request_quit`。

## 文件索引查询与首爬

查询级排除名单判定移到 token 命中之后。两个谓词是合取,顺序不影响结果集,复杂度从 O(条目 × 片段) 降到 O(条目 × token + 命中 × 片段)——34 万条目 × 20 片段曾是每键约 170 ms(实测单次查询 212–233 ms,跳过该过滤的 `\` 查询 49 ms)。语义不变:名单变更仍在"下一次查询"生效。

首爬是同步遍历,期间没有别的消费者,4096 事件队列在长首爬里必然溢出,而溢出标记会让首爬刚结束就再来一次全量重爬(实测 20.2 s 首爬后紧跟 9.9 s 重爬,条目数相同、名单文件未变)。现在 `crawl` 每访问一个目录回调一次排水,把首爬窗口的事件收进 batch,爬完后照常合批应用;排水量超过 64 K 才退回全量重扫(内存有界,正确性优先)。

## watcher 生命周期

单根就绪握手有 5 s 上限:坏根(无响应 UNC / 休眠 NAS)的 `CreateFileW` 不得让首爬与退出无限期卡住。打开失败走断开连接,不占超时。

退出时的收尾 join 有 2 s 上限,超时即 detach。watcher 卡在 `CancelIoEx` + `GetOverlappedResult(bWait=TRUE)` 上时,泄漏一个线程好过 UI 永久假死;缓冲区与 `OVERLAPPED` 的所有权约束不变(取消后必须等到完成才能释放)。

## 剪贴板测试

真实剪贴板集成测试改用**匿名** window station。只有提权到 Administrators 的令牌才允许给 window station 命名,传名字在普通 shell 里直接 ACCESS_DENIED——§141 记录里"部分 Windows 环境禁止创建 window station"是误诊:环境允许创建,只是不允许命名。desktop 保留:线程的 desktop 必须属于进程的 window station,这是连通性前提,不是隔离手段(desktop 名字没有管理员限制)。

受限环境(沙箱 / 非交互会话)里整个会话都拿不到剪贴板——本机实测连交互式 station 的 `OpenClipboard(NULL)` 与 PowerShell `Get-Clipboard` 都返回 ACCESS_DENIED。该用例默认 `#[ignore]`,需要在普通交互会话里跑才算验证。

## 设置与路由

`decode_value(Bool)` 只认 `"true"` / `"false"`,其余返回 None 回落规格默认值。此前"非 true 一律 false"让手工改坏的值静默变成关闭(§141 的"非法持久化值逐项回落"对 Bool 不成立)。

触发词同长并列(只有手工/历史持久化冲突能造出)按注册序先到先赢,与 §128 容错条款一致。`max_by_key` 在并列时取最后一个,方向相反,改用 `reduce` 保留首个最大项。

## OSD 与护栏

`ShowLauncher` 顺带收起锁键 OSD。抑制判定只在 host 发送时做,已显示的卡片不会自己消失——否则 launcher 隐藏后它会闪出来。

`check-arch.ps1` 的源码扫描优先 ripgrep,没有则退回 `git grep`,不再硬依赖 `rg`。

## 记录在案的取舍

- 退出瞬间仍在飞行中的激活:其 `ActivationCompleted` 不会在 `QuitApplication` 之后被处理(事件泵随消息循环结束),usage 记录丢失。要消除需要"退出前等待在飞工作",与 §91(不取消异步)冲突,不在本次范围。
- `UsageStore` 的"容量 1 合并"测试只证明队列有界、`record` 不阻塞,没有证明 N 次 record 产生少于 N 次落盘(未加测试期计数器)。
