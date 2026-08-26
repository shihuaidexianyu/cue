//! 注册表 App Paths 发现(HKLM + HKCU,§133)。
//!
//! `SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\<x>.exe` 的
//! 默认值是可执行文件全路径——安装器注册过但可能没有开始菜单项的
//! 应用在这里(Flow Launcher 的 Win32 发现源之二)。App Paths 键在
//! 32/64 位视图间共享,读一次即可。

use crate::catalog::{AppEntry, LaunchTarget};
use crate::start_menu::is_uninstall_entry;
use cue_protocol::{LogLevel, ModuleLogger};
use std::path::PathBuf;
use windows::Win32::Foundation::ERROR_NO_MORE_ITEMS;
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ, RegCloseKey,
    RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
};
use windows::core::{PCWSTR, PWSTR};

const APP_PATHS: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";

pub fn discover(logger: &ModuleLogger) -> Vec<AppEntry> {
    let mut out = Vec::new();
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        out.extend(enum_root(root));
    }
    logger.log(LogLevel::Info, &format!("app paths: {} entries", out.len()));
    out
}

fn enum_root(root: HKEY) -> Vec<AppEntry> {
    unsafe {
        let subkey: Vec<u16> = APP_PATHS.encode_utf16().chain(Some(0)).collect();
        let mut hk = HKEY::default();
        if RegOpenKeyExW(root, PCWSTR(subkey.as_ptr()), Some(0), KEY_READ, &mut hk).is_err() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut index = 0u32;
        loop {
            let mut name_buf = [0u16; 256];
            let mut name_len = name_buf.len() as u32;
            let r = RegEnumKeyExW(
                hk,
                index,
                Some(PWSTR(name_buf.as_mut_ptr())),
                &mut name_len,
                None,
                None,
                None,
                None,
            );
            if r == ERROR_NO_MORE_ITEMS {
                break;
            }
            index += 1;
            if r.is_err() {
                continue; // 单条失败只跳过——外部数据永不 panic(§63)
            }
            let name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            if let Some(exe) = read_default_value(hk, &name)
                && let Some(entry) = entry_from(&name, exe)
            {
                out.push(entry);
            }
        }
        let _ = RegCloseKey(hk);
        out
    }
}

/// 读子键的默认值(未命名值)= exe 全路径;REG_EXPAND_SZ 展开环境变量。
fn read_default_value(hk: HKEY, subkey_name: &str) -> Option<String> {
    unsafe {
        let wide: Vec<u16> = subkey_name.encode_utf16().chain(Some(0)).collect();
        let mut sub = HKEY::default();
        if RegOpenKeyExW(hk, PCWSTR(wide.as_ptr()), Some(0), KEY_READ, &mut sub).is_err() {
            return None;
        }
        let value = (|| {
            // 第一次调用取所需缓冲区大小。
            let mut value_type = REG_SZ;
            let mut size = 0u32;
            if RegQueryValueExW(
                sub,
                PCWSTR::null(),
                None,
                Some(&mut value_type),
                None,
                Some(&mut size),
            )
            .is_err()
            {
                return None;
            }
            if value_type != REG_SZ && value_type != REG_EXPAND_SZ {
                return None;
            }
            let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
            if RegQueryValueExW(
                sub,
                PCWSTR::null(),
                None,
                None,
                Some(buf.as_mut_ptr() as *mut u8),
                Some(&mut size),
            )
            .is_err()
            {
                return None;
            }
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let raw = String::from_utf16_lossy(&buf[..end]);
            if value_type == REG_EXPAND_SZ && raw.contains('%') {
                let src: Vec<u16> = raw.encode_utf16().chain(Some(0)).collect();
                let need = ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), None);
                if need == 0 {
                    return None;
                }
                let mut dst = vec![0u16; need as usize];
                if ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(&mut dst)) == 0 {
                    return None;
                }
                let end = dst.iter().position(|&c| c == 0).unwrap_or(dst.len());
                Some(String::from_utf16_lossy(&dst[..end]))
            } else {
                Some(raw)
            }
        })();
        let _ = RegCloseKey(sub);
        value
    }
}

/// 显示名 = 子键名去 ".exe"(注册表里没有更漂亮的可读名)。
fn entry_from(subkey_name: &str, raw_path: String) -> Option<AppEntry> {
    let path = raw_path.trim().trim_matches('"');
    let exe = PathBuf::from(path);
    if !exe
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return None;
    }
    if !exe.exists() {
        return None; // 卸载残留的陈旧注册
    }
    let name = subkey_name
        .strip_suffix(".exe")
        .or_else(|| subkey_name.strip_suffix(".EXE"))
        .unwrap_or(subkey_name);
    if name.is_empty() || is_uninstall_entry(name) {
        return None;
    }
    Some(AppEntry::new(
        name,
        LaunchTarget::Win32 {
            working_dir: exe.parent().map(|p| p.to_path_buf()),
            exe,
            args: "".into(),
        },
    ))
}
