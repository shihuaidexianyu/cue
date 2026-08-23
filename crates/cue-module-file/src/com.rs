//! COM 初始化 guard。复制自 cue-module-bookmark(Rule of Three 第三次
//! 使用——icon/com 已达下沉 util crate 的条件,待 FileModule 落地后
//! 单独提交处理,本次保持各模块自包含、改动隔离)。

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
