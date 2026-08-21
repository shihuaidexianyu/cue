//! cue —— Launcher 可执行文件,唯一的 composition root(§70、§112)。
//!
//! 编排:HostEvent → Core → CoreEffect → cue-ui / cue-windows。
//! 只有本 crate 同时认识 Core、GPUI 和 Win32。

use cue_core::{Core, CoreConfig, CoreEffect, CoreEvent, HostEvent, ModuleRegistry, TaskSpawner};
use cue_module_app::AppModule;
use cue_protocol::{Hotkey, Key, Modifiers};
use cue_ui::LauncherView;
use cue_windows as win;
use futures::future::BoxFuture;
use gpui::*;
use std::path::PathBuf;
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

/// 开发期热键覆盖:`CUE_HOTKEY="ctrl+alt+k"`(修饰键 alt/ctrl/shift/win,
/// 键为单字符或 space/tab/enter/esc/f1..f12)。
/// §53 默认 Alt+Space 不变;正式配置走 Settings(Phase 6)。
/// 用途:开发机上 Alt+Space 常被其他 launcher 占用,不覆盖就没法联调。
fn parse_hotkey_env() -> Option<Hotkey> {
    let raw = std::env::var("CUE_HOTKEY").ok()?;
    let mut modifiers = Modifiers::NONE;
    let mut key = None;
    for token in raw.split('+').map(|t| t.trim().to_ascii_lowercase()) {
        match token.as_str() {
            "alt" => modifiers.alt = true,
            "ctrl" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            "win" | "super" => modifiers.super_key = true,
            "space" => key = Some(Key::Space),
            "tab" => key = Some(Key::Tab),
            "enter" => key = Some(Key::Enter),
            "esc" => key = Some(Key::Escape),
            t if t.len() == 1 => key = Some(Key::Char(t.chars().next().unwrap())),
            t if t.len() >= 2 && t.starts_with('f') => {
                let n: u32 = t[1..].parse().ok()?;
                key = Some(match n {
                    1 => Key::F1,
                    2 => Key::F2,
                    3 => Key::F3,
                    4 => Key::F4,
                    5 => Key::F5,
                    6 => Key::F6,
                    7 => Key::F7,
                    8 => Key::F8,
                    9 => Key::F9,
                    10 => Key::F10,
                    11 => Key::F11,
                    12 => Key::F12,
                    _ => return None,
                });
            }
            _ => return None,
        }
    }
    let key = key?;
    if modifiers.is_empty() {
        return None;
    }
    Some(Hotkey { modifiers, key })
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
        let core = Core::new(
            CoreConfig {
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
                win::host::HostMsg::FocusLost => HostEvent::FocusLost,
                win::host::HostMsg::QuitRequested => unreachable!(),
            };
            let _ = core_tx.unbounded_send(CoreEvent::Host(event));
        }))
        .expect("host window");

        // §116:托盘图标是进程存活的唯一常驻可见信号。
        win::tray::add(host.hwnd()).expect("tray icon");

        let mut hotkeys = win::hotkey::HotkeyManager::new(host.hwnd());
        let hotkey = parse_hotkey_env().unwrap_or(core.config().default_hotkey);
        // 热键被其他应用(如另一个 launcher)占用时降级为警告:
        // Launcher 继续运行,可经第二实例信号唤起,设置落地后可换键。
        if let Err(e) = hotkeys.apply(hotkey) {
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
        window_handle
            .update(cx, |view, _window, _cx| {
                view.set_effect_handler(Box::new(move |effect| {
                    eprintln!("[effect] {effect:?}");
                    match effect {
                        CoreEffect::ShowLauncher => {
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

        // 进程生命周期资源:guard 随进程退出释放,无回收点。
        std::mem::forget((host, hotkeys, focus_hook, window_handle, single_instance));
    });
}
