//! cue-module-app —— AppModule。
//!
//! V1 唯一必装模块:User/Common Start Menu + UWP/MSIX 发现,
//! 拼音(全拼 + 首字母)+ fuzzy 搜索,usage ranking,异步图标。
//! Core 不知道什么是 .lnk、拼音、AUMID——全部语义在本 crate。

mod catalog;
mod icon;
mod launch;
mod matcher;
mod packaged;
mod pinyin_index;
mod ready;
mod start_menu;

pub use catalog::{AppEntry, LaunchTarget};

use cue_protocol::*;
use icon::IconPipeline;
use ready::CatalogCell;
use std::sync::Arc;

/// 次级动作 ID(顺序即菜单顺序;PRIMARY = 打开)。
const ACTION_RUN_AS_ADMIN: ActionId = ActionId(1);
const ACTION_OPEN_LOCATION: ActionId = ActionId(2);

/// AppModule 是 V1 的 required default module。
pub struct AppModule {
    descriptor: ModuleDescriptor,
    /// spike:发现不满足冷启动预算,catalog 由后台线程一次性
    /// 构建(进程启动时唯一一次,无 watcher);query future 等就绪。
    catalog: Arc<CatalogCell>,
    usage: Option<UsageReader>,
    icons: Option<IconPipeline>,
}

impl AppModule {
    pub fn new() -> Self {
        Self {
            descriptor: ModuleDescriptor {
                id: ModuleId::from_static("app"),
                name: "应用",
                version: "0.1.0",
            },
            catalog: CatalogCell::new(),
            usage: None,
            icons: None,
        }
    }
}

impl Default for AppModule {
    fn default() -> Self {
        Self::new()
    }
}

/// Score = StringMatch + UsageBonus + RecencyBonus(+ AliasBonus,
/// V1 无 aliases UI,恒 0)。具体公式属于本模块。
fn usage_bonus(usage: Option<&UsageReader>, entry: &AppEntry) -> i32 {
    let Some(stat) = usage.and_then(|u| u.stat(&entry.item_key, ActionId::PRIMARY)) else {
        return 0;
    };
    let mut bonus = (stat.count as i32).min(20) * 2;
    if let Ok(elapsed) = stat.last_used.elapsed() {
        let hours = elapsed.as_secs() / 3600;
        if hours < 24 {
            bonus += 10;
        } else if hours < 24 * 7 {
            bonus += 5;
        }
    }
    bonus
}

fn search(
    entries: &[AppEntry],
    usage: Option<&UsageReader>,
    query: &str,
    limit: usize,
) -> Vec<ModuleItem> {
    if query.is_empty() {
        // 空查询 = usage Top Apps;无 usage 数据时空列表,
        // 不显示任何"推荐内容"。
        return top_used(entries, usage, limit);
    }
    let q = query.to_lowercase();
    let mut scored: Vec<(i32, &AppEntry)> = entries
        .iter()
        .filter_map(|e| {
            // name 传原始大小写:驼峰边界(VSCode 的 S/C)是词首加分
            // 信号;字符比较在 matcher 内做 ascii 小写归一。
            let keys: [&str; 3] = [&e.name, &e.pinyin_full, &e.pinyin_initials];
            matcher::best_score(&q, &keys).map(|s| (s + usage_bonus(usage, e), e))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.name_lower.cmp(&b.1.name_lower))
    });
    scored.truncate(limit);
    scored
        .into_iter()
        .map(|(_, e)| ModuleItem::new(ItemId(e.item_id()), e.clone()))
        .collect()
}

/// 空查询:按 (count, last_used) 排的 Top Apps。
fn top_used(entries: &[AppEntry], usage: Option<&UsageReader>, limit: usize) -> Vec<ModuleItem> {
    let Some(usage) = usage else {
        return Vec::new();
    };
    let mut with_stat: Vec<(&AppEntry, UsageStat)> = entries
        .iter()
        .filter_map(|e| usage.stat(&e.item_key, ActionId::PRIMARY).map(|s| (e, s)))
        .filter(|(_, s)| s.count > 0)
        .collect();
    with_stat.sort_by(|a, b| {
        b.1.count
            .cmp(&a.1.count)
            .then(b.1.last_used.cmp(&a.1.last_used))
    });
    with_stat.truncate(limit);
    with_stat
        .into_iter()
        .map(|(e, _)| ModuleItem::new(ItemId(e.item_id()), e.clone()))
        .collect()
}

impl Module for AppModule {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    /// load 只做廉价初始化——catalog 发现(Win32 COM / WinRT)
    /// 实测阻塞至秒级,不满足 冷启动预算,移入 module 自有线程
    /// 图标提取本来就不在 load 内。
    fn load(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.usage = Some(ctx.usage.clone());
        self.icons = Some(IconPipeline::new(ctx.events.clone()));

        let cell = Arc::clone(&self.catalog);
        let logger = ctx.logger.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let mut entries = start_menu::discover(&logger);
            let t_start_menu = started.elapsed();
            let n_start_menu = entries.len();
            entries.extend(packaged::discover(&logger));
            let t_packaged = started.elapsed() - t_start_menu;
            let n_packaged = entries.len() - n_start_menu;
            catalog::dedup(&mut entries);
            entries.sort_by(|a, b| a.name_lower.cmp(&b.name_lower));
            // 冷启动 spike:构建耗时就地记录。
            logger.log(
                LogLevel::Info,
                &format!(
                    "app catalog ready: {} entries ({n_start_menu} start menu, {n_packaged} packaged) in {:?} (start menu {t_start_menu:?}, packaged {t_packaged:?})",
                    entries.len(),
                    started.elapsed()
                ),
            );
            cell.set(entries);
        });
        Ok(())
    }

    fn unload(&mut self) {
        self.icons = None; // Drop 关停 worker 线程
        self.usage = None;
        // catalog cell 不重建:构建线程最多再 set 一次,幂等无害。
    }

    fn settings_schema(&self) -> SettingsSchema {
        Vec::new()
    }

    fn try_apply_settings(&mut self, _changes: SettingsChangeSet) -> Result<(), ModuleError> {
        Ok(())
    }
}

