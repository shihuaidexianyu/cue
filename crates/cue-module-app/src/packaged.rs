//! UWP/MSIX 发现(§29):PackageManager → GetAppListEntriesAsync → AppListEntry。
//!
//! **Package ≠ App**:枚举单位是 AppListEntry(一个 package 可含 0..n 个
//! application);不解析 manifest,不走 shell:AppsFolder(脏数据)。

use crate::catalog::{AppEntry, LaunchTarget};
use cue_protocol::{LogLevel, ModuleLogger};
use cue_util_win::com::ComGuard;
use windows::Management::Deployment::PackageManager;

pub fn discover(logger: &ModuleLogger) -> Vec<AppEntry> {
    let _com = ComGuard::new();
    match discover_inner() {
        Ok(entries) => {
            logger.log(
                LogLevel::Info,
                &format!("packaged: {} entries", entries.len()),
            );
            entries
        }
        Err(e) => {
            // WinRT 可用性属于环境事实,不构成 load 失败(§63)。
            logger.log(
                LogLevel::Warn,
                &format!("packaged discovery unavailable: {e}"),
            );
            Vec::new()
        }
    }
}

fn discover_inner() -> Result<Vec<AppEntry>, String> {
    let mgr = PackageManager::new().map_err(|e| e.to_string())?;
    let packages = mgr.FindPackages().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for package in packages {
        let Ok(op) = package.GetAppListEntriesAsync() else {
            continue;
        };
        let Ok(entries) = op.join() else {
            continue;
        };
        for entry in entries {
            let Ok(aumid) = entry.AppUserModelId() else {
                continue;
            };
            let name = entry
                .DisplayInfo()
                .and_then(|d| d.DisplayName())
                .map(|h| h.to_string_lossy())
                .unwrap_or_default();
            // 资源引用未解析的条目("ms-resource:...")展示不出名字,跳过。
            if name.is_empty() || name.starts_with("ms-resource:") {
                continue;
            }
            let aumid = aumid.to_string_lossy();
            out.push(AppEntry::new(
                &name,
                LaunchTarget::Packaged {
                    aumid: aumid.into(),
                },
            ));
        }
    }
    Ok(out)
}
