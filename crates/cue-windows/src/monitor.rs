//! 显示器放置(§54):显示在当前用户活跃 monitor。

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// 以当前前台窗口所在显示器为"活跃显示器",返回其工作区。
fn active_work_area() -> (i32, i32, i32, i32) {
    unsafe {
        let foreground = GetForegroundWindow();
        let monitor: HMONITOR = MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let r = info.rcWork;
            (r.left, r.top, r.right, r.bottom)
        } else {
            // 回退:主显示器 1080p 假设。
            (0, 0, 1920, 1080)
        }
    }
}

/// 把 Launcher 窗口放置到活跃显示器:水平居中,垂直约 1/4 处。
pub fn place_on_active_monitor(hwnd: HWND, width: i32, height: i32) {
    let (left, top, right, bottom) = active_work_area();
    let area_w = right - left;
    let area_h = bottom - top;
    let w = width.min(area_w);
    let h = height.min(area_h);
    let x = left + (area_w - w) / 2;
    let y = top + area_h / 4;
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_SHOWWINDOW);
    }
}
