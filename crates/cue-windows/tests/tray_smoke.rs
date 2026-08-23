//! tray::add / remove 回归测试(Win32 托盘路径,CI 无法肉眼验证,
//! 至少保证 NIM_ADD 被系统接受、roundtrip 不 panic)。
//! 运行:cargo test -p cue-windows --test tray_smoke

use cue_windows::tray;
use std::mem::size_of;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::NOTIFYICONDATAW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

#[test]
fn tray_add_remove_roundtrip() {
    // x64 下 Vista+ 完整结构体大小;cbSize 错了 NIM_ADD 会静默拒绝。
    assert_eq!(size_of::<NOTIFYICONDATAW>(), 976);
    unsafe {
        let hinstance = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            lpszClassName: w!("CUE.TraySmoke"),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("CUE.TraySmoke"),
            w!("smoke"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
        .unwrap();

        tray::add(hwnd).expect("NIM_ADD must be accepted");
        tray::remove();
        let _ = DestroyWindow(hwnd);
    }
}
