//! Host window:隐藏顶层窗口,承载 Host 关注的全部 Win32 消息
//! (WM_HOTKEY、第二实例唤醒、失焦通知、托盘回调),与 GPUI 窗口完全解耦——
//! 主线程消息循环(GPUI 驱动)会把投递到本窗口的消息分发到我们的 WndProc。
//!
//! 不是 message-only 窗口:托盘菜单要求 owner 可设为前台,
//! message-only 窗口做不到。
//!
//! 失焦检测用 `SetWinEventHook(EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT)`:
//! 无需子类化 GPUI 窗口,前台窗口变化且不是 Launcher 时通知 Core。

use cue_protocol::logln;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::{NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, PCWSTR, w};

/// Host 上报给编排层的事件(由 cue binary 翻译成 Core 的 HostEvent)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostMsg {
    HotkeyPressed,
    /// 第二实例请求 show / focus,或托盘左键/菜单"显示"。
    ShowRequested,
    /// 托盘菜单"设置"(设置页入口)。
    OpenSettings,
    /// 托盘菜单"退出":进程唯一的正常退出路径。
    QuitRequested,
    /// 前台焦点离开 Launcher 窗口。
    FocusLost,
}

/// 窗口类名。同时是第二实例 `FindWindow` 的定位依据。
pub const HOST_WINDOW_CLASS: PCWSTR = w!("CUE.HostWindow");

pub const WM_CUE_SHOW: u32 = WM_APP + 1;
pub const WM_CUE_FOCUS_LOST: u32 = WM_APP + 2;
/// 托盘图标回调消息,lparam 为鼠标消息。
pub const WM_CUE_TRAY: u32 = WM_APP + 3;
/// 托盘菜单命令的延迟分发消息(见 tray::show_menu):wParam 为命令 id。
/// TrackPopupMenu 返回后菜单窗口仍在拆除,Windows 会把前台还给原前台
/// 窗口(且这一步发生在我们 PostMessage 的消息被处理之后——实测即
/// 使延迟一拍,刚 Show+Focus 的 Launcher 仍被抢焦点,FocusLost →
/// 失焦隐藏)。所以这里再升级成 SetTimer 延迟 ~60ms,等前台仲裁落定。
pub const WM_CUE_TRAY_CMD: u32 = WM_APP + 4;
/// 托盘命令的定时器 id 基数:timer id = 基数 + 命令 id(免去全局状态)。
pub const TRAY_CMD_TIMER_BASE: usize = 0xC0E0;

/// WTS 会话通知(winuser.h;windows crate 未导出,按 ABI 值定义)。
/// 用途见 wnd_proc 的 WM_WTSSESSION 分支:锁屏 = 失焦。
const WM_WTSSESSION: u32 = 0x02B1;
/// 会话被锁(Win+L / 唤醒要求登录自动锁 / 屏保锁)。
const WTS_SESSION_LOCK: usize = 0x7;
/// 会话被从控制台断开(快速用户切换离开本会话)。
const WTS_CONSOLE_DISCONNECT: usize = 0x2;

static LAUNCHER_HWND: AtomicIsize = AtomicIsize::new(0);
static HOST_HWND: AtomicIsize = AtomicIsize::new(0);

/// core.dnd_mode 的 host 侧镜像(Core 的 notify_dnd_mode 回调驱动,
/// UI 线程):免打扰状态图标的门控半边。初始 true 与设置默认值一致,
/// Core::new 后立刻用真实值校准,偏差窗口无害(见 set_dnd_enabled)。
static DND_ENABLED: AtomicBool = AtomicBool::new(true);

/// Core 告知 dnd 开关值(初始一次 + 每次成功 commit,§127)。
/// 设置翻转时立即重估图标——正在全屏前台时关掉免打扰,图标当场回红。
pub fn set_dnd_enabled(on: bool) {
    DND_ENABLED.store(on, Ordering::SeqCst);
    refresh_dnd_icon();
}

/// 免打扰生效 = 开关开 && 前台全屏;托盘图标红 ↔ 灰。
/// 在前台切换钩子上调用:事件驱动、纯查询、翻转才换,零轮询。
fn refresh_dnd_icon() {
    let engaged =
        DND_ENABLED.load(Ordering::SeqCst) && crate::fullscreen::foreground_is_fullscreen();
    crate::tray::set_dnd_engaged(engaged);
}

/// 窗口创建后由编排层告知 Launcher 的 HWND(失焦比较用)。
pub fn set_launcher_hwnd(hwnd: HWND) {
    LAUNCHER_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
}

pub fn launcher_hwnd() -> Option<HWND> {
    let v = LAUNCHER_HWND.load(Ordering::SeqCst);
    (v != 0).then_some(HWND(v as *mut core::ffi::c_void))
}

pub struct HostWindow {
    hwnd: HWND,
}

