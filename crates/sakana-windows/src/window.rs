//! Launcher 窗口的 Win32 操作:查找、显示、隐藏、聚焦。
//!
//! HWND 通过"按进程枚举顶层可见窗口"获得,刻意不依赖 GPUI 的内部 API
//! (raw window handle),把 GPUI 版本漂移关在门外。

use sakana_protocol::logln;
use std::collections::HashMap;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow};
use windows::Win32::UI::Input::Ime::{HIMC, ImmAssociateContext};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Error};

/// 枚举本进程的顶层窗口,返回 GPUI 窗口(`Zed::Window` 类)的 HWND。
/// host window 也是本进程的顶层(隐藏)窗口,类名过滤把它挡在门外;
/// 不过滤可见性——Launcher 创建时是隐藏的。
fn find_zed_window() -> Option<HWND> {
    struct Search {
        pid: u32,
        result: HWND,
    }
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe {
            let search = &mut *(lparam.0 as *mut Search);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid != search.pid {
                return BOOL(1);
            }
            let mut class = [0u16; 64];
            let n = GetClassNameW(hwnd, &mut class);
            if String::from_utf16_lossy(&class[..n as usize]) == "Zed::Window" {
                search.result = hwnd;
                return BOOL(0); // 找到即停
            }
            BOOL(1)
        }
    }
    unsafe {
        let mut search = Search {
            pid: std::process::id(),
            result: HWND::default(),
        };
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut search as *mut Search as isize));
        (!search.result.0.is_null()).then_some(search.result)
    }
}

/// 取第一个 `Zed::Window`(唯一的 GPUI 窗口 = Launcher,§148 起
/// 不再有第二个)。
pub fn find_main_window_hwnd() -> Option<HWND> {
    find_zed_window()
}

/// Hotkey 路径的显示 + 前台聚焦。WM_HOTKEY 处理是系统认可的
/// 前台抢占路径,SetForegroundWindow 在这里允许生效。
pub fn show_and_focus(hwnd: HWND) {
    unsafe {
        let shown = ShowWindow(hwnd, SW_SHOW);
        let pos = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        let fg = SetForegroundWindow(hwnd);
        if !fg.as_bool() {
            // 前台抢占被拒:窗口可见但未聚焦,失焦钩子很快会触发隐藏。
            // 诊断留下证据(热键路径理论上不会被拒)。
            logln!(
                "[focus] SetForegroundWindow denied (pos_ok={} err={:?})",
                pos.is_ok(),
                windows::core::Error::from_thread()
            );
        }
        let _ = shown;
    }
}

pub fn hide(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

/// GPUI 0.2.2 的 `WM_DISPLAYCHANGE` 处理器(`handle_display_change_msg`):
/// 窗口记录的显示器一旦断开,它假定"OS 把窗口挪走并最小化了",无条件
/// `ShowWindow(SW_SHOWNORMAL)` 复原——对常驻隐藏的 Launcher 这就是凭空
/// 误唤醒:多屏变单屏 / DP 显示器睡眠断链 / 显卡驱动重排拓扑都会触发。
/// 显示的是最后一次刷新的无会话快照(空输入 + "No results"),完全不
/// 经过 Core。GPUI 是 crates.io 依赖、不 vendor,这里用经典子类化
/// (替换 `GWLP_WNDPROC`;GPUI 的状态挂在 `GWLP_USERDATA`,互不干扰)
/// 在窗口隐藏时把 `WM_DISPLAYCHANGE` 吞掉。
///
/// 吞掉不会让 GPUI 的显示器跟踪失联:下次唤起的
/// `place_on_active_monitor`(SetWindowPos)必然带来 WM_MOVE,跨 DPI 时
/// 还有 WM_DPICHANGED,GPUI 在那两条路径上自行重挂显示器;渲染只依赖
/// scale_factor,不依赖 display handle。窗口可见时不吞,GPUI 的
/// 复原逻辑照常工作。窗口与进程同寿,无需还原子类。
///
/// 多窗口支持(§140):原 wndproc 按 HWND 存进映射表——Launcher 与
/// OSD 窗口各装各的,单个 static 会被第二次安装覆盖(Launcher 的
/// 链就断了)。
pub fn install_display_change_guard(hwnd: HWND) {
    unsafe {
        // 返回值是原 wndproc;类过程指针不可能为 0,为 0 即失败。
        let orig = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, display_guard_proc as *const () as isize);
        if orig == 0 {
            logln!(
                "[warn] display-change guard install failed: {:?}",
                Error::from_thread()
            );
            return;
        }
        ORIG_WNDPROCS
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(hwnd.0 as isize, orig);
    }
}

