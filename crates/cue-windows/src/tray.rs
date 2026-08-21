//! §116 托盘图标:进程存活的唯一常驻可见信号,V1 唯一的退出路径。
//!
//! 图标在运行时生成(32×32 纯色圆角方块,无需资源文件管线);
//! 左键唤起,右键菜单"显示 / 退出"(§116 不为托盘做更多)。

use crate::host::{HostMsg, WM_CUE_TRAY, WM_CUE_TRAY_CMD};
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::{w, Error};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const TRAY_UID: u32 = 1;
const TRAY_CMD_SHOW: usize = 1;
const TRAY_CMD_SETTINGS: usize = 2;
const TRAY_CMD_QUIT: usize = 3;

/// 已挂图标的 host hwnd;退出时据此 NIM_DELETE(§116 不留幽灵图标)。
static TRAY_HOST: AtomicIsize = AtomicIsize::new(0);

pub fn add(host: HWND) -> Result<(), Error> {
    let icon = build_icon()?;
    let mut tip = [0u16; 128];
    for (i, c) in "CUE".encode_utf16().enumerate() {
        tip[i] = c;
    }
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: host,
        uID: TRAY_UID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_CUE_TRAY,
        hIcon: icon,
        szTip: tip,
        ..Default::default()
    };
    unsafe {
        if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
            return Err(Error::from_thread());
        }
    }
    TRAY_HOST.store(host.0 as isize, Ordering::SeqCst);
    Ok(())
}

/// 退出路径必须调用;进程异常退出时系统会在下次悬停时清理,尽力而为即可。
pub fn remove() {
    let host = TRAY_HOST.swap(0, Ordering::SeqCst);
    if host == 0 {
        return;
    }
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: HWND(host as *mut core::ffi::c_void),
        uID: TRAY_UID,
        ..Default::default()
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

/// host WndProc 在 WM_CUE_TRAY 时调用。未设 NOTIFYICON_VERSION_4,
/// lparam 直接是鼠标消息。
pub fn handle_message(host: HWND, lparam: LPARAM, handler: &dyn Fn(HostMsg)) {
    match lparam.0 as u32 {
        WM_LBUTTONUP => handler(HostMsg::ShowRequested),
        WM_RBUTTONUP => unsafe { show_menu(host) },
        _ => {}
    }
}

unsafe fn show_menu(host: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        let _ = AppendMenuW(menu, MF_STRING, TRAY_CMD_SHOW, w!("显示 CUE"));
        let _ = AppendMenuW(menu, MF_STRING, TRAY_CMD_SETTINGS, w!("设置"));
        let _ = AppendMenuW(menu, MF_STRING, TRAY_CMD_QUIT, w!("退出 CUE"));
        // MSDN 托盘菜单模式:弹出前把 owner 设为前台,
        // 否则点击别处菜单不收起。owner 需为普通窗口(§116)。
        let _ = SetForegroundWindow(host);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            None,
            host,
            None,
        );
        let _ = PostMessageW(Some(host), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        if cmd.as_bool() {
            // 延迟分发(见 WM_CUE_TRAY_CMD 注释):此刻同步调 handler
            // 会让刚唤起的窗口在菜单拆除时被抢前台。
            let _ = PostMessageW(Some(host), WM_CUE_TRAY_CMD, WPARAM(cmd.0 as usize), LPARAM(0));
        }
    }
}

/// WM_CUE_TRAY_CMD 的 wParam → HostMsg(host WndProc 侧分发)。
pub fn msg_from_cmd(cmd: usize) -> Option<HostMsg> {
    match cmd {
        TRAY_CMD_SHOW => Some(HostMsg::ShowRequested),
        TRAY_CMD_SETTINGS => Some(HostMsg::OpenSettings),
        TRAY_CMD_QUIT => Some(HostMsg::QuitRequested),
        _ => None,
    }
}

/// 运行时生成 32×32 图标:accent 色圆角方块。返回 HICON。
///
/// `CreateIconFromResourceEx` 吃的是 RT_ICON 资源位
/// (BITMAPINFOHEADER + XOR + AND mask),**不含** .ico 文件的
/// ICONDIR/ICONDIRENTRY 目录头——那是 RT_GROUP_ICON 的事,
/// 带上它函数会把目录头误读成位图头。
fn build_icon() -> Result<HICON, Error> {
    const S: usize = 32;
    const XOR: usize = S * S * 4;
    const AND: usize = S * S / 8;

    // accent 蓝(BGRA 字节序)
    let (b, g, r) = (0xFFu8, 0x8Du8, 0x4Cu8);
    let radius = 7.0f32;

    let mut xor = vec![0u8; XOR];
    for y in 0..S {
        for x in 0..S {
            // 圆角矩形 SDF:超过半径的角部像素留透明
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let qx = (fx - S as f32 / 2.0).abs() - (S as f32 / 2.0 - radius);
            let qy = (fy - S as f32 / 2.0).abs() - (S as f32 / 2.0 - radius);
            if qx.max(qy) <= radius {
                let o = (y * S + x) * 4;
                xor[o] = b;
                xor[o + 1] = g;
                xor[o + 2] = r;
                xor[o + 3] = 0xFF;
            }
        }
    }

    let mut bits = Vec::with_capacity(40 + XOR + AND);
    // BITMAPINFOHEADER(biHeight = 2S:XOR + AND;位图自下而上存储)
    bits.extend_from_slice(&40u32.to_le_bytes());
    bits.extend_from_slice(&(S as i32).to_le_bytes());
    bits.extend_from_slice(&((S * 2) as i32).to_le_bytes());
    bits.extend_from_slice(&1u16.to_le_bytes()); // planes
    bits.extend_from_slice(&32u16.to_le_bytes()); // bitcount
    bits.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    bits.extend_from_slice(&((XOR + AND) as u32).to_le_bytes());
    bits.extend_from_slice(&[0u8; 16]); // 分辨率 + 调色板
    for y in (0..S).rev() {
        bits.extend_from_slice(&xor[y * S * 4..(y + 1) * S * 4]);
    }
    // AND mask:全 0 = 全不透明(角部透明由 XOR 的 alpha 表达)
    bits.extend_from_slice(&[0u8; AND]);

    unsafe { CreateIconFromResourceEx(&bits, true, 0x00030000, S as i32, S as i32, LR_DEFAULTCOLOR) }
}
