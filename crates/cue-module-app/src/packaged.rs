//! UWP/MSIX 发现:PackageManager → GetAppListEntriesAsync → AppListEntry。
//!
//! **Package ≠ App**:枚举单位是 AppListEntry(一个 package 可含 0..n 个
//! application);不解析 manifest,不走 shell:AppsFolder(脏数据)。
//!
//! §134:发现同时产出 AUMID → AppListEntry 索引,交给图标管线用
//! `DisplayInfo.GetLogo` 提取真实 logo——枚举只有一次,logo 惰性提取。

use crate::catalog::{AppEntry, LaunchTarget};
use cue_protocol::{LogLevel, ModuleLogger};
use cue_util_win::com::ComGuard;
use std::collections::HashMap;
use windows::ApplicationModel::Core::AppListEntry;
use windows::Management::Deployment::PackageManager;
use windows::core::HSTRING;

/// 发现产物:catalog 条目 + 图标管线用的 AUMID → AppListEntry 索引。
pub struct PackagedDiscovery {
    pub entries: Vec<AppEntry>,
    pub logo_index: HashMap<String, AppListEntry>,
}

pub fn discover(logger: &ModuleLogger) -> PackagedDiscovery {
    let _com = ComGuard::new();
    match discover_inner() {
        Ok((entries, logo_index)) => {
            logger.log(
                LogLevel::Info,
                &format!("packaged: {} entries", entries.len()),
            );
            PackagedDiscovery {
                entries,
                logo_index,
            }
        }
        Err(e) => {
            // WinRT 可用性属于环境事实,不构成 load 失败。
            logger.log(
                LogLevel::Warn,
                &format!("packaged discovery unavailable: {e}"),
            );
            PackagedDiscovery {
                entries: Vec::new(),
                logo_index: HashMap::new(),
            }
        }
    }
}

fn discover_inner() -> Result<(Vec<AppEntry>, HashMap<String, AppListEntry>), String> {
    let mgr = PackageManager::new().map_err(|e| e.to_string())?;
    // 空串 = 当前用户。实测(Win11 26200)无参 FindPackages() 可整个调用
    // 抛 E_ACCESSDENIED(枚举碰到个别 ACL 异常的注册即失败,全模块零
    // packaged 条目);按 SID 的 overload 走另一代码路径,同机正常。
    // (windows crate 把 WinRT 的 FindPackagesForUser 重命名为
    // FindPackagesByUserSecurityId。)
    let packages = mgr
        .FindPackagesByUserSecurityId(&HSTRING::new())
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut logo_index = HashMap::new();
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
            logo_index.insert(aumid.clone(), entry);
            out.push(AppEntry::new(
                &name,
                LaunchTarget::Packaged {
                    aumid: aumid.into(),
                },
            ));
        }
    }
    Ok((out, logo_index))
}
