//! 显示器放置(§54):显示在当前用户活跃 monitor。

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;

/// 以当前前台窗口所在显示器为"活跃显示器",返回其 HMONITOR 与工作区。
fn active_monitor() -> (HMONITOR, RECT) {
    unsafe {
        let foreground = GetForegroundWindow();
        let monitor: HMONITOR = MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let work = if GetMonitorInfoW(monitor, &mut info).as_bool() {
            info.rcWork
        } else {
            // 回退:主显示器 1080p 假设。
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            }
        };
        (monitor, work)
    }
}

/// 目标显示器 DPI 取有效 DPI;失败依次回退到窗口当前 DPI、96。
fn monitor_dpi(monitor: HMONITOR, hwnd: HWND) -> u32 {
    unsafe {
        let mut dpi_x = GetDpiForWindow(hwnd);
        let mut dpi_y = 0u32;
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        if dpi_x == 0 {
            96
        } else {
            dpi_x
        }
    }
}

/// 逻辑尺寸 → 物理像素(按目标 DPI,四舍五入)。
fn logical_to_physical(logical: i32, dpi: u32) -> i32 {
    (logical * dpi as i32 + 48) / 96
}

/// 把 Launcher 窗口放置到活跃显示器:水平居中,垂直约 1/4 处。
///
/// 进程是 PerMonitorV2(见 exe manifest),`SetWindowPos` 吃**物理**
/// 像素:尺寸按目标显示器 DPI 由设计逻辑尺寸换算,保证任意缩放下
/// 都是同一逻辑大小。尺寸必须显式设置——GPUI 以 `show: false` 创建
/// 窗口时用 `CW_USEDEFAULT`,请求的尺寸只存在它内部的
/// `initial_placement` 里、仅由 GPUI 自己的 `activate()` 补设;
/// 我们走原生 `ShowWindow` 唤起,那条路径永远不会执行。
pub fn place_on_active_monitor(hwnd: HWND, logical_w: i32, logical_h: i32) {
    let (monitor, work) = active_monitor();
    let dpi = monitor_dpi(monitor, hwnd);
    let w = logical_to_physical(logical_w, dpi);
    let h = logical_to_physical(logical_h, dpi);
    let area_w = work.right - work.left;
    let area_h = work.bottom - work.top;
    let x = work.left + (area_w - w).max(0) / 2;
    // 窗口过高时保证底边不超出工作区。
    let y = (work.top + area_h / 4).min(work.bottom - h).max(work.top);
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_SHOWWINDOW);
    }
}
