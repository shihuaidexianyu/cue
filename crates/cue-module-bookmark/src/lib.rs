//! cue-module-bookmark —— BookmarkModule。
//!
//! Chromium 系(Edge/Chrome)书签搜索:触发词 `b`(词边界规则:
//! 字母触发的模块只吃 `b<空格>` 或裸 `b`,不吞 `baidu`)。数据源是
//! `<User Data>/<profile>/{Bookmarks,AccountBookmarks}` JSON(无锁,浏览器运行中可读);
//! 刷新走 mtime 指纹,无 watcher。打开从哪来回哪开——
//! 来源浏览器 exe 带 URL 启动(exe 缺失退回系统默认浏览器)。
//! Firefox(places.sqlite)不在范围——不为它引入 rusqlite。

mod catalog;
mod chromium;
mod icon;
mod matcher;
mod pinyin_index;

use chromium::Browser;
use cue_protocol::*;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// 次级动作 ID(顺序即菜单顺序;PRIMARY = 打开)。
const ACTION_COPY_URL: ActionId = ActionId(1);

/// BookmarkModule,trigger `b`。
pub struct BookmarkModule {
    descriptor: ModuleDescriptor,
    catalog: Arc<catalog::CatalogCache>,
    /// 浏览器图标(每浏览器一张,load 后台线程一次性提取;同一 Arc
    /// 复用,UI 按 rgba 指针缓存纹理)。
    icons: Arc<OnceLock<HashMap<Browser, Arc<IconImage>>>>,
    usage: Option<UsageReader>,
}

impl BookmarkModule {
    pub fn new() -> Self {
        Self {
            descriptor: ModuleDescriptor {
                id: ModuleId::from_static("bookmark"),
                name: "书签",
                version: "0.1.0",
            },
            catalog: catalog::CatalogCache::new(),
            icons: Arc::new(OnceLock::new()),
            usage: None,
        }
    }
}

impl Default for BookmarkModule {
    fn default() -> Self {
        Self::new()
    }
}

/// 公式复制自 cue-module-app(Rule of Three 第二次使用):
/// UsageBonus = min(count,20)*2;24h 内 +10,7d 内 +5。
fn usage_bonus(usage: Option<&UsageReader>, entry: &catalog::BookmarkEntry) -> i32 {
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
    entries: &[Arc<catalog::BookmarkEntry>],
    usage: Option<&UsageReader>,
    query: &str,
    limit: usize,
) -> Vec<ModuleItem> {
    if query.is_empty() {
        // 空查询 = usage Top Bookmarks;无 usage 不显示"推荐"。
        return top_used(entries, usage, limit);
    }
    let q = query.to_lowercase();
    let mut scored: Vec<(i32, &Arc<catalog::BookmarkEntry>)> = entries
        .iter()
        .filter_map(|e| {
            // title 传原始大小写(驼峰词首加分);domain 是第四个键——
            // "b github.com" 直接按域名命中。
            let keys: [&str; 4] = [&e.title, &e.pinyin_full, &e.pinyin_initials, &e.domain];
            matcher::best_score(&q, &keys).map(|s| (s + usage_bonus(usage, e), e))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.title_lower.cmp(&b.1.title_lower))
    });
    scored.truncate(limit);
    scored
        .into_iter()
        // payload 直接用 catalog 的 Arc:零拷贝,条目生命周期由
        // Arc 所有权表达。
        .map(|(_, e)| ModuleItem::new(ItemId(e.item_id()), e.clone()))
        .collect()
}

