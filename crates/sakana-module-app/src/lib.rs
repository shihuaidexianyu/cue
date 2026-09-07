//! sakana-module-app —— AppModule。
//!
//! V1 唯一必装模块:User/Common Start Menu + UWP/MSIX 发现,
//! 拼音(全拼 + 首字母)+ fuzzy 搜索,usage ranking,异步图标。
//! Core 不知道什么是 .lnk、拼音、AUMID——全部语义在本 crate。

mod app_paths;
mod catalog;
mod extra_dirs;
mod icon;
mod icon_cache;
mod launch;
mod matcher;
mod packaged;
mod pinyin_index;
mod ready;
mod start_menu;

pub use catalog::{AppEntry, LaunchTarget};

use icon::IconPipeline;
use ready::CatalogCell;
use sakana_protocol::*;
use std::sync::Arc;

/// 次级动作 ID(顺序即菜单顺序;PRIMARY = 打开)。
const ACTION_RUN_AS_ADMIN: ActionId = ActionId(1);
const ACTION_OPEN_LOCATION: ActionId = ActionId(2);

/// 便携应用目录设置(§133):分号分隔,改动重启后生效
/// (catalog 只在进程启动时构建,§56)。
const KEY_EXTRA_DIRS: &str = "module.app.extra_dirs";

/// packaged 应用 logo 未就绪时的兜底字形:Segoe AppIconDefault(§135)。
const GLYPH_APP: u32 = 0xECAA;

/// AppModule 是 V1 的 required default module。
pub struct AppModule {
    descriptor: ModuleDescriptor,
    /// spike:发现不满足冷启动预算,catalog 由后台线程一次性
    /// 构建(进程启动时唯一一次,无 watcher);query future 等就绪。
    catalog: Arc<CatalogCell>,
    usage: Option<UsageReader>,
    icons: Option<Arc<IconPipeline>>,
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
        let icons = Arc::new(IconPipeline::new(
            ctx.events.clone(),
            // §137:图标磁盘缓存目录(Core 已建好 cache 根)。
            Some(ctx.storage.cache.join("icons")),
            ctx.logger.clone(),
        ));
        self.icons = Some(Arc::clone(&icons));

        let extra_dirs = match ctx.settings.get("extra_dirs") {
            Some(SettingValue::String(s)) => extra_dirs::parse_dirs(s),
            _ => Vec::new(),
        };

        let cell = Arc::clone(&self.catalog);
        let logger = ctx.logger.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let mut entries = start_menu::discover(&logger);
            let n_start_menu = entries.len();
            let packaged = packaged::discover(&logger);
            let n_packaged = packaged.entries.len();
            // §134:logo 索引先于 catalog 发布填入图标管线——query 看到
            // packaged 条目时索引必然就绪,worker 取 logo 零等待。
            // §137:版本号表同行,作磁盘缓存的失效指纹。
            icons.set_packaged_index(packaged.logo_index, packaged.versions);
            entries.extend(packaged.entries);
            // §133:App Paths 注册表 + 用户声明便携目录。排在开始菜单
            // 之后:同一 exe 的首见者胜(dedup 保首个),lnk 的显示名
            // 通常比 exe 文件名漂亮。
            entries.extend(app_paths::discover(&logger));
            let n_app_paths = entries.len() - n_start_menu - n_packaged;
            entries.extend(extra_dirs::discover(&extra_dirs, &logger));
            let n_extra = entries.len() - n_start_menu - n_packaged - n_app_paths;
            catalog::dedup(&mut entries);
            entries.sort_by(|a, b| a.name_lower.cmp(&b.name_lower));
            // 冷启动 spike:构建耗时就地记录。
            logger.log(
                LogLevel::Info,
                &format!(
                    "app catalog ready: {} entries ({n_start_menu} start menu, {n_packaged} packaged, {n_app_paths} app paths, {n_extra} extra dirs) in {:?}",
                    entries.len(),
                    started.elapsed()
                ),
            );
            // §137:发布前先预载磁盘图标缓存(亚秒级)——首个查询
            // 即可见全部缓存图标,不经历占位蹦出;相对秒级的 catalog
            // 构建,这点延迟无感。
            icons.preload_from_cache(&entries);
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
        vec![SettingSpec {
            key: SettingKey(KEY_EXTRA_DIRS.into()),
            label: "应用:便携软件目录".into(),
            description: Some(
                "分号分隔的目录列表,递归扫描其中的 exe/lnk(≤4 层,跳过隐藏项);改动重启 sakana 后生效"
                    .into(),
            ),
            kind: SettingKind::String,
            default: SettingValue::String(String::new()),
            apply_policy: ApplyPolicy::RestartApplication,
        }]
    }

    /// extra_dirs 是 RestartApplication 策略:Core 直接提交并标记
    /// 待重启,不经模块 try-apply——这里无事可做。
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
            LaunchTarget::Win32 { exe, .. } => self.icons.as_ref().and_then(|icons| {
                icons.get_or_queue(item.id(), &entry.icon_key(), icon::IconSource::Exe(exe))
            }),
            // §134:packaged logo 走 GetLogo 异步提取;未就绪/失败
            // 用通用应用字形兜底(§135,Segoe AppIconDefault)。
            LaunchTarget::Packaged { aumid } => self
                .icons
                .as_ref()
                .and_then(|icons| {
                    icons.get_or_queue(
                        item.id(),
                        &entry.icon_key(),
                        icon::IconSource::Packaged(aumid),
                    )
                })
                .or_else(|| {
                    sakana_util_win::glyph::cached_glyph(
                        GLYPH_APP,
                        sakana_util_win::glyph::DEFAULT_RGB,
                    )
                    .map(ResultIcon::Raster)
                }),
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
