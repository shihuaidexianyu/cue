// release 用 GUI 子系统:双击/开始菜单/自启不再给进程挂控制台窗口。
// debug 保留控制台(cargo run 可见 [boot]/[perf] 探针);release 下
// 探针仍可由父进程重定向 stderr 捕获(§114 E2E 正是这么测的)。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! cue —— Launcher 可执行文件,唯一的 composition root(§70、§112)。
//!
//! 编排:HostEvent → Core → CoreEffect → cue-ui / cue-windows。
//! 只有本 crate 同时认识 Core、GPUI 和 Win32。

use cue_core::{Core, CoreConfig, CoreEffect, CoreEvent, HostEvent, ModuleRegistry, TaskSpawner};
use cue_module_app::AppModule;
use cue_protocol::Hotkey;
use cue_ui::LauncherView;
use cue_windows as win;
use futures::future::BoxFuture;
use gpui::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

/// §97:生产环境 TaskSpawner,把 Core 的异步工作挂到 GPUI 后台线程池。
struct GpuiSpawner {
    executor: BackgroundExecutor,
}

impl TaskSpawner for GpuiSpawner {
    fn spawn(&self, fut: BoxFuture<'static, ()>) {
        // §91 北极星:Core 不做物理取消——spawn 即移交,detach 后
        // 结果有效性由 SessionId / ModuleEpoch / Generation 判定。
        self.executor.spawn(fut).detach();
    }
}

const WINDOW_WIDTH: i32 = 640;
const WINDOW_HEIGHT: i32 = 420;

/// 开发期热键覆盖:`CUE_HOTKEY="ctrl+alt+k"`(格式同设置值,§53)。
/// 只影响本次进程的初始注册,不写入 settings.tsv——调试覆盖不应
/// 变成持久配置。正式修改走托盘 → 设置(§41)。
fn parse_hotkey_env() -> Option<Hotkey> {
    std::env::var("CUE_HOTKEY").ok()?.parse().ok()
}

