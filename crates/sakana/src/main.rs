// release 用 GUI 子系统:双击/开始菜单/自启不再给进程挂控制台窗口。
// debug 保留控制台(cargo run 可见 [boot]/[perf] 探针);release 下
// 探针仍可由父进程重定向 stderr 捕获(E2E 正是这么测的)。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! sakana —— Launcher 可执行文件,唯一的 composition root。
//!
//! 编排:HostEvent → Core → CoreEffect → sakana-ui / sakana-windows。
//! 只有本 crate 同时认识 Core、GPUI 和 Win32。

use futures::StreamExt;
use futures::channel::mpsc;
use futures::future::BoxFuture;
use gpui::*;
use sakana_core::{
    Core, CoreConfig, CoreEffect, CoreEvent, CoreEventSender, HostEvent, ModuleRegistry,
    TaskSpawner,
};
use sakana_module_app::AppModule;
use sakana_module_bookmark::BookmarkModule;
use sakana_module_file::FileModule;
use sakana_module_system::SystemModule;
use sakana_protocol::logln;
use sakana_protocol::{Hotkey, LockKey, LockKeysConfig};
use sakana_ui::{LauncherView, OsdView};
use sakana_windows as win;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

/// 生产环境 TaskSpawner,把 Core 的异步工作挂到 GPUI 后台线程池。
struct GpuiSpawner {
    executor: BackgroundExecutor,
}

impl TaskSpawner for GpuiSpawner {
    fn spawn(&self, fut: BoxFuture<'static, ()>) {
        // 北极星:Core 不做物理取消——spawn 即移交,detach 后
        // 结果有效性由 SessionId / ModuleEpoch / Generation 判定。
        self.executor.spawn(fut).detach();
    }
}

const WINDOW_WIDTH: i32 = 640;
const WINDOW_HEIGHT: i32 = 450;
/// 锁键状态 OSD(§140):活动显示器正中的提示卡片尺寸(逻辑像素)。
const OSD_WIDTH: i32 = 220;
const OSD_HEIGHT: i32 = 64;
/// OSD 自动收起时延;新事件/会话复位重置计时(generation 去抖)。
const OSD_HIDE_AFTER: std::time::Duration = std::time::Duration::from_millis(900);

/// host handler → OSD 泵的消息(§140)。不进 Core——OSD 是 host
/// 侧呈现,与搜索会话无关。
#[derive(Clone, Copy, Debug)]
enum OsdMsg {
    State(LockKey, bool),
    Reset,
}

/// 开发期热键覆盖:`SAKANA_HOTKEY="ctrl+alt+k"`(格式同设置值)。
/// 只影响本次进程的初始注册,不写入 settings.tsv——调试覆盖不应
/// 变成持久配置。正式修改走托盘 → 设置。
fn parse_hotkey_env() -> Option<Hotkey> {
    std::env::var("SAKANA_HOTKEY").ok()?.parse().ok()
}

/// HostMsg → CoreEvent 的翻译。退出先经 Core 停止模块并 flush usage;
/// LockKeyChanged / SessionReset 由 handler 处理(OSD 与会话复位)。
fn to_core_event(msg: win::host::HostMsg) -> CoreEvent {
    let event = match msg {
        win::host::HostMsg::HotkeyPressed => HostEvent::HotkeyPressed,
        win::host::HostMsg::ShowRequested => HostEvent::ShowRequested,
        win::host::HostMsg::OpenSettings => HostEvent::OpenSettings,
        win::host::HostMsg::FocusLost => HostEvent::FocusLost,
        win::host::HostMsg::FocusGained => HostEvent::FocusGained,
        win::host::HostMsg::QuitRequested => HostEvent::QuitRequested,
        win::host::HostMsg::LockKeyChanged { .. } => unreachable!("OSD 事件在 handler 内拦截"),
        win::host::HostMsg::SessionReset => unreachable!("会话复位在 handler 内拦截"),
    };
    CoreEvent::Host(event)
}

/// §139 改名迁移:旧 CUE 存储目录 → sakana。同卷 rename 原子完成,
/// settings/usage/模块数据与图标缓存整体随迁。两边都在 = 用户跑过
/// 新版又装了旧版:保留新目录,旧目录留着不动(里面仍是用户数据)。
fn migrate_storage_root() -> PathBuf {
    let root = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    let new = root.join("sakana");
    let old = root.join("CUE");
    if !new.exists()
        && old.is_dir()
        && let Err(e) = std::fs::rename(&old, &new)
    {
        eprintln!("[migrate] rename {old:?} -> {new:?} failed: {e}");
    }
    new
}