static ORIG_WNDPROCS: std::sync::Mutex<Option<std::collections::HashMap<isize, isize>>> =
    std::sync::Mutex::new(None);

unsafe extern "system" fn display_guard_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if msg == WM_DISPLAYCHANGE && !IsWindowVisible(hwnd).as_bool() {
            logln!(
                "[host] display change while hidden: swallowed WM_DISPLAYCHANGE (auto-show guard)"
            );
            return LRESULT(0);
        }
        // §142:Launcher 窗口没有"关闭"语义——托盘"退出"是唯一退出
        // 路径(§116)。不吞 WM_CLOSE 的话,Alt+F4 / 任务栏关闭会销毁
        // 窗口,连带 drop LauncherView 与它持有的 Core:事件泵随之
        // 消失,热键、托盘、单实例互斥量全部失灵,进程变成既无 UI
        // 也退不掉的僵尸(§142 记录)。
        if msg == WM_CLOSE {
            logln!("[host] WM_CLOSE on launcher window: swallowed (tray Quit is the only exit)");
            return LRESULT(0);
        }
        let orig = ORIG_WNDPROCS
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|m| m.get(&(hwnd.0 as isize)).copied());
        let Some(orig) = orig else {
            // 不该发生(安装失败时根本没换 wndproc);防御性直通。
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        let orig: WNDPROC = std::mem::transmute::<isize, WNDPROC>(orig);
        CallWindowProcW(orig, hwnd, msg, wparam, lparam)
    }
}

/// 渲染预热:boot 后离屏显示并强制同步一帧,把 DirectX /
/// DirectWrite 管线、字体与字形缓存的惰性初始化从首次唤起路径挪走
/// (实测首唤 406 ms vs 稳态 ~95 ms)。GPUI 在 WM_PAINT 里完成整帧
/// 渲染,所以 InvalidateRect + UpdateWindow 即可驱动预热。
///
/// 只动窗口、不碰 Core 会话与 IME;SWP_NOACTIVATE 保证不抢前台。
/// 调用方负责只在会话从未打开过时调用(开了就已经真实渲染过了)。
pub fn render_warmup_offscreen(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            -32000,
            -32000,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        let _ = InvalidateRect(Some(hwnd), None, true);
        let _ = UpdateWindow(hwnd);
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

pub fn focus(hwnd: HWND) -> Result<(), Error> {
    unsafe {
        if SetForegroundWindow(hwnd).as_bool() {
            Ok(())
        } else {
            Err(Error::from_thread())
        }
    }
}

/// 给 Launcher 窗口挂品牌图标(alt-tab / 任务栏 / 窗口标题区)。
/// 资源缺失(测试宿主等)时静默跳过,沿用系统默认。
pub fn set_brand_icon(hwnd: HWND) {
    let (small, big) = unsafe { (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CXICON)) };
    for (id, size) in [(ICON_SMALL, small), (ICON_BIG, big)] {
        if let Some(h) = crate::icon::brand_icon(size, size) {
            unsafe {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(id as usize)),
                    Some(LPARAM(h.0 as isize)),
                );
            }
        }
    }
}

/// §149 止血(v0.6.3):解除 IME 与 Launcher 窗口的关联。
///
/// 裁撤强制英文后 IME 重新挂靠本窗口,而 GPUI 0.2 的 Windows 后端
/// 不处理 IME 组合投递(WM_IME_COMPOSITION):中文布局下键入的
/// 字母全部进入组合串、无法上屏,触发词与拼音搜索失效;候选窗的
/// 定位查询(WM_IME_COMPOSITION 的 CPS_* / ImmSetCompositionWindow)
/// 无人响应,固定弹在屏幕右下角。
///
/// `ImmAssociateContext(hwnd, NULL)` 让 IME 不挂靠本窗口——按键
/// 直接以字符到达(键盘布局不动,仍是用户自己的全局输入法),
/// 候选窗不再弹出。这是窗口属性、调用一次即常驻,须在创建窗口
/// 的 UI 线程执行。中文直输(组合串上屏)是已知的后续项,
/// 粘贴 Unicode 不受影响(§115)。
pub fn detach_ime(hwnd: HWND) {
    // 传 NULL 解除关联;返回值是之前的关联,不再使用。
    unsafe { ImmAssociateContext(hwnd, HIMC(std::ptr::null_mut())) };
    logln!("[ime] detached (window now has no IMC)");
}
