//! 一次性写剪贴板("Copy path" / "Copy link"):OpenClipboard →
//! EmptyClipboard → SetClipboardData(CF_UNICODETEXT)。这是单次写入,
//! 不做 clipboard manager(历史/监听)。

use sakana_protocol::ModuleError;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::w;

struct Memory(HGLOBAL);
impl Drop for Memory {
    fn drop(&mut self) {
        unsafe {
            let _ = GlobalFree(Some(self.0));
        }
    }
}

struct Owner(HWND);
impl Owner {
    fn new() -> Result<Self, ModuleError> {
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!(""),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .map(Self)
            .map_err(|e| ModuleError::ActivationFailed(format!("clipboard owner: {e}")))
        }
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

struct Opened;
impl Drop for Opened {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// 把 `text` 放进系统剪贴板(覆盖既有内容)。剪贴板被别的程序短暂
/// 占用时 OpenClipboard 失败——不重试,错误横幅展示后用户可再触发。
pub fn set_text(text: &str) -> Result<(), ModuleError> {
    let wide = crate::shell::to_wide(text);
    unsafe {
        // 先准备内存;分配失败时不能破坏用户当前剪贴板。
        let memory = prepare(&wide)?;
        // 系统 STATIC 类的 message-only 窗口,由本次调用所在的线程拥有。
        let owner = Owner::new()?;
        OpenClipboard(Some(owner.0))
            .map_err(|e| ModuleError::ActivationFailed(format!("OpenClipboard: {e}")))?;
        let _opened = Opened;
        EmptyClipboard()
            .map_err(|e| ModuleError::ActivationFailed(format!("EmptyClipboard: {e}")))?;
        SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(memory.0.0)))
            .map_err(|e| ModuleError::ActivationFailed(format!("SetClipboardData: {e}")))?;
        // 只有成功后才移交内存。析构顺序:关闭剪贴板 → 销毁 owner。
        std::mem::forget(memory);
        Ok(())
    }
}

unsafe fn prepare(wide: &[u16]) -> Result<Memory, ModuleError> {
    unsafe {
        let hmem = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of_val(wide))
            .map_err(|e| ModuleError::ActivationFailed(format!("GlobalAlloc: {e}")))?;
        let memory = Memory(hmem);
        let dst = GlobalLock(hmem);
        if dst.is_null() {
            return Err(ModuleError::ActivationFailed("GlobalLock failed".into()));
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dst as *mut u16, wide.len());
        let _ = GlobalUnlock(hmem);
        Ok(memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::DataExchange::{GetClipboardData, IsClipboardFormatAvailable};

    /// 子进程切到私有非交互 window station(有独立剪贴板),
    /// 验证真实 Win32 写入与 owner 销毁后的读回,不触碰用户剪贴板。
    /// 注意:受限环境(沙箱 / 非交互会话)里整个会话都拿不到剪贴板,
    /// OpenClipboard 会直接 ACCESS_DENIED——那不是本测试的缺陷。
    #[test]
    #[ignore = "spawns a child process on a private anonymous window station"]
    fn set_text_round_trips_unicode() {
        const CHILD: &str = "SAKANA_CLIPBOARD_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            use std::os::windows::process::CommandExt;
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "clipboard::tests::set_text_round_trips_unicode",
                    "--ignored",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        unsafe {
            use windows::Win32::Foundation::GENERIC_ALL;
            use windows::Win32::System::StationsAndDesktops::*;
            use windows::core::PCWSTR;
            // 匿名 window station:剪贴板按 window station 隔离,私有
            // station 就够。**不能传名字**——只有提权到 Administrators
            // 的令牌才允许命名 window station,普通 shell 会直接
            // ACCESS_DENIED(§142)。
            let station = CreateWindowStationW(PCWSTR::null(), 0, GENERIC_ALL.0, None).unwrap();
            SetProcessWindowStation(station).unwrap();
            // 线程的 desktop 必须属于进程的 window station,否则线程
            // 上的窗口与进程的剪贴板不在同一 station。desktop 不是隔离
            // 手段,是连通性前提;desktop 名字没有管理员限制(只有
            // window station 的名字有)。
            let desktop = CreateDesktopW(
                w!("test"),
                PCWSTR::null(),
                None,
                DESKTOP_CONTROL_FLAGS::default(),
                GENERIC_ALL.0,
                None,
            )
            .unwrap();
            SetThreadDesktop(desktop).unwrap();
            // 此处任一步失败都会终止子测试,绝不退回交互剪贴板。
            // station/desktop 保持到子进程退出,由 OS 回收。
        }
        let text = "sakana 测试 ✓ C:\\路径\\文件.txt";
        set_text(text).expect("set_text");
        unsafe {
            IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).expect("CF_UNICODETEXT available");
            OpenClipboard(None).expect("reopen");
            let hmem = GetClipboardData(CF_UNICODETEXT.0 as u32).expect("GetClipboardData");
            let ptr = GlobalLock(windows::Win32::Foundation::HGLOBAL(hmem.0));
            assert!(!ptr.is_null());
            let mut read = Vec::new();
            let mut p = ptr as *const u16;
            while *p != 0 {
                read.push(*p);
                p = p.add(1);
            }
            let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(hmem.0));
            let _ = CloseClipboard();
            assert_eq!(String::from_utf16(&read).unwrap(), text);
        }
    }

    #[test]
    fn prepares_unicode_memory_without_touching_clipboard() {
        let wide = crate::shell::to_wide("测试 ✓ C:\\路径\\文件.txt");
        unsafe {
            let memory = prepare(&wide).unwrap();
            let ptr = GlobalLock(memory.0);
            assert!(!ptr.is_null());
            assert_eq!(
                std::slice::from_raw_parts(ptr as *const u16, wide.len()),
                wide.as_slice()
            );
            let _ = GlobalUnlock(memory.0);
        }
    }

    #[test]
    fn clipboard_owner_is_a_valid_scoped_window() {
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;
        let owner = Owner::new().unwrap();
        let hwnd = owner.0;
        assert!(unsafe { IsWindow(Some(hwnd)) }.as_bool());
        drop(owner);
        assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    }
}
