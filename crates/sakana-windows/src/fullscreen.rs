//! 前台全屏探针(免打扰模式,§127)。
//!
//! 定义:前台顶层窗口的矩形**覆盖其所在显示器的全部物理区域**
//! (`GetWindowRect` ⊇ `MONITORINFO.rcMonitor`,含等于与超出),
//! 且该窗口不属于桌面 shell 类。覆盖判定用 `>=` 而非 `==`:
//! 独占全屏(游戏、部分播放器)常把窗口撑得比屏幕大几像素。
//! 最大化窗口顶到任务栏为止(`rcWork`),不会误判;桌面与
//! 任务栏本身通过类名白名单排除(截图/取 rect 可能覆盖全屏,
//! 但它们显然不是游戏)。

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetWindowRect};

/// 桌面 shell 顶层类名——它们可能覆盖全屏但不是"全屏应用"。
const SHELL_CLASSES: [&str; 4] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
];

fn is_shell_class(class: &str) -> bool {
    SHELL_CLASSES.contains(&class)
}

/// 窗口矩形是否覆盖显示器矩形(含超出)。
fn rect_covers_monitor(window: RECT, monitor: RECT) -> bool {
    window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
}

/// 当前前台窗口是否处于"全屏覆盖"状态(免打扰模式门控的唯一依据)。
/// 任何一步 Win32 失败都返回 false——宁可唤起,不错杀。
pub fn foreground_is_fullscreen() -> bool {
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.is_invalid() {
            return false;
        }

        let mut buf = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut buf);
        if len == 0 {
            return false;
        }
        if is_shell_class(&String::from_utf16_lossy(&buf[..len as usize])) {
            return false;
        }

        let mut window_rect = RECT::default();
        if GetWindowRect(hwnd, &mut window_rect).is_err() {
            return false;
        }

        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }

        rect_covers_monitor(window_rect, info.rcMonitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: RECT = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }

    #[test]
    fn exact_cover_counts_as_fullscreen() {
        assert!(rect_covers_monitor(SCREEN, SCREEN));
    }

    #[test]
    fn overshoot_counts_as_fullscreen() {
        // 独占全屏常比屏幕大几像素;副屏则整体平移到负坐标。
        assert!(rect_covers_monitor(rect(-2, -2, 1922, 1082), SCREEN));
        assert!(rect_covers_monitor(
            rect(-1920, 0, 0, 1080),
            rect(-1920, 0, 0, 1080)
        ));
    }

    #[test]
    fn maximized_window_does_not_count() {
        // 最大化顶到任务栏:底边在屏幕底之上(rcWork),不覆盖。
        assert!(!rect_covers_monitor(rect(0, 0, 1920, 1040), SCREEN));
        // 普通窗口更不覆盖。
        assert!(!rect_covers_monitor(rect(100, 100, 900, 700), SCREEN));
        // 单侧没铺满也不算。
        assert!(!rect_covers_monitor(rect(0, 0, 960, 1080), SCREEN));
    }

    #[test]
    fn shell_classes_are_excluded() {
        assert!(is_shell_class("Progman"));
        assert!(is_shell_class("WorkerW"));
        assert!(is_shell_class("Shell_TrayWnd"));
        assert!(is_shell_class("Shell_SecondaryTrayWnd"));
        assert!(!is_shell_class("Chrome_WidgetWin_1"));
        assert!(!is_shell_class(""));
    }
}
