//! 一次性写剪贴板(§18 "Copy path" / "Copy link"):OpenClipboard →
//! EmptyClipboard → SetClipboardData(CF_UNICODETEXT)。这是单次写入,
//! 不是 §76 明确不做的 clipboard manager(历史/监听)。

use cue_protocol::ModuleError;
use windows::Win32::Foundation::{GlobalFree, HANDLE};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// 把 `text` 放进系统剪贴板(覆盖既有内容)。剪贴板被别的程序短暂
/// 占用时 OpenClipboard 失败——不重试,错误横幅展示后用户可再触发。
pub fn set_text(text: &str) -> Result<(), ModuleError> {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        OpenClipboard(None)
            .map_err(|e| ModuleError::ActivationFailed(format!("OpenClipboard: {e}")))?;
        let result = fill(&wide);
        let _ = CloseClipboard();
        result
    }
}

unsafe fn fill(wide: &[u16]) -> Result<(), ModuleError> {
    unsafe {
        EmptyClipboard()
            .map_err(|e| ModuleError::ActivationFailed(format!("EmptyClipboard: {e}")))?;
        let hmem = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of_val(wide))
            .map_err(|e| ModuleError::ActivationFailed(format!("GlobalAlloc: {e}")))?;
        let dst = GlobalLock(hmem);
        if dst.is_null() {
            let _ = GlobalFree(Some(hmem));
            return Err(ModuleError::ActivationFailed("GlobalLock failed".into()));
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dst as *mut u16, wide.len());
        let _ = GlobalUnlock(hmem);
        // SetClipboardData 成功后内存所有权移交系统;失败则自行释放。
        if let Err(e) = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hmem.0))) {
            let _ = GlobalFree(Some(hmem));
            return Err(ModuleError::ActivationFailed(format!(
                "SetClipboardData: {e}"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::DataExchange::{GetClipboardData, IsClipboardFormatAvailable};

    /// 真机写一次再读回:内容逐字相等,且剪贴板在函数返回后已可被
    /// 本进程再次打开(句柄无泄漏)。
    #[test]
    fn set_text_round_trips_unicode() {
        let text = "CUE 测试 ✓ C:\\路径\\文件.txt";
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
}
