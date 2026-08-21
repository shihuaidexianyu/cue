//! Launcher 窗口的 Win32 操作:查找、显示、隐藏、聚焦。
//!
//! HWND 通过"按进程枚举顶层可见窗口"获得,刻意不依赖 GPUI 的内部 API
//! (raw window handle),把 GPUI 版本漂移关在门外。

use windows::core::{Error, BOOL};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

/// 枚举本进程的顶层窗口,返回 GPUI 主窗口的 HWND。
///
/// 按类名过滤:GPUI 0.2 的主窗口类名是 `Zed::Window`。host window
/// 也是本进程的顶层(隐藏)窗口(§116),不过滤就会张冠李戴;
/// 也不过滤可见性——Launcher 窗口创建时就是隐藏的(§54)。
pub fn find_main_window_hwnd() -> Option<HWND> {
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
            eprintln!(
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

pub fn focus(hwnd: HWND) -> Result<(), Error> {
    unsafe {
        if SetForegroundWindow(hwnd).as_bool() {
            Ok(())
        } else {
            Err(Error::from_thread())
        }
    }
}
