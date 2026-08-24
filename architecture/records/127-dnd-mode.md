> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 127. 免打扰模式(全屏时热键不唤起)

> 原名「游戏模式」,设置 key `core.game_mode`;2026-08-24 更名为
> 免打扰模式 / `core.dnd_mode`(旧 key 读入时自动迁移,§42 设置宿主
> read_persisted 做别名,下次整体重写自愈)。

前台应用全屏时(游戏、全屏视频),`Alt+Space` 弹窗会抢焦点、
打断沉浸——免打扰模式让热键在这种状态下静默失效。

## 决议

```text
定义   = 前台顶层窗口的矩形覆盖其所在显示器的全部物理区域
         (GetWindowRect ⊇ MONITORINFO.rcMonitor,含等于与超出
         ——独占全屏常撑出屏幕边缘几像素,故用覆盖而非相等)
排除   = 桌面 shell 类名白名单(Progman / WorkerW /
         Shell_TrayWnd / Shell_SecondaryTrayWnd):它们可能覆盖
         全屏但不是"全屏应用"
不误判 = 最大化窗口顶到任务栏(rcWork 不含任务栏),矩形盖不满
         rcMonitor——只有真正吃满整块屏幕的才算
探测   = cue-windows::fullscreen::foreground_is_fullscreen();
         GetForegroundWindow → 类名过滤 → GetWindowRect →
         MonitorFromWindow → GetMonitorInfoW。纯查询、无 IO,
         只在热键按下瞬间调用一次(不在轮询循环里)
注入   = CoreConfig.fullscreen_probe(函数,不是 trait——
         同 apply_hotkey / open_path 的 §53/§112 同步例外模式;
         平台代码隔离在 cue-windows,§110 不破)
门控   = Core::hotkey_pressed 的"隐藏→打开"半段:
         core.dnd_mode 开启且探针为真 → 静默 return。
         只拦热键的唤起半段——隐藏/聚焦半段照常(窗口开着
         总能用同一键关掉),托盘与第二实例唤起不受门控
静默   = 无任何 UI 反馈(用户在游戏里,提示本身就是打扰);
         被吞的键不回注重放(回注会变成半个键盘钩子)
设置   = core.dnd_mode,Bool,默认 true,Immediate;
         无 try-apply 回调——每次按键时从 SettingsHost 现读
容错   = 探针内任何一步 Win32 失败返回 false(宁可唤起,不错杀)
明确不做 = 手动"免打扰"开关按钮(设置项即开关)、按进程名/
           路径的应用名单(矩形判据已覆盖真全屏;窗口化游戏
           本就该能唤起)、键回注、轮询监控
```

## 验证

```text
单测 = rect_covers_monitor(等于/超出/最大化/普通窗口/半铺)、
       is_shell_class 白名单;Core 门控:设置开+全屏→静默、
       设置关+全屏→照常唤起、非全屏→照常、可见时热键照常
       关闭、show_requested 不受门控
E2E  = 真机:无边框置顶全屏窗口盖住主屏 → 热键不唤起(截图
       取证);关闭全屏窗口 → 热键恢复唤起;基线(普通前台)
       唤起正常
```

---


## 增补(2026-08-24):托盘状态图标

更名免打扰的同时重做了图标语义:**红 = 我在工作(热键可用),
灰 = 免打扰生效(热键被压制)**——按下热键没反应时,托盘一眼
给出原因;灰化表"被压制"与静音/禁用态的约定一致。品牌主图标
(资源 id 1,exe / 安装器 / Alt-Tab / 托盘平时)换成红火箭
(#E5484D),火箭一并 1.20× 放大(16px 可读性);灰色版
(资源 id 2,锌灰 #52525B)仅托盘运行时 NIM_MODIFY 切换。

```text
状态合成 = host 层:DND_ENABLED(Core 的 notify_dnd_mode 回调
           镜像,初始一次 + 每次成功 commit)&& 前台全屏探针
重估时机 = 前台切换 WinEvent 钩子——每次前台变化都重估(纯
           查询)。与热键门控"只在按键瞬间探测"互补:门控是
           瞬判,图标是状态,必须随状态连续;仍无轮询
换图标   = 状态翻转才 NIM_MODIFY;CUE 启动时全屏已在前台,
           由 tray add 读生效态挑初始图标
设置翻转 = 全屏前台时关掉免打扰 → 图标当场回红(notify 回调
           里同步重估,不等下次前台切换)
资产     = assets/cue.svg(红)= 品牌;assets/cue-dnd.svg(灰)
           = 生效态;icon.ps1 各生成多尺寸 ICO,cue.rc id 1/2
```

验证:单测 = notify 回调初始一次、commit 触发、失败事务与其他
key 不触发(外加更名迁移:旧 key 映射、并存新 key 赢);E2E
真机 = 启动日志 enabled=true;全屏窗口前台 → engaged true
(转灰);关闭 → engaged false(回红)。
