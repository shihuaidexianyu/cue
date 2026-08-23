//! COM 初始化 guard:发现(Start Menu / Packaged)与图标 worker 共用。
//!
//! 已初始化(S_FALSE)或模式冲突(RPC_E_CHANGED_MODE)都不算失败——
//! 但 MSDN 要求**每次成功调用(含 S_FALSE)都配对 CoUninitialize**;
//! 只有失败(RPC_E_CHANGED_MODE,沿用现有 apartment)才不解引用。

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

pub struct ComGuard(bool);

impl ComGuard {
    pub fn new() -> Self {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            Self(hr.is_ok())
        }
    }
}

impl Default for ComGuard {
    fn default() -> Self {
        Self::new()
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::S_OK;

    /// 外层已初始化时,guard 拿到 S_FALSE;drop 后引用计数必须回到
    /// 进入前的水平 —— 否则外层最终的 CoUninitialize 之后线程仍挂着
    /// 一次初始化,下一次 CoInitializeEx 会返回 S_FALSE 而非 S_OK。
    #[test]
    fn s_false_is_balanced() {
        unsafe {
            let outer = CoInitializeEx(None, COINIT_MULTITHREADED);
            assert_eq!(outer, S_OK);
            {
                let _guard = ComGuard::new();
            }
            CoUninitialize();
            let again = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoUninitialize();
            assert_eq!(again, S_OK, "S_FALSE 路径漏了一次 CoUninitialize");
        }
    }
}
