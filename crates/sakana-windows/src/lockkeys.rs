//! 锁键状态提示(§140;§144 起裁为纯观察):WH_KEYBOARD_LL 全局钩子 +
//! 专用线程,只观察不干预——不吞键、不注入、无手势、无定时器。
//!
//! §144 裁撤:§140 原设计的手势(轻点切输入法 / 长按切锁定)与
//! NumLock 三模式(长按防误触 / 常开守护)都建立在吞键 + 注入之上,
//! 而 Windows 在进 LL 钩子**之前**就翻转 toggle 态——吞消息挡不住
//! 翻转,得靠回滚注入抵消;回滚又撞上 win32k 的 make/repeat 语义
//! (按住时注入的"按下"是重复报文,翻不动 toggle),且被吞的键从不
//! 投递,本进程的状态读数(GetKeyState / GetAsyncKeyState)与前台
//! 真态永久脱钩,OSD 只能靠账本。每一层修补都引入下一层不一致,
//! 2026-09 全部砍掉(详见 architecture/records/144)。
//!
//! 状态追踪 = **按下沿记账**,不读 API:我们不吞任何键,锁键的每个
//! 真实按下沿必然翻转前台 toggle(翻转在进钩子前发生,与钩子是否
//! 放行无关),钩子见到按下沿就把账本翻一次并上报 host——计数因此
//! 永远为真。启动时以 GetAsyncKeyState 读数初始化账本:进程没跑就
//! 没有吞,历史按键全是投递过的,读数可信。残余盲区:别的程序用
//! SetKeyboardState 改它自己线程的队列态——无事件可捕,影响面也
//! 本来只有那个程序。
//!
//! 线程模型:钩子装在本 worker 线程,GetMessage 泵投递钩子回调;
//! 回调里只做 O(1) 判定与 PostMessage,绝不阻塞系统输入。

use crate::host::WM_SAKANA_LOCKKEY;
use sakana_protocol::LockKeysConfig;
use sakana_protocol::logln;
use std::cell::RefCell;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

// LockKey 定义在 protocol(纯数据,OSD 视图也按它区分文案);
// 此处再导出,host.rs 等引用 crate::lockkeys::LockKey 的地方不动。
pub use sakana_protocol::LockKey;

thread_local! { static STATE: RefCell<Option<Watch>> = const { RefCell::new(None) }; }

/// 共享配置。UI 线程写(set_config),worker 逐事件读。
type Shared = Arc<Mutex<LockKeysConfig>>;

/// worker 窗口私有消息:会话复位(锁屏吞掉过释放事件)后重同步账本。
const WM_LOCKKEYS_RESET: u32 = WM_APP + 1;

struct Watch {
    host: HWND,
    shared: Shared,
    ledger: Ledger,
}

/// 锁定态账本:启动时以 API 读数初始化,此后只在按下沿翻转。
/// 账本是被观察键状态的唯一可信来源(见模块头注释)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Ledger {
    caps: bool,
    num: bool,
    /// 物理按下沿标记:自动重复的 down 不翻 toggle,也不记账。
    caps_down: bool,
    num_down: bool,
}

impl Ledger {
    /// 一个按键事件入账:返回 (键, 新状态) 仅当账本真的翻转。
    /// 与配置无关——上报与否由调用点按 enabled/osd 门控。
    pub(crate) fn on_key(&mut self, vk: VIRTUAL_KEY, down: bool) -> Option<(LockKey, bool)> {
        let (on, held, key) = match vk {
            VK_CAPITAL => (&mut self.caps, &mut self.caps_down, LockKey::Caps),
            VK_NUMLOCK => (&mut self.num, &mut self.num_down, LockKey::Num),
            _ => return None,
        };
        if !down {
            *held = false;
            return None;
        }
        if *held {
            return None; // 自动重复
        }
        *held = true;
        *on = !*on;
        Some((key, *on))
    }
}

/// 启动初始化读数:此刻可信(进程没跑就没有吞,历史按键全是投递
/// 过的)。运行期不再读——账本在按下沿推进。
fn initial_toggle_on(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(vk.0 as i32) & 2 != 0 }
}

pub struct Worker {
    window: HWND,
    thread: Option<thread::JoinHandle<()>>,
    shared: Shared,
}

