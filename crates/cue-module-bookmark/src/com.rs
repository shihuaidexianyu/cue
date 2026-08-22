//! COM 初始化 guard。复制自 cue-module-app(Rule of Three 第二次使用,
//! §72 允许重复;第三个消费者落地时下沉 util crate)。
//!
//! 已初始化(S_FALSE)或模式冲突(RPC_E_CHANGED_MODE)都不算失败——
//! 但 MSDN 要求**每次成功调用(含 S_FALSE)都配对 CoUninitialize**;
//! 只有失败(RPC_E_CHANGED_MODE,沿用现有 apartment)才不解引用。

use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

pub struct ComGuard(bool);

impl ComGuard {
    pub fn new() -> Self {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            Self(hr.is_ok())
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::S_OK;

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
