//! 开机自启(登录启动项):HKCU 下 Run 键的 `CUE` 值。
//!
//! 每用户安装、无需管理员权限;只写当前用户 hive,卸载/关闭时
//! 删除同名值。core.start_on_boot 设置的事务回调由编排层注入。

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Registry::*;
use windows::core::w;

const RUN_SUBKEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: windows::core::PCWSTR = w!("CUE");

/// 开/关登录启动项。值内容为带引号的 exe 绝对路径(防空格截断)。
pub fn set_enabled(enable: bool, exe: &Path) -> Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        let err = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_SUBKEY, None, KEY_SET_VALUE, &mut key);
        if !err.is_ok() {
            return Err(format!("open Run key failed: {err:?}"));
        }
        let result = if enable {
            // 路径不经 to_string_lossy:非 UTF-8 路径会被换成 U+FFFD。
            // 引号防路径含空格时被命令行解析截断。
            let wide: Vec<u16> = [0x0022u16] // '"'
                .into_iter()
                .chain(exe.as_os_str().encode_wide())
                .chain([0x0022, 0])
                .collect();
            let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
            RegSetValueExW(key, VALUE_NAME, None, REG_SZ, Some(bytes))
        } else {
            let err = RegDeleteValueW(key, VALUE_NAME);
            // 值本就不存在 = 目标状态已达成,不算失败。
            if err == ERROR_FILE_NOT_FOUND {
                WIN32_ERROR(0)
            } else {
                err
            }
        };
        let _ = RegCloseKey(key);
        if result.is_ok() {
            Ok(())
        } else {
            Err(format!("write Run value failed: {result:?}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真注册表往返:enable → 读到带引号路径 → disable → 值消失。
    /// 写真实 HKCU Run 键,故默认 ignore,手动跑:
    /// `cargo test -p cue-windows -- --ignored`
    #[test]
    #[ignore]
    fn registry_roundtrip() {
        let exe = Path::new(r"C:\Program Files\CUE\cue.exe");
        set_enabled(true, exe).unwrap();
        unsafe {
            let mut key = HKEY::default();
            assert!(
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    RUN_SUBKEY,
                    None,
                    KEY_QUERY_VALUE,
                    &mut key
                )
                .is_ok()
            );
            let mut ty = REG_VALUE_TYPE::default();
            let mut buf = [0u8; 512];
            let mut len = buf.len() as u32;
            let err = RegQueryValueExW(
                key,
                VALUE_NAME,
                None,
                Some(&mut ty),
                Some(buf.as_mut_ptr()),
                Some(&mut len),
            );
            let _ = RegCloseKey(key);
            assert!(err.is_ok());
            assert_eq!(ty, REG_SZ);
            let wide = std::slice::from_raw_parts(buf.as_ptr() as *const u16, (len as usize) / 2);
            let text = String::from_utf16_lossy(wide);
            assert_eq!(
                text.trim_end_matches('\0'),
                "\"C:\\Program Files\\CUE\\cue.exe\""
            );
        }
        set_enabled(false, exe).unwrap();
        unsafe {
            let mut key = HKEY::default();
            assert!(
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    RUN_SUBKEY,
                    None,
                    KEY_QUERY_VALUE,
                    &mut key
                )
                .is_ok()
            );
            let err = RegQueryValueExW(key, VALUE_NAME, None, None, None, None);
            let _ = RegCloseKey(key);
            assert_eq!(err, ERROR_FILE_NOT_FOUND);
        }
    }
}
