//! 用户声明的便携应用目录(§133):`module.app.extra_dirs` 设置,
//! 分号分隔,递归浅扫 *.exe / *.lnk。Flow Launcher 的 Program
//! Sources 对应物——Throne 这类无安装器、无注册、无开始菜单项的
//! 便携应用由此进入 catalog。
//!
//! catalog 只在进程启动时构建一次(§56 无 watcher),所以设置改动
//! 走 RestartApplication——新目录重启后生效,与"新装应用重启后
//! 出现"的既有纪律一致。

use crate::catalog::{AppEntry, LaunchTarget};
use crate::start_menu::{is_uninstall_entry, resolve_lnk};
use cue_protocol::{LogLevel, ModuleLogger};
use cue_util_win::com::ComGuard;
use std::path::{Path, PathBuf};

/// 递归上限:便携目录天然扁平(Throne = ~\Apps\Throne\Throne\ 深 3
/// 层);防用户误指 ~\、C:\ 这类广域根时扫穿整棵树。
const MAX_DEPTH: u32 = 4;

pub fn parse_dirs(setting: &str) -> Vec<PathBuf> {
    setting
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

pub fn discover(dirs: &[PathBuf], logger: &ModuleLogger) -> Vec<AppEntry> {
    if dirs.is_empty() {
        return Vec::new();
    }
    let _com = ComGuard::new(); // .lnk 解析走 COM(ShellLink)
    let mut out = Vec::new();
    for dir in dirs {
        if !dir.is_dir() {
            logger.log(
                LogLevel::Warn,
                &format!("extra_dirs: 目录不存在,跳过 {}", dir.display()),
            );
            continue;
        }
        scan(dir, 0, &mut out);
    }
    logger.log(
        LogLevel::Info,
        &format!("extra dirs: {} entries", out.len()),
    );
    out
}

fn scan(dir: &Path, depth: u32, out: &mut Vec<AppEntry>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        if is_hidden(&entry) {
            continue; // 隐藏项不进(误指 ~\ 时 AppData 之类不炸列表)
        }
        let path = entry.path();
        if path.is_dir() {
            scan(&path, depth + 1, out);
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case("lnk") {
            if let Some((name, target)) = resolve_lnk(&path) {
                out.push(AppEntry::new(&name, target));
            }
        } else if ext.eq_ignore_ascii_case("exe") {
            let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            if is_uninstall_entry(&name) {
                continue;
            }
            out.push(AppEntry::new(
                &name,
                LaunchTarget::Win32 {
                    working_dir: path.parent().map(|p| p.to_path_buf()),
                    exe: path,
                    args: "".into(),
                },
            ));
        }
    }
}

fn is_hidden(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    entry
        .metadata()
        .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dirs_splits_and_trims() {
        let dirs = parse_dirs(r" C:\Apps ;D:\Tools;; ");
        assert_eq!(
            dirs,
            [PathBuf::from(r"C:\Apps"), PathBuf::from(r"D:\Tools")]
        );
        assert!(parse_dirs("").is_empty());
        assert!(parse_dirs(" ; ").is_empty());
    }
}
