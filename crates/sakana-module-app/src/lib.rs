//! sakana-module-app —— AppModule。
//!
//! V1 唯一必装模块:User/Common Start Menu + UWP/MSIX 发现,
//! 拼音(全拼 + 首字母)+ fuzzy 搜索,usage ranking,异步图标。
//! Core 不知道什么是 .lnk、拼音、AUMID——全部语义在本 crate。
//! 另有 §143 的默认路径「打开链接」伪结果:输入可判定为 URL
//! 时钉顶一行(粘贴直达,零按键流程)。

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
use sakana_util_common::usage_bonus;
use std::sync::Arc;

/// 次级动作 ID(顺序即菜单顺序;PRIMARY = 打开)。
const ACTION_RUN_AS_ADMIN: ActionId = ActionId(1);
const ACTION_OPEN_LOCATION: ActionId = ActionId(2);
/// 「打开链接」行的次级动作(§143;3/4/5 接在应用动作的 1/2 后)。
const ACTION_OPEN_EDGE: ActionId = ActionId(3);
const ACTION_OPEN_CHROME: ActionId = ActionId(4);
const ACTION_COPY_URL: ActionId = ActionId(5);

/// 便携应用目录设置(§133):分号分隔,改动重启后生效
/// (catalog 只在进程启动时构建,§56)。
const KEY_EXTRA_DIRS: &str = "module.app.extra_dirs";

/// packaged 应用 logo 未就绪时的兜底字形:Segoe AppIconDefault(§135)。
const GLYPH_APP: u32 = 0xECAA;
/// 「打开链接」行的字形:Segoe Link(§135)。
const GLYPH_LINK: u32 = 0xE71B;

/// 「打开链接」伪结果的 ItemId 哨兵(§143):catalog 条目序号从 1
/// 起,u64::MAX 永不相撞;行只随查询存在,无需稳定跨查询。
const OPEN_URL_ITEM_ID: ItemId = ItemId(u64::MAX);

/// 「打开链接」伪结果的 payload(§143):默认路径粘贴/输入 URL 时
/// 钉顶一行,AppModule 自持——模态隔离不破(§83),§82 不改。
struct OpenUrl {
    url: String,
}

/// 粘贴收尾噪声:引号/中文句号逗号/成对括号(markdown `(url)`、
/// 中文「url」;trim 已去空白)。URL 不会以这些字符开头,
/// trim_matches 双端安全。
const PASTE_NOISE: [char; 16] = [
    '"', '\'', '。', '，', '、', ';', '；', ',', '(', ')', '[', ']', '{', '}', '「', '」',
];

/// URL 判定(§143,纯函数可测):通过则返回可直接交给 ShellExecute
/// 的 URL。scheme 白名单 http/https(大小写不敏感);无 scheme 时
/// 裸域名(含 '.')、IPv4 字面量、localhost(:port)补 https://;
/// 含空格或其余 scheme 一律拒绝——绝不让任意 scheme 到达
/// ShellExecute。字符检查从宽(拒引号/尖括号/反斜杠),打不开的
/// 主机交给浏览器自行降级为搜索。
fn url_normalize(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches(|c| PASTE_NOISE.contains(&c));
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    if let Some((scheme, rest)) = trimmed.split_once("://") {
        if !rest.is_empty() && matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
            return Some(trimmed.to_string());
        }
        return None;
    }
    if trimmed.starts_with('.') || !trimmed.chars().all(url_char) {
        return None;
    }
    let host = trimmed.split(['/', ':', '?', '#']).next().unwrap_or("");
    if host == "localhost" || is_ipv4(host) || host.contains('.') {
        return Some(format!("https://{trimmed}"));
    }
    None
}

/// 无 scheme 分支的字符白名单:unicode 字母数字(IDN 中文域名可
/// 过)+ URL 标点;反引号、尖括号、反斜杠、花括号等被拒。
fn url_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            '-' | '.'
                | '_'
                | ':'
                | '/'
                | '?'
                | '#'
                | '['
                | ']'
                | '@'
                | '!'
                | '$'
                | '&'
                | '('
                | ')'
                | '*'
                | '+'
                | ','
                | ';'
                | '='
                | '%'
                | '~'
        )
}

