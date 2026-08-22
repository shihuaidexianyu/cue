//! 品牌图标:从 exe 内嵌资源(id 1,由 crates/cue/build.rs 经
//! embed-resource 从 assets/cue.ico 写入)取 HICON。托盘、Launcher
//! 窗口(alt-tab/任务栏)、exe 文件图标同源。
//!
//! 多尺寸 ICO 资源由系统按请求尺寸挑最近条目,避免模糊缩放。
//! 资源缺失(如不带资源的测试宿主)返回 None,调用方走兜底。

use windows::core::PCWSTR;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{LoadImageW, HICON, IMAGE_ICON, LR_DEFAULTCOLOR};

/// 与 crates/cue/cue.rc 的资源 id 一致。
const BRAND_ICON_ID: usize = 1;

/// 按 (cx, cy) 请求尺寸加载品牌图标。非 LR_SHARED:句柄为私有副本,
/// 随进程生命周期持有即可(托盘/窗口图标本就与进程同寿)。
pub fn brand_icon(cx: i32, cy: i32) -> Option<HICON> {
    unsafe {
        let hmod = GetModuleHandleW(None).ok()?;
        LoadImageW(
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            PCWSTR(BRAND_ICON_ID as *const u16), // MAKEINTRESOURCEW
            IMAGE_ICON,
            cx,
            cy,
            LR_DEFAULTCOLOR,
        )
        .ok()
        .map(|h| HICON(h.0))
    }
}