fn main() {
    let boot_started = std::time::Instant::now();
    let storage_root = migrate_storage_root();
    // 诊断日志(§132)在最早时机落地:之后的 [boot]/[host] 行都进文件。
    // 写线程承载全部文件 IO;打开失败退回纯 stderr,不影响启动。
    sakana_protocol::log::init(&storage_root.join(sakana_protocol::log::LOG_FILE_NAME));
    // 单实例必须在最早时机——任何状态文件被打开之前。
    // 第二实例:signal_first_instance 已在 acquire 内完成,直接退出。
    let single_instance = match win::single_instance::acquire() {
        win::single_instance::AcquireOutcome::Primary(guard) => guard,
        win::single_instance::AcquireOutcome::AlreadyRunning => return,
    };

    // §139 改名迁移:HKCU Run 的旧 `CUE` 自启值换名接管(指向当前
    // exe 新路径)。失败只记日志,不阻断启动——下次启动会重试。
    if let Ok(exe) = std::env::current_exe()
        && let Err(e) = win::autostart::migrate_legacy_entry(&exe)
    {
        logln!("[migrate] autostart legacy entry: {e}");
    }

    // 热键尽早注册(启动序列的核心设计):host window 与热键都是纯
    // Win32,不依赖 GPUI,抢在 Application 初始化之前完成——进程入口
    // 后 ~10 ms 热键即生效。GPUI 起来之前按下的热键进线程消息队列,
    // 主循环启动时分发到 handler;Core 未就位则由 backlog 暂存,
    // Core::new 后原序补发。开机/安装后立刻按键不再被吞。
    let core_tx_slot: Rc<RefCell<Option<CoreEventSender>>> = Rc::new(RefCell::new(None));
    let backlog: Rc<RefCell<Vec<win::host::HostMsg>>> = Rc::new(RefCell::new(Vec::new()));
    // 锁键服务(§140)的接线点:worker 槽(host 创建后起步)、OSD
    // 事件通道(GPUI 起来后装发送端)、launcher 可见性(OSD 抑制:
    // 用户正看着 launcher 时锁键卡片纯属打扰)。都在 UI 线程访问。
    let lockkeys_slot: Rc<RefCell<Option<win::lockkeys::Worker>>> = Rc::new(RefCell::new(None));
    let osd_tx_slot: Rc<RefCell<Option<mpsc::UnboundedSender<OsdMsg>>>> =
        Rc::new(RefCell::new(None));
    let launcher_visible = Rc::new(Cell::new(false));
    let host = {
        let slot = Rc::clone(&core_tx_slot);
        let backlog = Rc::clone(&backlog);
        let lockkeys = Rc::clone(&lockkeys_slot);
        let osd_tx = Rc::clone(&osd_tx_slot);
        let visible = Rc::clone(&launcher_visible);
        win::host::HostWindow::create(Box::new(move |msg| {
            logln!("[host] {msg:?}");
            match msg {
                // §140:OSD 事件不进 Core;launcher 可见时不弹。通道
                // 未就位(GPUI 起来前的瞬态)直接丢弃——那时的状态
                // 提示没有价值。
                win::host::HostMsg::LockKeyChanged { key, on } => {
                    if !visible.get()
                        && let Some(tx) = osd_tx.borrow().as_ref()
                    {
                        let _ = tx.unbounded_send(OsdMsg::State(key, on));
                    }
                }
                // 锁屏/休眠唤醒(§140):worker 丢弃挂起手势(锁屏期间
                // 按键的释放事件不会送达),OSD 收起。
                win::host::HostMsg::SessionReset => {
                    if let Some(worker) = lockkeys.borrow().as_ref() {
                        worker.reset();
                    }
                    if let Some(tx) = osd_tx.borrow().as_ref() {
                        let _ = tx.unbounded_send(OsdMsg::Reset);
                    }
                }
                msg => match slot.borrow().as_ref() {
                    Some(tx) => {
                        let _ = tx.unbounded_send(to_core_event(msg));
                    }
                    None => backlog.borrow_mut().push(msg),
                },
            }
        }))
        .expect("host window")
    };

    // 锁键 worker 与热键同批起步,先于 GPUI——手势与守护从登录起
    // 即生效。持久化配置由 Core 就位后的初始 notify 校准(此前按
    // protocol 默认运行,与热键初始注册同款瞬态)。启动失败降级为
    // 警告:锁键服务缺席不影响 Launcher 本体。
    *lockkeys_slot.borrow_mut() =
        match win::lockkeys::Worker::start(host.hwnd(), LockKeysConfig::default()) {
            Ok(worker) => Some(worker),
            Err(e) => {
                logln!("[warn] lockkeys worker start failed: {e}");
                None
            }
        };

    // 初始注册用 env 覆盖(仅本次进程)或默认 Alt+Space;设置里的
    // 自定义值要等 Core 读了 settings.tsv 才知道,Core 就位后再次
    // apply(相同则 apply 早退;不同则事务式换绑,这 ~100 ms 内
    // 默认键暂可用,是可接受的瞬态)。
    let hotkey_slot: Rc<RefCell<Option<win::hotkey::HotkeyManager>>> = Rc::new(RefCell::new(None));
    *hotkey_slot.borrow_mut() = Some(win::hotkey::HotkeyManager::new(host.hwnd()));
    let registered = hotkey_slot
        .borrow_mut()
        .as_mut()
        .expect("hotkey manager just installed")
        .apply(parse_hotkey_env().unwrap_or_default());
    // 热键被其他应用(如另一个 launcher)占用时降级为警告:
    // Launcher 继续运行,可经第二实例信号唤起,设置里可换键。
    if let Err(e) = registered {
        logln!("[warn] hotkey registration failed: {e}");
    }
    // 冷启动预算(< 500 ms)的常驻探针:进程入口 → 热键就绪。
    logln!("[boot] hotkey ready in {:?}", boot_started.elapsed());

    Application::new().run(move |cx: &mut App| {
        logln!("[boot] gpui entered in {:?}", boot_started.elapsed());
        let spawner = Arc::new(GpuiSpawner {
            executor: cx.background_executor().clone(),
        });

        let mut registry = ModuleRegistry::new();
        // AppModule 是 V1 的 required default module。
        registry
            .register(Box::new(AppModule::new()))
            .expect("register app module");
        // BookmarkModule,触发词 `b`(词边界规则)。
        registry
            .register(Box::new(BookmarkModule::new()))
            .expect("register bookmark module");
        // FileModule,触发词 `/`(自建索引,§138)。
        registry
            .register(Box::new(FileModule::new()))
            .expect("register file module");
        // SystemModule,触发词 `>`(固定系统动作,§126)。
        registry
            .register(Box::new(SystemModule::new()))
            .expect("register system module");

        let storage_root = storage_root.clone();

        // apply_hotkey 回调与初始注册共用同一个 HotkeyManager——
        // manager 在 main 里已创建并入槽(host window 早于 GPUI);
        // 共享槽只在 UI 线程访问。
        let apply_hotkey = {
            let slot = Rc::clone(&hotkey_slot);
            move |hk: &Hotkey| -> Result<(), String> {
                match slot.borrow_mut().as_mut() {
                    Some(m) => {
                        let r = m.apply(*hk).map_err(|e| e.to_string());
                        logln!(
                            "[hotkey] try-apply {hk} -> {}",
                            if r.is_ok() { "ok" } else { "failed" }
                        );
                        r
                    }
                    // manager 未就位只可能发生在窗口创建前,而设置 UI
                    // 那时还不存在——真走到这里说明接线坏了,宁可事务
                    // 失败也不静默提交一个未注册的值。
                    None => Err("hotkey manager not installed".to_string()),
                }
            }
        };

        // core.start_on_boot 的 try-apply:写当前 exe 到 HKCU Run 键。
        // exe 路径启动时解析一次;解析失败则事务恒失败(不 commit)。
        let apply_start_on_boot = {
            let exe = std::env::current_exe().ok();
            move |on: bool| -> Result<(), String> {
                match &exe {
                    Some(path) => win::autostart::set_enabled(on, path),
                    None => Err("cannot resolve current exe path".to_string()),
                }
            }
        };

        // Path 类设置行的"打开":explorer 拉起文件的默认关联
        // (.txt → 用户默认编辑器)。explorer 返回码不可靠(成功也常
        // 非零),只认 spawn 失败;GUI 子进程,无控制台闪烁。
        let open_path = |path: &std::path::Path| -> Result<(), String> {
            std::process::Command::new("explorer")
                .arg(path)
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("打开失败:{e}"))
        };

        // core.lockkeys.* 的 commit 后通知(§140):全量配置下发
        // worker(初始一次 + 每次锁键行 commit)。worker 缺席(启动
        // 失败)时丢弃——设置照常持久化,下次启动生效。
        let notify_lockkeys = {
            let slot = Rc::clone(&lockkeys_slot);
            move |config: LockKeysConfig| {
                logln!("[lockkeys] config -> {config:?}");
                if let Some(worker) = slot.borrow().as_ref() {
                    worker.set_config(config);
                }
            }
        };

        let core = Core::new(
            CoreConfig {
                usage_file: Some(storage_root.join("usage.tsv")),
                settings_file: Some(storage_root.join("settings.tsv")),
                apply_hotkey: Some(Box::new(apply_hotkey)),
                apply_start_on_boot: Some(Box::new(apply_start_on_boot)),
                open_path: Some(Box::new(open_path)),
                // 免打扰模式(§127):全屏探针——UI 线程热键路径上的
                // 几次便宜 Win32 查询,无 IO、无锁,同步注入。
                fullscreen_probe: Some(Box::new(win::fullscreen::foreground_is_fullscreen)),
                // 托盘状态图标(§127):Core 告知 dnd 开关的初始值与每次
                // commit;host 侧合成"开关 && 前台全屏"决定红/灰。
                notify_dnd_mode: Some(Box::new(|on| {
                    logln!("[dnd] enabled={on}");
                    win::host::set_dnd_enabled(on);
                })),
                notify_lockkeys: Some(Box::new(notify_lockkeys)),
                storage_root,
                ..CoreConfig::default()
            },
            registry,
            spawner,
        )
        .expect("core init");
        logln!("[boot] core ready in {:?}", boot_started.elapsed());
        let core_tx = core.event_sender();

        // Core 就位:backlog 里攒下的早期消息(启动后抢先按的热键/
        // 第二实例唤起)原序补发,再装上发送端。handler 与本段同在
        // UI 线程,不存在交错。
        {
            let mut pending = backlog.borrow_mut();
            for msg in pending.drain(..) {
                let _ = core_tx.unbounded_send(to_core_event(msg));
            }
            *core_tx_slot.borrow_mut() = Some(core_tx);
        }

        // 托盘图标是进程存活的唯一常驻可见信号。
        win::tray::add(host.hwnd()).expect("tray icon");

        // 设置里的自定义热键此时才读到:与早期注册(默认/env)不同
        // 则事务式换绑;相同则 apply 早退,零成本。
        let registered = hotkey_slot
            .borrow_mut()
            .as_mut()
            .expect("hotkey manager installed in main")
            .apply(parse_hotkey_env().unwrap_or_else(|| core.hotkey()));
        // 热键被其他应用(如另一个 launcher)占用时降级为警告:
        // Launcher 继续运行,可经第二实例信号唤起,设置里可换键。
        if let Err(e) = registered {
            logln!("[warn] hotkey re-apply from settings failed: {e}");
        }

        // 常驻但默认隐藏——窗口创建时不可见,等待 ShowLauncher 效果。
        let bounds = Bounds::centered(
            None,
            size(px(WINDOW_WIDTH as f32), px(WINDOW_HEIGHT as f32)),
            cx,
        );
        let window_handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: None,
                    show: false,
                    focus: false,
                    is_resizable: false,
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| LauncherView::new(core, cx)),
            )
            .expect("open window");
        logln!("[boot] window created in {:?}", boot_started.elapsed());

        cx.activate(true);

        // HWND 发现(按进程枚举,避开 GPUI 内部 API 漂移)。
        let hwnd = win::window::find_main_window_hwnd().expect("launcher hwnd");
        win::host::set_launcher_hwnd(hwnd);
        win::window::set_brand_icon(hwnd); // alt-tab / 任务栏图标(资源 id 1)
        // GPUI 在"记录的显示器断开"时会无条件 ShowWindow 隐藏窗口
        // (多屏变单屏即误唤醒);隐藏状态下吞掉 WM_DISPLAYCHANGE。
        win::window::install_display_change_guard(hwnd);
        let focus_hook = win::host::install_focus_hook().expect("focus hook");

        // 锁键状态 OSD 窗口(§140):第二个 GPUI 窗口。**顺序敏感**:
        // find_main_window_hwnd 按类名取枚举序第一个 Zed::Window——
        // 必须先完成上面的 launcher hwnd 发现再创建 OSD,OSD 自己的
        // hwnd 用排除法发现。PopUp = WS_EX_TOOLWINDOW(不进任务栏/
        // alt-tab);Transparent 让圆角卡片外缘透明。
        let osd_handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(OSD_WIDTH as f32), px(OSD_HEIGHT as f32)),
                        cx,
                    ))),
                    titlebar: None,
                    show: false,
                    focus: false,
                    is_movable: false,
                    is_resizable: false,
                    kind: WindowKind::PopUp,
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |_, cx| cx.new(|_cx| OsdView::new()),
            )
            .expect("open osd window");
        let osd_hwnd = win::window::find_window_hwnd_excluding(hwnd).expect("osd hwnd");
        // GPUI 的 WM_DISPLAYCHANGE 无条件 ShowWindow bug 对 OSD 同样
        // 成立(隐藏时被误唤醒就是一张常驻卡片),装同款护栏。
        win::window::install_display_change_guard(osd_hwnd);

        // OSD 泵:show/place/hide 全走 Win32(place_centered 恒
        // SWP_NOACTIVATE,永不抢焦),视图只渲染当前状态;自动收起用
        // generation 计数去抖——新事件/会话复位 bump 代次,旧计时
        // 到点发现代次已变即作废。
        {
            let (osd_tx, mut osd_rx) = mpsc::unbounded::<OsdMsg>();
            *osd_tx_slot.borrow_mut() = Some(osd_tx);
            let osd_gen = Rc::new(Cell::new(0u64));
            cx.spawn(async move |cx: &mut AsyncApp| {
                while let Some(msg) = osd_rx.next().await {
                    match msg {
                        OsdMsg::Reset => {
                            osd_gen.set(osd_gen.get() + 1);
                            win::window::hide(osd_hwnd);
                        }
                        OsdMsg::State(key, on) => {
                            osd_gen.set(osd_gen.get() + 1);
                            let my_gen = osd_gen.get();
                            // 先换内容再显示:用户永远看不到上一张卡片。
                            let _ = osd_handle.update(&mut *cx, |view, _window, cx| {
                                view.set_state(key, on, cx);
                            });
                            win::monitor::place_centered_on_active_monitor(
                                osd_hwnd, OSD_WIDTH, OSD_HEIGHT,
                            );
                            let timer = cx.background_executor().timer(OSD_HIDE_AFTER);
                            let gen_cell = Rc::clone(&osd_gen);
                            cx.spawn(async move |_cx: &mut AsyncApp| {
                                timer.await;
                                if gen_cell.get() == my_gen {
                                    win::window::hide(osd_hwnd);
                                }
                            })
                            .detach();
                        }
                    }
                }
            })
            .detach();
        }

        // CoreEffect → Win32 执行。FocusInput 的视图侧焦点
        // 由 LauncherView 自己在 render 时消费(见 sakana-ui)。
        // ever_shown 同时是渲染预热的护栏:会话开过就不必预热。
        // visible_fx 是 §140 OSD 抑制的可见性镜像(Show/Hide 是
        // 仅有的两个可见性迁移,都在这条效果通道上)。
        let ever_shown = Rc::new(std::cell::Cell::new(false));
        let ever_shown_fx = ever_shown.clone();
        let visible_fx = Rc::clone(&launcher_visible);
        window_handle
            .update(cx, |view, _window, _cx| {
                view.set_effect_handler(Box::new(move |effect| {
                    logln!("[effect] {effect:?}");
                    match effect {
                        CoreEffect::ShowLauncher => {
                            ever_shown_fx.set(true);
                            visible_fx.set(true);
                            win::monitor::place_on_active_monitor(
                                hwnd,
                                WINDOW_WIDTH,
                                WINDOW_HEIGHT,
                            );
                            // 必须在抢到前台之前记录用户的输入法布局。
                            let _ = win::ime::enter_english_mode(hwnd);
                            win::window::show_and_focus(hwnd);
                        }
                        CoreEffect::HideLauncher => {
                            visible_fx.set(false);
                            // 窗口仍在前台时恢复用户布局;
                            // 失焦隐藏路径是尽力而为(已知边界)。
                            win::ime::restore_saved_layout();
                            win::window::hide(hwnd);
                        }
                        CoreEffect::FocusInput => {
                            let _ = win::window::focus(hwnd);
                        }
                        CoreEffect::QuitApplication => {
                            // Core 已停止模块并刷完 usage,再退出消息循环。
                            win::tray::remove();
                            win::ime::restore_saved_layout();
                            win::host::request_quit();
                        }
                    }
                }));
            })
            .expect("wire effect handler");

        // 首次唤起的 406 ms 是 GPU/字体管线的惰性初始化。
        // 空闲时离屏预热一帧,把它从唤起热路径上挪走。
        // 若用户抢先唤起,ever_shown 护栏让预热直接跳过。
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(700))
                .await;
            let _ = cx.update(|_cx| {
                if !ever_shown.get() {
                    win::window::render_warmup_offscreen(hwnd);
                }
            });
        })
        .detach();

        // 进程生命周期资源:guard 随进程退出释放,无回收点。
        // hotkey_slot / lockkeys_slot 被 run 闭包环境与 Core 内回调
        // 共同持有;OSD 泵任务持有 osd_handle——均无需 forget 进元组
        // 以外的额外处置。
        std::mem::forget((host, focus_hook, window_handle, osd_handle, single_instance));
    });
}