impl HostWindow {
    /// 在调用线程(必须是带消息循环的主线程)创建隐藏顶层窗口。
    /// 永不 ShowWindow;顶层是为了托盘菜单的前台 owner 要求。
    ///
    /// handler 不需要 Send:它只会被本窗口的 WndProc 调用,而窗口
    /// 有线程亲和(Win32 保证 WndProc 在创建线程上跑)——编排层
    /// 因此可以在 handler 里持有 Rc<RefCell<...>> 这类单线程状态。
    pub fn create(handler: Box<dyn Fn(HostMsg)>) -> Result<Self, Error> {
        unsafe {
            let hinstance = GetModuleHandleW(None)?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(host_wnd_proc),
                hInstance: hinstance.into(),
                lpszClassName: HOST_WINDOW_CLASS,
                ..Default::default()
            };
            // 重复注册返回 0,忽略(单实例保证下只会注册一次)。
            let _ = RegisterClassW(&wc);

            let state = Box::into_raw(Box::new(handler));
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                HOST_WINDOW_CLASS,
                w!("CUE Host"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinstance.into()),
                Some(state as *const core::ffi::c_void),
            )?;
            HOST_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
            // 锁屏感知:锁屏切到安全桌面时,WinEvent 钩子能否收到前台
            // 事件是未文档行为(实测 Win11 26200 显式锁屏有 LockApp 前台
            // 事件,但随构建与锁屏路径而异,如灭屏自动锁),不能只依赖它。
            // WTS 会话通知是文档化的锁屏途径;注册失败只降级为旧行为。
            // 注册随进程生命周期,host 窗口与进程同寿,无需 unregister。
            if let Err(e) = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) {
                logln!("[host] WTSRegisterSessionNotification failed: {e}");
            }
            Ok(Self { hwnd })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

/// 退出路径:结束主线程消息循环,GPUI 的 run 随 WM_QUIT 返回,
/// 进程正常退出(热键随进程释放;托盘图标由编排层先 remove)。
pub fn request_quit() {
    unsafe {
        PostQuitMessage(0);
    }
}

type HostHandler = Box<dyn Fn(HostMsg)>;

unsafe extern "system" fn host_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = lparam.0 as *const CREATESTRUCTW;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*cs).lpCreateParams as isize);
            return LRESULT(1);
        }
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const HostHandler;
        if !state.is_null() {
            let handler = &*state;
            match msg {
                WM_HOTKEY => handler(HostMsg::HotkeyPressed),
                m if m == WM_CUE_SHOW => handler(HostMsg::ShowRequested),
                m if m == WM_CUE_FOCUS_LOST => handler(HostMsg::FocusLost),
                m if m == WM_WTSSESSION => {
                    // 锁屏 / 快速用户切换离开控制台:安全桌面接管输入,
                    // 焦点必然丢失。前台事件钩子在此刻是否投递是未文档
                    // 行为(实测与 WTS 的先后都不保证),以文档化的 WTS
                    // 为准补投 FocusLost,是否隐藏仍由
                    // core.hide_on_focus_loss 裁决(§115)。重复投递无害
                    // (visible=false 时是 no-op)。解锁不自动唤起:
                    // 残留可见是误唤醒,主动 show 更是。
                    if wparam.0 == WTS_SESSION_LOCK || wparam.0 == WTS_CONSOLE_DISCONNECT {
                        logln!("[host] session lock/disconnect -> FocusLost");
                        handler(HostMsg::FocusLost);
                    }
                }
                m if m == WM_CUE_TRAY => crate::tray::handle_message(hwnd, lparam, handler),
                m if m == WM_CUE_TRAY_CMD => {
                    // 命令 id 编进 timer id,WM_TIMER 时再取回分发。
                    let _ = SetTimer(Some(hwnd), TRAY_CMD_TIMER_BASE + wparam.0, 60, None);
                }
                WM_TIMER => {
                    let id = wparam.0;
                    if id > TRAY_CMD_TIMER_BASE {
                        let _ = KillTimer(Some(hwnd), id);
                        if let Some(msg) = crate::tray::msg_from_cmd(id - TRAY_CMD_TIMER_BASE) {
                            handler(msg);
                        }
                    }
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

// ---------------------------------------------------------------------
// 失焦钩子
// ---------------------------------------------------------------------

pub struct FocusHook(HWINEVENTHOOK);

/// 在带消息循环的主线程安装。回调读取 `set_launcher_hwnd` 设置的 HWND;
/// 前台窗口变为非 Launcher 时向 host window 投递 FocusLost。
pub fn install_focus_hook() -> Result<FocusHook, Error> {
    unsafe {
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        if hook.0.is_null() {
            Err(Error::from_thread())
        } else {
            Ok(FocusHook(hook))
        }
    }
}

impl Drop for FocusHook {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWinEvent(self.0);
        }
    }
}

unsafe extern "system" fn foreground_win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if event != EVENT_SYSTEM_FOREGROUND {
        return;
    }
    // 免打扰状态图标:每次前台变化都重估——与 §127"只在按键瞬间
    // 探测"的热键门控互补,这里是图标状态,必须随状态连续。
    refresh_dnd_icon();
    let launcher = LAUNCHER_HWND.load(Ordering::SeqCst);
    if launcher == 0 || hwnd.0 as isize == launcher {
        return;
    }
    // 诊断:只在 Launcher 可见时(即焦点真的从我们手里离开)记录
    // 新前台窗口是谁。全局前台切换本身不刷屏。
    let launcher_hwnd = HWND(launcher as *mut core::ffi::c_void);
    if unsafe { IsWindowVisible(launcher_hwnd) }.as_bool() {
        let mut pid = 0u32;
        let mut class = [0u16; 64];
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let n = GetClassNameW(hwnd, &mut class);
            let class = String::from_utf16_lossy(&class[..n as usize]);
            logln!("[focus] launcher lost foreground -> pid={pid} class={class:?}");
        }
    }
    let host = HOST_HWND.load(Ordering::SeqCst);
    if host != 0 {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(host as *mut core::ffi::c_void)),
                WM_CUE_FOCUS_LOST,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}