impl Worker {
    /// host = 接收 WM_SAKANA_LOCKKEY 的 host 窗口(已存在于 UI 线程)。
    pub fn start(host: HWND, config: LockKeysConfig) -> Result<Self> {
        let shared: Shared = Arc::new(Mutex::new(config));
        let (tx, rx) = mpsc::sync_channel(1);
        let host_raw = host.0 as usize;
        let shared_clone = Arc::clone(&shared);
        let thread = thread::spawn(move || {
            // SAFETY: 钩子、窗口、消息循环都由本线程持有到清理结束。
            unsafe {
                let setup = (|| -> Result<(HWND, HHOOK)> {
                    let instance = GetModuleHandleW(None)?;
                    let class = WNDCLASSW {
                        lpfnWndProc: Some(worker_wnd_proc),
                        hInstance: instance.into(),
                        lpszClassName: w!("sakana.LockKeys"),
                        ..Default::default()
                    };
                    if RegisterClassW(&class) == 0 {
                        return Err(Error::from_thread());
                    }
                    let window = CreateWindowExW(
                        WINDOW_EX_STYLE::default(),
                        class.lpszClassName,
                        w!(""),
                        WINDOW_STYLE::default(),
                        0,
                        0,
                        0,
                        0,
                        Some(HWND_MESSAGE),
                        None,
                        Some(instance.into()),
                        None,
                    )?;
                    STATE.with_borrow_mut(|state| {
                        let ledger = Ledger {
                            caps: initial_toggle_on(VK_CAPITAL),
                            num: initial_toggle_on(VK_NUMLOCK),
                            caps_down: false,
                            num_down: false,
                        };
                        logln!(
                            "[lockkeys] watch started: caps={} num={}",
                            ledger.caps,
                            ledger.num
                        );
                        *state = Some(Watch {
                            host: HWND(host_raw as _),
                            shared: shared_clone,
                            ledger,
                        })
                    });
                    match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), Some(instance.into()), 0) {
                        Ok(hook) => Ok((window, hook)),
                        Err(error) => {
                            let _ = DestroyWindow(window);
                            Err(error)
                        }
                    }
                })();
                match setup {
                    Ok((window, hook)) => {
                        let _ = tx.send(Ok(window.0 as usize));
                        let mut msg = MSG::default();
                        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                            DispatchMessageW(&msg);
                        }
                        let _ = UnhookWindowsHookEx(hook);
                        let _ = DestroyWindow(window);
                        STATE.with_borrow_mut(|state| *state = None);
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error));
                    }
                }
            }
        });
        let window = rx
            .recv()
            .map_err(|_| Error::new(E_FAIL, "lockkeys thread did not start"))??;
        Ok(Self {
            window: HWND(window as _),
            thread: Some(thread),
            shared,
        })
    }

    /// 推送新配置:存值即可,worker 逐事件读。无手势无定时器,
    /// 没有需要随配置取消的挂起状态。
    pub fn set_config(&self, config: LockKeysConfig) {
        *self.shared.lock().unwrap() = config;
    }

    /// 会话复位(锁屏/挂起):按键的释放事件可能没送达,沿标记与
    /// 账本都可能失真——投私有消息让 worker 清沿并按当前读数重同步
    /// (纯观察世界里读数可信:我们不吞任何键,每次翻转都伴随投递)。
    pub fn reset(&self) {
        // SAFETY: worker 窗口活着,直到本 owner 的 Drop;只投递整数。
        unsafe {
            let _ = PostMessageW(Some(self.window), WM_LOCKKEYS_RESET, WPARAM(0), LPARAM(0));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // SAFETY: worker 窗口活着,直到本 owner 的 Drop;WM_CLOSE 退消息循环。
        unsafe {
            let _ = PostMessageW(Some(self.window), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: HC_ACTION 时 lparam 是有效的 KBDLLHOOKSTRUCT,回调期内有效。
    unsafe {
        if code != HC_ACTION as i32 {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let down = wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN;
        let vk = VIRTUAL_KEY(event.vkCode as u16);
        // 纯观察:账本翻转 + 按配置门控的上报,然后一律放行。
        STATE.with_borrow_mut(|state| {
            let state = state.as_mut().unwrap();
            if let Some((key, on)) = state.ledger.on_key(vk, down) {
                let config = *state.shared.lock().unwrap();
                if config.enabled && config.osd {
                    post_lockkey(state.host, key, on);
                }
            }
        });
        CallNextHookEx(None, code, wparam, lparam)
    }
}

fn post_lockkey(host: HWND, key: LockKey, on: bool) {
    let key = match key {
        LockKey::Caps => 0,
        LockKey::Num => 1,
    };
    // SAFETY: 只投递整数;host 窗口与进程同寿。
    unsafe {
        let _ = PostMessageW(
            Some(host),
            WM_SAKANA_LOCKKEY,
            WPARAM(key),
            LPARAM(on as isize),
        );
    }
}

unsafe extern "system" fn worker_wnd_proc(
    window: HWND,
    message: u32,
    _wparam: WPARAM,
    _lparam: LPARAM,
) -> LRESULT {
    // SAFETY: 全部在本 worker 线程的消息泵上运行。
    unsafe {
        match message {
            WM_CLOSE => PostQuitMessage(0),
            // 会话复位:清按下沿 + 按当前读数重同步账本(见 reset)。
            m if m == WM_LOCKKEYS_RESET => STATE.with_borrow_mut(|state| {
                let ledger = &mut state.as_mut().unwrap().ledger;
                ledger.caps_down = false;
                ledger.num_down = false;
                ledger.caps = initial_toggle_on(VK_CAPITAL);
                ledger.num = initial_toggle_on(VK_NUMLOCK);
            }),
            _ => return DefWindowProcW(window, message, _wparam, _lparam),
        }
        LRESULT(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 账本契约:按下沿翻一次,重复 down 不翻,抬起后再按才再翻;
    /// 无关键不记账(§144 纯观察:记账是状态追踪的唯一手段)。
    #[test]
    fn ledger_flips_on_press_edges_only() {
        let mut ledger = Ledger::default();
        // 按下沿:翻。
        assert_eq!(ledger.on_key(VK_NUMLOCK, true), Some((LockKey::Num, true)));
        // 自动重复:不翻。
        assert_eq!(ledger.on_key(VK_NUMLOCK, true), None);
        // 抬起:不翻;再按:翻回。
        assert_eq!(ledger.on_key(VK_NUMLOCK, false), None);
        assert_eq!(ledger.on_key(VK_NUMLOCK, true), Some((LockKey::Num, false)));
        // Caps 独立记账;无关键无账。
        assert_eq!(ledger.on_key(VK_CAPITAL, true), Some((LockKey::Caps, true)));
        assert_eq!(ledger.on_key(VK_A, true), None);
        // 缺抬起的下一次按下(锁屏吞掉释放等)按重复处理,不翻。
        assert_eq!(ledger.on_key(VK_CAPITAL, true), None);
    }
}
