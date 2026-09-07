//! 显示器放置:显示在当前用户活跃 monitor。

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromWindow,
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
        if dpi_x == 0 { 96 } else { dpi_x }
    }
}

/// 逻辑尺寸 → 物理像素(按目标 DPI,四舍五入)。
fn logical_to_physical(logical: i32, dpi: u32) -> i32 {
    (logical * dpi as i32 + 48) / 96
}

/// 量取窗口矩形与客户区的差(DWM 不可见缩放边框),返回
/// (左边框, 上边框, 总宽差, 总高差)。必须在窗口已落到目标显示器
/// 之后调用——边框厚度随 DPI 缩放,跨显示器移动前的值是错的。
/// 失败时全零回退(等价于旧行为:按窗口矩形设定尺寸)。
fn frame_margins(hwnd: HWND) -> (i32, i32, i32, i32) {
    unsafe {
        let mut win = RECT::default();
        let mut client = RECT::default();
        if GetWindowRect(hwnd, &mut win).is_err() || GetClientRect(hwnd, &mut client).is_err() {
            return (0, 0, 0, 0);
        }
        let mut origin = POINT::default();
        let _ = ClientToScreen(hwnd, &mut origin);
        let border_left = origin.x - win.left;
        let border_top = origin.y - win.top;
        let frame_w = (win.right - win.left) - (client.right - client.left);
        let frame_h = (win.bottom - win.top) - (client.bottom - client.top);
        (border_left, border_top, frame_w, frame_h)
    }
}

/// 把 Launcher 窗口放置到活跃显示器:水平居中,垂直约 1/4 处。
///
/// 进程是 PerMonitorV2(见 exe manifest),`SetWindowPos` 吃**物理**
/// 像素:尺寸按目标显示器 DPI 由设计逻辑尺寸换算,保证任意缩放下
/// 都是同一逻辑大小。尺寸必须显式设置——GPUI 以 `show: false` 创建
/// 窗口时用 `CW_USEDEFAULT`,请求的尺寸只存在它内部的
/// `initial_placement` 里、仅由 GPUI 自己的 `activate()` 补设;
/// 我们走原生 `ShowWindow` 唤起,那条路径永远不会执行。
///
/// **两步 SetWindowPos,杜绝跨 DPI 双重缩放**:第一步移动+显示但
/// 不带尺寸——窗口跨入不同 DPI 显示器时,Windows 在此同步派发
/// WM_DPICHANGED,GPUI 会把 suggested rect(旧尺寸 × 缩放比)
/// 应用上去;第二步在同一显示器上设最终尺寸,不再触发
/// WM_DPICHANGED,尺寸定音。单次带尺寸调用时,suggested rect
/// 会把我们已按目标 DPI 换算好的尺寸再缩一次——这正是
/// "首次唤起尺寸错、第二次才正常"的根因。
///
/// 两步都带 SWP_NOACTIVATE:放置阶段不抢激活——调用方的唤起
/// 序列是 place → enter_english_mode → show_and_focus,焦点由
/// 最后的 show_and_focus 显式取得(IME 布局切换须在聚焦前完成)。
///
/// **第二步按客户区下尺寸**:SetWindowPos 的尺寸是窗口矩形,包含
/// DWM 不可见缩放边框(150% 缩放下宽 22px、高 13px);直接喂
/// w×h 会让客户区缩水成 938×662,5 行结果时内容超出客户区
/// 2.5px,flex 收缩压矮输入行、把分割线上移并挤细(bug 3b——
/// "不足 5 条时分割线变粗下移"实为满 5 行时它被压缩)。
/// 第一步移动后窗口 DPI 已与目标显示器同步,此时量取的边框
/// 差值才是当前 DPI 下的真值;位置也改按客户区居中/定位,
/// 可见内容与旧版几乎重合(旧版客户端偏上约 4px)。
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
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
        // 第一步已完成 DPI 同步,量取真实边框,把客户区定为 w×h。
        let (border_left, border_top, frame_w, frame_h) = frame_margins(hwnd);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x - border_left,
            y - border_top,
            w + frame_w,
            h + frame_h,
            SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
    }
}