fn main() {
    let boot_started = std::time::Instant::now();
    // §113:单实例必须在最早时机——任何状态文件被打开之前。
    // 第二实例:signal_first_instance 已在 acquire 内完成,直接退出。
    let single_instance = match win::single_instance::acquire() {
        win::single_instance::AcquireOutcome::Primary(guard) => guard,
        win::single_instance::AcquireOutcome::AlreadyRunning => return,
    };

    Application::new().run(move |cx: &mut App| {
        let spawner = Arc::new(GpuiSpawner {
            executor: cx.background_executor().clone(),
        });

        let mut registry = ModuleRegistry::new();
        // §65:AppModule 是 V1 的 required default module。
        registry
            .register(Box::new(AppModule::new()))
            .expect("register app module");

        let storage_root = std::env::var("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("CUE"))
            .unwrap_or_else(|_| PathBuf::from("CUE"));

        // §53:apply_hotkey 回调与初始注册共用同一个 HotkeyManager。
        // Core::new 先于 host window(manager 需要 host hwnd)——
        // 用共享槽打破构造顺序环;槽只在 UI 线程访问。
        let hotkey_slot: Rc<RefCell<Option<win::hotkey::HotkeyManager>>> =
            Rc::new(RefCell::new(None));
        let apply_hotkey = {
            let slot = Rc::clone(&hotkey_slot);
            move |hk: &Hotkey| -> Result<(), String> {
                match slot.borrow_mut().as_mut() {
                    Some(m) => {
                        let r = m.apply(*hk).map_err(|e| e.to_string());
                        eprintln!(
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

        let core = Core::new(
            CoreConfig {
                usage_file: Some(storage_root.join("usage.tsv")),
                settings_file: Some(storage_root.join("settings.tsv")),
                apply_hotkey: Some(Box::new(apply_hotkey)),
                apply_start_on_boot: Some(Box::new(apply_start_on_boot)),
                storage_root,
                ..CoreConfig::default()
            },
            registry,
            spawner,
        )
        .expect("core init");
        let core_tx = core.event_sender();

        // Host window(Win32 消息入口)→ Core 事件队列(§112)。
        let host = win::host::HostWindow::create(Box::new(move |msg| {
            eprintln!("[host] {msg:?}");
            // §116:托盘"退出"是唯一正常退出路径——先删托盘图标
            // (不留幽灵图标),再结束消息循环;热键随进程释放。
            if msg == win::host::HostMsg::QuitRequested {
                win::tray::remove();
                win::host::request_quit();
                return;
            }
            let event = match msg {
                win::host::HostMsg::HotkeyPressed => HostEvent::HotkeyPressed,
                win::host::HostMsg::ShowRequested => HostEvent::ShowRequested,
                win::host::HostMsg::OpenSettings => HostEvent::OpenSettings,
                win::host::HostMsg::FocusLost => HostEvent::FocusLost,
                win::host::HostMsg::QuitRequested => unreachable!(),
            };
            let _ = core_tx.unbounded_send(CoreEvent::Host(event));
        }))
        .expect("host window");

        // §116:托盘图标是进程存活的唯一常驻可见信号。
        win::tray::add(host.hwnd()).expect("tray icon");

        // 热键管理器入槽;初始注册 = env 覆盖(仅本次进程)或设置值(§53)。
        *hotkey_slot.borrow_mut() = Some(win::hotkey::HotkeyManager::new(host.hwnd()));
        let initial_hotkey = parse_hotkey_env().unwrap_or_else(|| core.hotkey());
        let registered = hotkey_slot
            .borrow_mut()
            .as_mut()
            .expect("hotkey manager just installed")
            .apply(initial_hotkey);
        // 热键被其他应用(如另一个 launcher)占用时降级为警告:
        // Launcher 继续运行,可经第二实例信号唤起,设置里可换键。
        if let Err(e) = registered {
            eprintln!("[warn] hotkey registration failed: {e}");
        }
        // §77 冷启动预算(< 500 ms)的常驻探针:进程入口 → 热键就绪。
        eprintln!("[boot] hotkey ready in {:?}", boot_started.elapsed());

        // §54:常驻但默认隐藏——窗口创建时不可见,等待 ShowLauncher 效果。
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

        cx.activate(true);

        // HWND 发现(按进程枚举,避开 GPUI 内部 API 漂移)。
        let hwnd = win::window::find_main_window_hwnd().expect("launcher hwnd");
        win::host::set_launcher_hwnd(hwnd);
        let focus_hook = win::host::install_focus_hook().expect("focus hook");

        // §112:CoreEffect → Win32 执行。FocusInput 的视图侧焦点
        // 由 LauncherView 自己在 render 时消费(见 cue-ui)。
        // ever_shown 同时是 §114 渲染预热的护栏:会话开过就不必预热。
        let ever_shown = Rc::new(std::cell::Cell::new(false));
        let ever_shown_fx = ever_shown.clone();
        window_handle
            .update(cx, |view, _window, _cx| {
                view.set_effect_handler(Box::new(move |effect| {
                    eprintln!("[effect] {effect:?}");
                    match effect {
                        CoreEffect::ShowLauncher => {
                            ever_shown_fx.set(true);
                            win::monitor::place_on_active_monitor(
                                hwnd,
                                WINDOW_WIDTH,
                                WINDOW_HEIGHT,
                            );
                            // §107:必须在抢到前台之前记录用户的输入法布局。
                            let _ = win::ime::enter_english_mode(hwnd);
                            win::window::show_and_focus(hwnd);
                        }
                        CoreEffect::HideLauncher => {
                            // §107:窗口仍在前台时恢复用户布局;
                            // 失焦隐藏路径是尽力而为(已知边界)。
                            win::ime::restore_saved_layout();
                            win::window::hide(hwnd);
                        }
                        CoreEffect::FocusInput => {
                            let _ = win::window::focus(hwnd);
                        }
                    }
                }));
            })
            .expect("wire effect handler");

        // §114:首次唤起的 406 ms 是 GPU/字体管线的惰性初始化。
        // 空闲时离屏预热一帧,把它从唤起热路径(§55)上挪走。
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
        // hotkey_slot 被 run 闭包环境与 Core 内回调共同持有,无需 forget。
        std::mem::forget((host, focus_hook, window_handle, single_instance));
    });
}
