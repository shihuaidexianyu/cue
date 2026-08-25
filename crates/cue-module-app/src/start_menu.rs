//! User / Common Start Menu 的 .lnk 发现。不扫盘找 exe。
//!
//! shell 的 All Apps = Start Menu 根目录直属快捷方式 + Programs 子树;
//! 有安装器(如 Inno 系,UniGetUI)会把 lnk 直接放进根目录,只扫
//! Programs 会漏掉这一类(实机 bug 报告)。

use crate::catalog::{AppEntry, LaunchTarget};
use cue_protocol::{LogLevel, ModuleLogger};
use cue_util_win::com::ComGuard;
use std::path::{Path, PathBuf};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{Interface, PCWSTR};

/// 枚举 Start Menu 根(直属文件)+ 两个 Programs 子树(递归),解析全部
/// .lnk,返回未去重的 entry。单个 lnk 失败只跳过并计数——外部数据永不 panic。
pub fn discover(logger: &ModuleLogger) -> Vec<AppEntry> {
    let _com = ComGuard::new();
    let mut out = Vec::new();
    let mut skipped = 0u32;
    for (root, recursive) in start_menu_roots() {
        let mut links = Vec::new();
        collect_lnk(&root, recursive, &mut links);
        for lnk in links {
            match resolve(&lnk) {
                Some((name, target)) => out.push(AppEntry::new(&name, target)),
                None => skipped += 1,
            }
        }
    }
    logger.log(
        LogLevel::Info,
        &format!("start menu: {} entries, {skipped} links skipped", out.len()),
    );
    out
}

/// (目录, 是否递归)。Programs 子树递归;根目录只收直属文件
/// (递归根目录会重复遍历 Programs 子树)。
fn start_menu_roots() -> Vec<(PathBuf, bool)> {
    let mut roots = Vec::new();
    for var in ["APPDATA", "ProgramData"] {
        if let Ok(p) = std::env::var(var) {
            let start_menu = PathBuf::from(p).join(r"Microsoft\Windows\Start Menu");
            roots.push((start_menu.join("Programs"), true));
            roots.push((start_menu, false));
        }
    }
    roots
}

fn collect_lnk(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                collect_lnk(&path, recursive, out);
            }
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
        {
            out.push(path);
        }
    }
}

/// 显示名 = .lnk 文件名去扩展名。卸载入口不算应用。
fn display_name(lnk: &Path) -> Option<String> {
    let name = lnk.file_stem()?.to_string_lossy().into_owned();
    let lower = name.to_lowercase();
    if lower.contains("uninstall") || name.contains("卸载") {
        return None;
    }
    Some(name)
}

fn resolve(lnk: &Path) -> Option<(String, LaunchTarget)> {
    let name = display_name(lnk)?;
    let wide: Vec<u16> = lnk
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        persist.Load(PCWSTR(wide.as_ptr()), STGM(0)).ok()?;

        let mut path_buf = [0u16; 520];
        link.GetPath(&mut path_buf, std::ptr::null_mut(), 0).ok()?;
        let exe = from_wide(&path_buf);
        if exe.is_empty() {
            return None;
        }
        let exe = PathBuf::from(exe);
        // V1 只收 exe 目标;文档 / URL / 文件夹链接不属于 AppModule。
        if !exe
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        {
            return None;
        }
        if !exe.exists() {
            return None;
        }

        let mut args_buf = [0u16; 1024];
        let mut dir_buf = [0u16; 520];
        let _ = link.GetArguments(&mut args_buf);
        let _ = link.GetWorkingDirectory(&mut dir_buf);
        let args: String = from_wide(&args_buf);
        let dir = from_wide(&dir_buf);

        Some((
            name,
            LaunchTarget::Win32 {
                exe,
                args: args.into(),
                working_dir: if dir.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(dir))
                },
            },
        ))
    }
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}