/// 宽松 IPv4:四段、每段 1–3 位数字 ≤ 255。
fn is_ipv4(host: &str) -> bool {
    let mut parts = host.split('.');
    let mut n = 0;
    for part in parts.by_ref() {
        if part.is_empty()
            || part.len() > 3
            || !part.bytes().all(|b| b.is_ascii_digit())
            || part.parse::<u16>().unwrap_or(1000) > 255
        {
            return false;
        }
        n += 1;
    }
    n == 4
}

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
/// V1 无 aliases UI,恒 0)。UsageBonus 公式已随第三次复制下沉
/// sakana-util-common(§145),本模块只保留调用。

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
            matcher::best_score(&q, &keys).map(|s| (s + usage_bonus(usage, &e.item_key), e))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.name_lower.cmp(&b.1.name_lower))
    });
    // §143:输入可判定为 URL 时钉顶「打开链接」伪结果(粘贴直达);
    // 为它预留名额,总数不破 result_limit 预算(§94)。空查询在上
    // 面的早退分支里,不会走到判定。
    let open_url = url_normalize(query);
    scored.truncate(limit.saturating_sub(usize::from(open_url.is_some())));
    let mut items: Vec<ModuleItem> = scored
        .into_iter()
        .map(|(_, e)| ModuleItem::new(ItemId(e.item_id()), e.clone()))
        .collect();
    if let Some(url) = open_url {
        items.insert(0, ModuleItem::new(OPEN_URL_ITEM_ID, OpenUrl { url }));
    }
    items
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

