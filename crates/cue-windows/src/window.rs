//! Launcher 窗口的 Win32 操作:查找、显示、隐藏、聚焦。
//!
//! HWND 通过"按进程枚举顶层可见窗口"获得,刻意不依赖 GPUI 的内部 API
//! (raw window handle),把 GPUI 版本漂移关在门外。

use cue_protocol::logln;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Error};

/// 枚举本进程的顶层窗口,返回 GPUI 主窗口的 HWND。
///
/// 按类名过滤:GPUI 0.2 的主窗口类名是 `Zed::Window`。host window
/// 也是本进程的顶层(隐藏)窗口,不过滤就会张冠李戴;
/// 也不过滤可见性——Launcher 窗口创建时就是隐藏的。
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