impl LauncherModule for AppModule {
    fn launcher_descriptor(&self) -> LauncherDescriptor {
        LauncherDescriptor {
            trigger: None,
            is_default: true,
        }
    }

    /// 创建 future 不触碰 IO;catalog 就绪前 future 挂起
    /// (不阻塞 UI 线程),过期完成由 Core 的 ticket 判定丢弃。
    fn query(&mut self, ctx: QueryContext) -> QueryFuture {
        let cell = Arc::clone(&self.catalog);
        let usage = self.usage.clone();
        Box::pin(async move {
            let entries = cell.wait().await;
            let items = search(&entries, usage.as_ref(), &ctx.query, ctx.result_limit);
            Ok(QueryResponse { items })
        })
    }

    fn present(&self, item: &ModuleItem) -> ResultPresentation {
        let Some(entry) = item.downcast_ref::<AppEntry>() else {
            return ResultPresentation::new("<unknown item>");
        };
        let mut p = ResultPresentation::new(entry.name.clone());
        p.subtitle = Some(
            match &entry.target {
                LaunchTarget::Win32 { exe, .. } => exe
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                LaunchTarget::Packaged { .. } => "商店应用".to_string(),
            }
            .into(),
        );
        p.icon = match &entry.target {
            LaunchTarget::Win32 { exe, .. } => self
                .icons
                .as_ref()
                .and_then(|icons| icons.get_or_queue(item.id(), &entry.icon_key(), exe)),
            // packaged logo 走 WinRT 资源,V1 用 SystemIcon 兜底。
            LaunchTarget::Packaged { .. } => Some(ResultIcon::SystemIcon(SystemIconId::App)),
        };
        p
    }

    /// 打开 / 以管理员身份运行 / 打开所在位置。后两个仅 Win32
    /// 目标——packaged 应用由系统代理激活,没有可提权/可定位的 exe。
    fn actions(&self, item: &ModuleItem) -> Vec<ActionDescriptor> {
        let mut actions = vec![ActionDescriptor {
            id: ActionId::PRIMARY,
            label: "打开".into(),
            shortcut: None,
        }];
        let is_win32 = item
            .downcast_ref::<AppEntry>()
            .is_some_and(|e| matches!(e.target, LaunchTarget::Win32 { .. }));
        if is_win32 {
            actions.push(ActionDescriptor {
                id: ACTION_RUN_AS_ADMIN,
                label: "以管理员身份运行".into(),
                shortcut: None,
            });
            actions.push(ActionDescriptor {
                id: ACTION_OPEN_LOCATION,
                label: "打开所在位置".into(),
                shortcut: None,
            });
        }
        actions
    }

    fn activate(&mut self, item: &ModuleItem, action: ActionId) -> ActivationFuture {
        let entry = item.downcast_ref::<AppEntry>().cloned();
        Box::pin(async move {
            let Some(entry) = entry else {
                return ModuleOutcome::failed(ModuleError::InvalidState(
                    "item payload is not an AppEntry".into(),
                ));
            };
            let result = match action {
                ActionId::PRIMARY => launch::launch(&entry.target),
                ACTION_RUN_AS_ADMIN => launch::launch_elevated(&entry.target),
                ACTION_OPEN_LOCATION => launch::reveal_location(&entry.target),
                _ => Err(ModuleError::ActivationFailed(format!(
                    "unknown action {action:?}"
                ))),
            };
            match result {
                Ok(()) => ModuleOutcome::success(
                    SessionDisposition::Close,
                    Some(UsageRecordRequest {
                        item_key: entry.item_key.to_string(),
                        action_id: action,
                    }),
                ),
                Err(e) => ModuleOutcome::failed(e),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn item_with(target: LaunchTarget) -> ModuleItem {
        ModuleItem::new(ItemId(1), AppEntry::new("TestApp", target))
    }

    /// Win32 应用有完整动作集(打开/以管理员身份运行/打开所在位置),
    /// packaged 应用只有打开——后两个动作没有可作用的 exe。
    #[test]
    fn actions_depend_on_target_kind() {
        let module = AppModule::new();
        let win32 = item_with(LaunchTarget::Win32 {
            exe: PathBuf::from(r"C:\Apps\test.exe"),
            args: "".into(),
            working_dir: None,
        });
        let actions = module.actions(&win32);
        assert_eq!(
            actions.iter().map(|a| a.id).collect::<Vec<_>>(),
            [ActionId::PRIMARY, ACTION_RUN_AS_ADMIN, ACTION_OPEN_LOCATION]
        );

        let packaged = item_with(LaunchTarget::Packaged {
            aumid: "Test!App".into(),
        });
        let actions = module.actions(&packaged);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, ActionId::PRIMARY);
    }

    /// packaged 目标走到次级动作只能来自 Core 与模块的版本错配——
    /// 明确报错,不静默降级。
    #[test]
    fn secondary_actions_reject_packaged() {
        let target = LaunchTarget::Packaged {
            aumid: "Test!App".into(),
        };
        assert!(launch::launch_elevated(&target).is_err());
        assert!(launch::reveal_location(&target).is_err());
    }
}
