//! UWP/MSIX 发现:PackageManager → GetAppListEntriesAsync → AppListEntry。
//!
//! **Package ≠ App**:枚举单位是 AppListEntry(一个 package 可含 0..n 个
//! application);不解析 manifest,不走 shell:AppsFolder(脏数据)。
//!
//! §134:发现同时产出 AUMID → AppListEntry 索引,交给图标管线用
//! `DisplayInfo.GetLogo` 提取真实 logo——枚举只有一次,logo 惰性提取。

use crate::catalog::{AppEntry, LaunchTarget};
use sakana_protocol::{LogLevel, ModuleLogger};
use sakana_util_win::com::ComGuard;
use std::collections::HashMap;
use windows::ApplicationModel::Core::AppListEntry;
use windows::Management::Deployment::PackageManager;
use windows::core::HSTRING;

/// 发现产物:catalog 条目 + 图标管线用的 AUMID → AppListEntry 索引
/// + AUMID → 包版本号表(§137 图标磁盘缓存的失效指纹)。
pub struct PackagedDiscovery {
    pub entries: Vec<AppEntry>,
    pub logo_index: HashMap<String, AppListEntry>,
    pub versions: HashMap<String, u64>,
}

pub fn discover(logger: &ModuleLogger) -> PackagedDiscovery {
    let _com = ComGuard::new();
    match discover_inner() {
        Ok((entries, logo_index, versions)) => {
            logger.log(
                LogLevel::Info,
                &format!("packaged: {} entries", entries.len()),
            );
            PackagedDiscovery {
                entries,
                logo_index,
                versions,
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
                versions: HashMap::new(),
            }
        }
    }
}

/// discover_inner 的产物三元组:catalog 条目、logo 索引、版本号表。
type PackagedParts = (
    Vec<AppEntry>,
    HashMap<String, AppListEntry>,
    HashMap<String, u64>,
);

fn discover_inner() -> Result<PackagedParts, String> {
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
    let mut versions = HashMap::new();
    for package in packages {
        // §137:包版本号是图标磁盘缓存的失效指纹(包更新 → 版本变 →
        // 旧缓存失效重建)。取不到的条目不缓存,现用现提。
        let version = package
            .Id()
            .and_then(|id| id.Version())
            .ok()
            .map(|v| crate::icon_cache::pack_version(v.Major, v.Minor, v.Build, v.Revision));
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
            if let Some(v) = version {
                versions.insert(aumid.clone(), v);
            }
            out.push(AppEntry::new(
                &name,
                LaunchTarget::Packaged {
                    aumid: aumid.into(),
                },
            ));
        }
    }
    Ok((out, logo_index, versions))
}
