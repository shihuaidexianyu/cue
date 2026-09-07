> sakana 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 140. 锁键服务:CapsLock 手势 + NumLock 守护 + 状态 OSD

锁键服务把两个系统级按键从"误触源"变成"可控手势",并把锁定态
变化做成即时可见的 OSD 卡片。手势引擎移植自 WinCaps
(https://github.com/coekfung/WinCaps,MIT License)——进程内
WH_KEYBOARD_LL 钩子 + 专用线程 + message-only 窗口的线程模型
照搬;判定状态机(`Press`:事件时间戳判定、环绕安全、修饰键/
前台切换取消)与其单测一并移植并扩展。

**架构判定:锁键服务不是 Module。** ModuleRegistry 只收
LauncherModule(query 形态),模块没有任何 CoreEffect / 宿主 UI
出口。它走免打扰模式(§127)同款的 host 路径:sakana-windows
worker + host 消息 + Core 合成 `core.*` 设置行 + commit 后通知
回调。

## 决议

```text
手势(触发键 = CapsLock / ScrollLock 可配)
  门控   = 仅当前台线程键盘布局 PRIMARYLANGID==0x04(中文 IME)
           才拦截;其余前台原样透传(我们自己被 §107 强制英文的
           launcher 天然透传,零冲突)
  轻点   = 阈值内松开且大写关 → 注入输入法切换快捷键
           (Ctrl+Space / Shift 可配);大写开 → 关掉大写
  长按   = 按住 ≥ hold_ms → 切换该键锁定态(大写开时 = 关掉)
  取消   = 修饰键按下 / 前台窗口切换 / 配置变更 / 会话复位
  判时   = 一律事件时间戳(GetTickCount 环绕安全),不靠定时器
           送达顺序;按住自动重复的报文同样吞掉
NumLock(无切输入法语义,三模式可配)
  native    = 不干预
  hold      = 防误触:轻点吞掉(与当前态无关),按住 ≥ hold_ms
              才放行切换
  always_on = 常开守护:吞掉全部 NumLock 按下;任何钩子事件顺带
              巡检,发现为关即注入一次切换打回开(启动时也巡检一次)
OSD    = 状态 diff 上报取代 WinCaps 的 confirm 定时器:钩子回调里
         对 CapsLock/NumLock 做 GetKeyState 逐事件比对,任何来源的
         切换(我们的注入 / 真实按键 / 别的程序)都上报
         host → OSD 卡片;无遗漏,也无专用确认路径
       = 卡片居中于活动显示器(用户明确"屏幕中间",非 Launcher
         的 1/4 高度),900ms 自动收起(generation 去抖),
         SWP_NOACTIVATE 恒不抢焦;Launcher 可见时不弹
配置   = 共享 Arc<Mutex<(LockKeysConfig, epoch)>>:UI 线程写,
         worker 逐事件读;epoch 变化 = 丢弃挂起手势(配置变更 /
         会话复位),全部取消逻辑收在 worker 线程一侧
会话复位 = WM_WTSSESSION 锁屏/断开 + WM_POWERBROADCAST 挂起/唤醒
         → worker.reset()(锁屏/休眠期间按键释放不会送达,挂起
         手势不能等一个永远不来的事件)+ OSD 收起
```

## 设置行(Core 合成,§128 触发词同款:String 行 + Core 侧校验)

```text
core.lockkeys.enabled       Bool   true        总开关(手势/守护/OSD 全停,按键全原生)
core.lockkeys.remap_key     String caps_lock   caps_lock / scroll_lock
core.lockkeys.tap_action    String ctrl_space  ctrl_space / shift
core.lockkeys.hold_ms       String 350         150–1000,触发键与 NumLock 防误触共用
core.lockkeys.numlock_mode  String hold        native / hold / always_on
core.lockkeys.osd           Bool   true        状态变化弹卡片
```

校验唯一入口 = protocol 的 `LockKeysConfig::from_settings`
(allowlist + 范围 + 中文报错),Core 事务的 validate 臂与 commit
后组装共用;持久化文件被手改成非法值时 warn + 回落默认(§128
空触发词同款自愈思路)。下发走 `NotifyLockKeys` 回调(§127
notify_dnd_mode 同款 post-commit 模式):Core::new 初始一次 +
每次锁键行 commit 一次,配置下发不能失败,不参与事务。

## OSD 的两个 GPUI 窗口坑(都已踩平)

1. `find_main_window_hwnd` 按 `Zed::Window` 类名取枚举序第一个
   → 两个 GPUI 窗口会撞。**创建顺序纪律**:先开 Launcher → 发现
   其 HWND → 再开 OSD → 用 `find_window_hwnd_excluding` 排除法
   发现 OSD HWND。
2. GPUI 0.2.2 的 WM_DISPLAYCHANGE 无条件 ShowWindow bug(§115)
   对 OSD 同样成立 → OSD 装同款 display-change guard;原 wndproc
   按 HWND 存映射表(单 static 会被第二次安装覆盖)。
3. OSD 窗口 = `WindowKind::PopUp`(WS_EX_TOOLWINDOW,不进任务栏/
   alt-tab)+ `WindowBackgroundAppearance::Transparent`(圆角外缘
   透明);show/place/hide 全走 Win32,视图(OsdView)纯渲染——
   sakana-ui 不依赖 sakana-windows 的依赖方向不变,事件以
   protocol 的 `LockKey` 纯数据进来。自动收起计时在编排层的
   OSD 泵里(background timer + foreground 任务),不在视图里。

## 复盘:SendInput 同步重入(0.5.0 闪退根因)

**铁律:STATE 的借用绝不跨 SendInput 存活。** win32k 会在注入
线程上同步回调(KiUserCallbackDispatcher → 本线程的 LL 钩子/窗口
过程重入)。0.5.0 的钩子把 SendInput 放在 `STATE.with_borrow_mut`
闭包里执行:第一次真实手势结算时重入钩子再次借 RefCell →
"RefCell already borrowed" panic,panic 点在 `extern "system"`
边界上 = panic_cannot_unwind → abort(WER:0xc0000409 子码 7,
FAST_FAIL_FATAL_APP_EXIT)。调试侧的两个教训:日志写线程在 abort
时丢尾部消息(日志缺尾 ≠ 事件没发生);`strip="symbols"` 发行版
靠链接 map(`-C link-arg=/MAP`)+ minidump 栈扫描即可符号化,
无需 PDB。

修复形态:钩子与 worker 窗口过程一律"借用内只判定(`Decision`),
释放后才执行(send/execute),状态 diff 在注入后再借一次上报"。
连带修正:常开守护巡检必须排除自家注入事件(注入送达时锁定态尚未
翻转,巡检会误判"仍是关"而再注入——同步重入下即无限递归);
启动巡检改由 set_config 投递 `WM_LOCKKEYS_PATROL` 触发(worker 以
默认配置启动,真实配置由初始 notify 下发,直接在 start() 里巡检
永远看到的是默认值,是死代码)。

## 明确不做

- 不移植 WinCaps 的 D2D OSD / 托盘菜单 / 注册表设置存储——我们
  有 GPUI 卡片、设置页与 settings.tsv。
- 暂停/恢复不加托盘菜单项:就是 `core.lockkeys.enabled` 设置行。
- ScrollLock 只做触发键候选,不做它自身的锁定态手势(无需求)。
- 不做自定义 OSD 位置/时延(用户拍板"屏幕中间";900ms 常量)。

## 验证

```text
单测 = 手势状态机(移植 WinCaps tap_and_hold_contract /
     cancellation_and_clock_wrap + NumLock 轻点吞掉双态用例 +
     中文布局判定);protocol from_settings 校验矩阵;Core 侧 6 行
     的校验/事务/commit 通知 + 持久化坏值回落默认
E2E  = 中文 IME 前台:轻点 → 中英切换 + OSD;长按 → 大写 + OSD;
     英文前台:原生 CapsLock;NumLock hold:轻点无效、长按切换 +
     OSD;always_on:被关掉即打回开 + OSD;OSD 在活动显示器正中、
     不抢焦、launcher 打开时不弹;设置页改 6 行(含非法值报错)、
     重启持久;§114 性能预算回归(钩子不得影响唤出 < 100ms)
```