/// 空查询:按 (count, last_used) 排的 Top Bookmarks。
fn top_used(
    entries: &[Arc<catalog::BookmarkEntry>],
    usage: Option<&UsageReader>,
    limit: usize,
) -> Vec<ModuleItem> {
    let Some(usage) = usage else {
        return Vec::new();
    };
    let mut with_stat: Vec<(&Arc<catalog::BookmarkEntry>, UsageStat)> = entries
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

impl Module for BookmarkModule {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    /// load 廉价:只启动一个后台线程——首次 catalog 解析(冷读可能
    /// 触发 AV 扫描,实测首个查询付出 3.2 s)与图标提取都在线程内完成,
    /// 之后的 query 只付指纹比对。
    fn load(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.usage = Some(ctx.usage.clone());

        let icons = Arc::clone(&self.icons);
        let catalog = Arc::clone(&self.catalog);
        let sink = ctx.events.clone();
        let logger = ctx.logger.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            catalog.refresh_if_changed(); // 冷启动解析挪离首个查询
            let n = catalog.entries().len();
            let t_catalog = started.elapsed();
            let _com = cue_util_win::com::ComGuard::new();
            let loaded = icon::load_browser_icons();
            logger.log(
                LogLevel::Info,
                &format!(
                    "bookmark ready: {n} entries in {t_catalog:?}, {} browser icons in {:?}",
                    loaded.len(),
                    started.elapsed()
                ),
            );
            if icons.set(loaded).is_ok() {
                // 图标只到一次;让 Core 重跑可见行的 present()。
                let items = catalog
                    .entries()
                    .iter()
                    .map(|e| ItemId(e.item_id()))
                    .collect();
                sink.send(ModuleEvent::PresentationInvalidated { items });
            }
        });
        Ok(())
    }

    fn unload(&mut self) {
        self.usage = None;
        // icons OnceLock / catalog cache 不重建:幂等无害(同 AppModule
        // catalog cell 的处理)。
    }

    fn settings_schema(&self) -> SettingsSchema {
        Vec::new()
    }

    fn try_apply_settings(&mut self, _changes: SettingsChangeSet) -> Result<(), ModuleError> {
        Ok(())
    }
}

impl LauncherModule for BookmarkModule {
    fn launcher_descriptor(&self) -> LauncherDescriptor {
        LauncherDescriptor {
            trigger: Some("b".to_string()),
            is_default: false,
        }
    }

    /// 创建不触碰 IO;刷新(mtime 指纹 + 必要时重解析)在
    /// future 内、后台执行器上跑。
    fn query(&mut self, ctx: QueryContext) -> QueryFuture {
        let catalog = Arc::clone(&self.catalog);
        let usage = self.usage.clone();
        Box::pin(async move {
            catalog.refresh_if_changed();
            let entries = catalog.entries();
            let items = search(&entries, usage.as_ref(), &ctx.query, ctx.result_limit);
            Ok(QueryResponse { items })
        })
    }

    fn present(&self, item: &ModuleItem) -> ResultPresentation {
        let Some(entry) = item.downcast_ref::<Arc<catalog::BookmarkEntry>>() else {
            return ResultPresentation::new("<unknown item>");
        };
        let mut p = ResultPresentation::new(entry.title.clone());
        // 非 Default profile 在副标题标注(Flow 同款:"Default" 不标注)。
        p.subtitle = Some(
            if entry.profile.is_empty() {
                entry.domain.to_string()
            } else {
                format!("{} · {}", entry.domain, entry.profile)
            }
            .into(),
        );
        p.accessory = Some(ResultAccessory::Text(entry.browser.display().into()));
        p.icon = Some(
            self.icons
                .get()
                .and_then(|m| m.get(&entry.browser))
                // IconImage.rgba 是 Arc<[u8]>,clone 保持指针不变——
                // UI 按该指针缓存纹理。
                .map(|i| ResultIcon::Raster((**i).clone()))
                .unwrap_or(ResultIcon::SystemIcon(SystemIconId::Generic)),
        );
        p
    }

    /// 打开 / 复制链接。
    fn actions(&self, _item: &ModuleItem) -> Vec<ActionDescriptor> {
        vec![
            ActionDescriptor {
                id: ActionId::PRIMARY,
                label: "打开".into(),
                shortcut: None,
            },
            ActionDescriptor {
                id: ACTION_COPY_URL,
                label: "复制链接".into(),
                shortcut: None,
            },
        ]
    }

