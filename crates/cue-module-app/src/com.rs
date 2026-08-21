//! COM 初始化 guard:发现(Start Menu / Packaged)与图标 worker 共用。
//!
//! 已初始化(S_FALSE)或模式冲突(RPC_E_CHANGED_MODE)都不算失败——
//! 前者不解引用,后者沿用现有 apartment(IShellLink 在 STA 同样可用)。

use windows::Win32::Foundation::S_OK;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

pub struct ComGuard(bool);

impl ComGuard {
    pub fn new() -> Self {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            Self(hr == S_OK)
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}