/// 「打开链接」行的动作表(§143,纯函数可测):打开 = 系统默认
/// 浏览器(永远可用);用 Edge/Chrome 打开 = 配对浏览器,探测到
/// 才出现;复制链接收尾。usage key 固定 open-url。
fn open_url_actions(edge: bool, chrome: bool) -> Vec<ActionDescriptor> {
    let mut actions = vec![ActionDescriptor {
        id: ActionId::PRIMARY,
        label: "打开".into(),
        shortcut: None,
    }];
    if edge {
        actions.push(ActionDescriptor {
            id: ACTION_OPEN_EDGE,
            label: "用 Edge 打开".into(),
            shortcut: None,
        });
    }
    if chrome {
        actions.push(ActionDescriptor {
            id: ACTION_OPEN_CHROME,
            label: "用 Chrome 打开".into(),
            shortcut: None,
        });
    }
    actions.push(ActionDescriptor {
        id: ACTION_COPY_URL,
        label: "复制链接".into(),
        shortcut: None,
    });
    actions
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
        // §143「打开链接」伪结果:URL 即标题,Link 字形图标。
        if let Some(open) = item.downcast_ref::<OpenUrl>() {
            let mut p = ResultPresentation::new(open.url.clone());
            p.subtitle = Some("在默认浏览器中打开".into());
            p.icon = sakana_util_win::glyph::cached_glyph(
                GLYPH_LINK,
                sakana_util_win::glyph::DEFAULT_RGB,
            )
            .map(ResultIcon::Raster);
            return p;
        }
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

    /// 应用行:打开 / 以管理员身份运行 / 打开所在位置(后两个仅
    /// Win32 目标——packaged 应用由系统代理激活,没有可提权/可
    /// 定位的 exe)。「打开链接」行:打开(默认浏览器)/ 用 Edge/
    /// Chrome 打开(探测到才出现,§143)/ 复制链接。
    fn actions(&self, item: &ModuleItem) -> Vec<ActionDescriptor> {
        if item.downcast_ref::<OpenUrl>().is_some() {
            return open_url_actions(
                sakana_util_win::browser::Browser::Edge.exe_path().is_some(),
                sakana_util_win::browser::Browser::Chrome
                    .exe_path()
                    .is_some(),
            );
        }
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
        // §143「打开链接」伪结果:默认浏览器/配对浏览器/复制链接;
        // usage 固定 key open-url(§50 存储不按需增长)。
        let open_url = item.downcast_ref::<OpenUrl>().map(|o| o.url.clone());
        let entry = item.downcast_ref::<AppEntry>().cloned();
        Box::pin(async move {
            if let Some(url) = open_url {
                let result = match action {
                    ActionId::PRIMARY => sakana_util_win::shell::shell_execute(&url, None, None),
                    ACTION_OPEN_EDGE => sakana_util_win::browser::open_url(
                        sakana_util_win::browser::Browser::Edge,
                        &url,
                    ),
                    ACTION_OPEN_CHROME => sakana_util_win::browser::open_url(
                        sakana_util_win::browser::Browser::Chrome,
                        &url,
                    ),
                    ACTION_COPY_URL => sakana_util_win::clipboard::set_text(&url),
                    _ => Err(ModuleError::ActivationFailed(format!(
                        "unknown action {action:?}"
                    ))),
                };
                return match result {
                    Ok(()) => ModuleOutcome::success(
                        SessionDisposition::Close,
                        Some(UsageRecordRequest {
                            item_key: "open-url".to_string(),
                            action_id: action,
                        }),
                    ),
                    Err(e) => ModuleOutcome::failed(e),
                };
            }
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

    /// url_normalize 全案(§143):scheme 白名单、裸域名补 https、
    /// IP/localhost、尾部粘贴噪声、含空格与其余 scheme 拒绝。
    #[test]
    fn url_normalize_cases() {
        // 显式 scheme(http/https 任意大小写)原样通过
        assert_eq!(
            url_normalize("https://example.com/path?q=1"),
            Some("https://example.com/path?q=1".to_string())
        );
        assert_eq!(
            url_normalize("HTTP://Example.COM"),
            Some("HTTP://Example.COM".to_string())
        );
        assert_eq!(
            url_normalize("http://localhost:3000"),
            Some("http://localhost:3000".to_string())
        );
        // 无 scheme:裸域名 / IPv4 / localhost 补 https://
        assert_eq!(
            url_normalize("github.com/xx"),
            Some("https://github.com/xx".to_string())
        );
        assert_eq!(
            url_normalize("127.0.0.1:8080/a"),
            Some("https://127.0.0.1:8080/a".to_string())
        );
        assert_eq!(
            url_normalize("localhost"),
            Some("https://localhost".to_string())
        );
        // 尾部粘贴噪声修剪(中文句号 / 引号 / markdown 括号)
        assert_eq!(
            url_normalize("example.com。"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            url_normalize("\"https://example.com\""),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            url_normalize("(https://example.com)"),
            Some("https://example.com".to_string())
        );
        // 拒绝:含空格、非白名单 scheme、空串
        assert_eq!(url_normalize("rust tutorial"), None);
        assert_eq!(url_normalize("javascript:alert(1)"), None);
        assert_eq!(url_normalize("file:///etc/passwd"), None);
        assert_eq!(url_normalize("ftp://example.com"), None);
        assert_eq!(url_normalize(""), None);
        assert_eq!(url_normalize("。。。"), None);
        // 灰色地带(§143 记录在案):带点输入放行,浏览器自行降级
        assert_eq!(
            url_normalize("config.yaml"),
            Some("https://config.yaml".to_string())
        );
    }

    /// 输入可判定为 URL 时钉顶「打开链接」行(§143):哨兵 ItemId、
    /// 应用结果退后、result_limit 预算不破;非 URL 输入不出现。
    #[test]
    fn open_url_row_pins_top_within_limit() {
        let no_usage: Option<&UsageReader> = None;
        // 空 catalog:URL 行独占
        let r = search(&[], no_usage, "https://example.com", 10);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id(), OPEN_URL_ITEM_ID);
        let open = r[0].downcast_ref::<OpenUrl>().unwrap();
        assert_eq!(open.url, "https://example.com");

        // limit = 1:预算全给钉顶行,应用结果一个不超
        let r = search(&[], no_usage, "https://example.com", 1);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id(), OPEN_URL_ITEM_ID);

        // 非 URL 输入:无伪结果
        let r = search(&[], no_usage, "chrome", 10);
        assert!(r.is_empty());
        // 空查询:Top Apps 分支,无伪结果
        let r = search(&[], no_usage, "", 10);
        assert!(r.is_empty());
    }

    /// 「打开链接」行的 present 与动作表(§143):标题 = URL、
    /// 副标题说明;动作 = 打开 + [用 Edge 打开] + [用 Chrome 打开]
    /// + 复制链接,探测开关控制中间两项。
    #[test]
    fn open_url_present_and_actions() {
        let module = AppModule::new();
        let item = ModuleItem::new(
            OPEN_URL_ITEM_ID,
            OpenUrl {
                url: "https://example.com".into(),
            },
        );
        let p = module.present(&item);
        assert_eq!(&*p.title, "https://example.com");
        assert_eq!(p.subtitle.as_deref(), Some("在默认浏览器中打开"));

        let both = open_url_actions(true, true);
        assert_eq!(
            both.iter().map(|a| a.id).collect::<Vec<_>>(),
            [
                ActionId::PRIMARY,
                ACTION_OPEN_EDGE,
                ACTION_OPEN_CHROME,
                ACTION_COPY_URL
            ]
        );
        assert_eq!(
            both.iter().map(|a| &*a.label).collect::<Vec<_>>(),
            ["打开", "用 Edge 打开", "用 Chrome 打开", "复制链接"]
        );
        // 只装 Edge:Chrome 动作缺席,复制链接收尾
        let edge_only = open_url_actions(true, false);
        assert_eq!(
            edge_only.iter().map(|a| a.id).collect::<Vec<_>>(),
            [ActionId::PRIMARY, ACTION_OPEN_EDGE, ACTION_COPY_URL]
        );
        // 都没有:打开 + 复制链接
        let none = open_url_actions(false, false);
        assert_eq!(
            none.iter().map(|a| a.id).collect::<Vec<_>>(),
            [ActionId::PRIMARY, ACTION_COPY_URL]
        );
    }
}