    /// 打开 = 从哪来回哪开:来源浏览器 exe 带 URL 启动;
    /// exe 缺失(浏览器已卸载等)退回系统默认浏览器。
    fn activate(&mut self, item: &ModuleItem, action: ActionId) -> ActivationFuture {
        let entry = item.downcast_ref::<Arc<catalog::BookmarkEntry>>().cloned();
        Box::pin(async move {
            let Some(entry) = entry else {
                return ModuleOutcome::failed(ModuleError::InvalidState(
                    "item payload is not a BookmarkEntry".into(),
                ));
            };
            let result = match action {
                ActionId::PRIMARY => open_in_browser(entry.browser, &entry.url),
                ACTION_COPY_URL => cue_util_win::clipboard::set_text(&entry.url),
                _ => Err(ModuleError::ActivationFailed(format!(
                    "unknown action {action:?}"
                ))),
            };
            match result {
                Ok(()) => ModuleOutcome::success(
                    SessionDisposition::Close,
                    // usage 身份 = {browser}:{url}。
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

/// 从哪来回哪开:来源浏览器 exe + URL 参数;exe 找不到退回
/// 默认浏览器打开 URL——宁可降级,不让激活失败。
fn open_in_browser(browser: Browser, url: &str) -> Result<(), ModuleError> {
    match browser.exe_path() {
        Some(exe) => cue_util_win::shell::shell_execute(&exe.to_string_lossy(), Some(url), None),
        None => cue_util_win::shell::shell_execute(url, None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn entry(title: &str, url: &str, browser: Browser) -> Arc<catalog::BookmarkEntry> {
        catalog::test_entry(title, url, browser)
    }

    struct FakeUsage(std::collections::HashMap<String, UsageStat>);
    impl UsageRead for FakeUsage {
        fn stat(&self, item_key: &str, _action: ActionId) -> Option<UsageStat> {
            self.0.get(item_key).copied()
        }
    }

    /// 打开 / 复制链接,顺序即菜单顺序。
    #[test]
    fn actions_are_open_and_copy_url() {
        let m = BookmarkModule::new();
        let item = ModuleItem::new(
            ItemId(1),
            entry("GitHub", "https://github.com/", Browser::Edge),
        );
        let actions = m.actions(&item);
        assert_eq!(
            actions.iter().map(|a| a.id).collect::<Vec<_>>(),
            [ActionId::PRIMARY, ACTION_COPY_URL]
        );
        assert_eq!(
            actions.iter().map(|a| &*a.label).collect::<Vec<_>>(),
            ["打开", "复制链接"]
        );
    }

    #[test]
    fn search_matches_title_pinyin_and_domain() {
        let entries = vec![
            entry("GitHub", "https://github.com/", Browser::Edge),
            entry("永劫无间官网", "https://www.yjwujian.cn/", Browser::Edge),
            entry("Rust 文档", "https://doc.rust-lang.org/", Browser::Chrome),
        ];
        // 标题(英文,走 title_lower 等值键)
        let r = search(&entries, None, "gith", 10);
        assert_eq!(r.len(), 1);
        // 拼音首字母
        let r = search(&entries, None, "yjwj", 10);
        assert_eq!(r.len(), 1);
        // 域名键
        let r = search(&entries, None, "rust-lang", 10);
        assert_eq!(r.len(), 1);
        // 非子序列不中
        assert!(search(&entries, None, "zzzz", 10).is_empty());
    }

    #[test]
    fn empty_query_shows_usage_top_only() {
        let entries = vec![
            entry("甲", "https://a.example/", Browser::Edge),
            entry("乙", "https://b.example/", Browser::Edge),
        ];
        // 无 usage → 空(不显示推荐内容)
        assert!(search(&entries, None, "", 10).is_empty());
        let mut map = std::collections::HashMap::new();
        map.insert(
            "edge:https://b.example/".to_string(),
            UsageStat {
                count: 3,
                last_used: SystemTime::now() - Duration::from_secs(60),
            },
        );
        let usage: UsageReader = Arc::new(FakeUsage(map));
        let r = search(&entries, Some(&usage), "", 10);
        assert_eq!(r.len(), 1);
        let e = r[0].downcast_ref::<Arc<catalog::BookmarkEntry>>().unwrap();
        assert_eq!(&*e.url, "https://b.example/");
    }

    /// 从哪来回哪开:usage 身份带来源浏览器前缀——同 URL 在 Edge 与
    /// Chrome 各收一份时,两行是不同启动动作,计数互不影响。
    #[test]
    fn item_key_scopes_usage_to_source_browser() {
        let edge = entry("同一站", "https://x.example/", Browser::Edge);
        let chrome = entry("同一站", "https://x.example/", Browser::Chrome);
        assert_eq!(&*edge.item_key, "edge:https://x.example/");
        assert_eq!(&*chrome.item_key, "chrome:https://x.example/");
        assert_ne!(edge.item_id(), chrome.item_id());
    }

    #[test]
    fn usage_bonus_breaks_ties() {
        let entries = vec![
            entry("rust one", "https://one.example/", Browser::Edge),
            entry("rust two", "https://two.example/", Browser::Edge),
        ];
        let mut map = std::collections::HashMap::new();
        map.insert(
            "edge:https://two.example/".to_string(),
            UsageStat {
                count: 5,
                last_used: SystemTime::now(),
            },
        );
        let usage: UsageReader = Arc::new(FakeUsage(map));
        let r = search(&entries, Some(&usage), "rust", 10);
        assert_eq!(r.len(), 2);
        let first = r[0].downcast_ref::<Arc<catalog::BookmarkEntry>>().unwrap();
        assert_eq!(&*first.url, "https://two.example/");
    }
}
