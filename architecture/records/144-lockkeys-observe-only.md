> sakana 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 144. 锁键服务裁为纯观察:手势/拦截/注入全部删除

§140 的手势引擎(轻点切输入法、长按切锁定)与 NumLock 三模式
(hold 防误触 / always_on 守护)建立在"吞键 + 注入"之上。实机
排障(2026-09,HP 自带键盘)证明这条路在 Windows 上不可靠,
**全部砍掉,只保留纯观察的锁定状态 OSD**。§140 记录保留作历史;
本文档取代其手势/NumLock 模式/设置行部分,OSD 窗口的工程结论
(两窗口坑、SWP_NOACTIVATE、generation 去抖、launcher 可见抑制)
继续有效。

## 为什么不可修(三连根因,逐层拆穿)

1. **系统在进 LL 钩子之前就翻转 toggle**(PowerToys
   KeyboardManager 注释同款发现):吞掉消息只挡传递,挡不住翻转
   ——拦截 NumLock 后按一下照样翻。
2. **回滚注入(PowerToys 解法)撞上 make/repeat 语义**:toggle
   只在按下沿翻转;用户按住键时真实按下在 win32k 内部留有 down
   态,手势结算时注入的"按下"被当成重复报文,一次都不翻——
   OSD 报出目标态,真状态原地不动(卡片与实际相反)。
3. **被吞的键从不投递,toggle 又是 per-线程队列的**:本进程的
   GetKeyState / GetAsyncKeyState 读数与前台真态永久脱钩,逐事件
   状态 diff 只会把结算出的正确状态盖回陈旧读数(盖卡 bug)。

三层可以两两组合出"看起来能跑"的修补,但每一层都把不一致推给
下一层;两天内连续四个形态(不拦 → 拦但卡片反 → 拦且卡片被盖 →
拦且状态不翻)后确认:**这是平台行为的固有冲突,不是实现 bug,
不为它擦屁股。**

## 裁后形态

```text
锁键服务 = 纯观察 OSD
  钩子   = WH_KEYBOARD_LL 照旧,但永不吞键、永不注入、无定时器
  状态   = 按下沿记账(Ledger):我们不吞任何键,锁键的每个真实
           按下沿必然翻转前台 toggle,钩子见到按下沿就把账本翻
           一次并上报 host——计数永远为真,不读 API
  初始化 = 启动时 GetAsyncKeyState 读一次账本初值(进程没跑就没
           有吞,历史按键全是投递过的,读数可信)
  会话复位 = 锁屏/挂起可能吞掉释放事件 → 清按下沿标记 + 按当前
           读数重同步账本(HostMsg::SessionReset → WM_LOCKKEYS_RESET)
  盲区   = 别的程序 SetKeyboardState 改自己线程的队列态:无事件
           可捕,影响面本来也只有那个程序——接受,不追
OSD      = 窗口/去抖/抑制全部沿用 §140,不变
```

## 设置行(裁后)

```text
core.lockkeys.enabled  Bool  true   总开关(纯观察,不拦截不注入)
core.lockkeys.osd      Bool  true   状态变化弹卡片
```

删掉的四行(remap_key / tap_action / hold_ms / numlock_mode)留在
旧 settings.tsv 里无害:无对应 spec 的行加载时忽略,下次整体重写
自愈消失。布尔行无校验面,protocol 的 `LockKeysConfig::from_settings`
与 `LockKeysConfigError` 一并删除;commit 后 `NotifyLockKeys` 全量
通知的模式不变。

## 验证

```text
单测 = Ledger 按下沿契约(沿翻一次 / 重复不翻 / 抬起后再按才再翻 /
     缺抬起的按下按重复处理);Core 侧两行的通知与坏候选拒绝;
     陈旧设置键(hold_ms 等)加载忽略
E2E  = 任意前台按 CapsLock / NumLock:键本身原生生效(数字键盘
     可用性随之切换)+ 卡片内容与实际一致;长按/连按不错账;
     launcher 可见时不弹;锁屏解锁后首按卡片仍正确
```
