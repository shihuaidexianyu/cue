//! sakana-module-web —— WebModule(§143)。
//!
//! `g` 触发的网页搜索:固定两对组合(必应+Edge / Google+Chrome),
//! 浏览器能力门控(§126 休眠同款,查询时现探),usage 把常用组合
//! 顶到第 0 行——没有「默认引擎」设置。`g` 单一职责:输入一律按
//! 搜索词处理,即使形似 URL(规则唯一、可预测);`g`+空 → 无结果
//! (无可枚举目录,不学 §126 空查询列全部)。
//!
//! 零索引、零网络 IO——搜索 URL 委派给浏览器打开;除 §128 合成
//! 的 module.web.trigger 外零设置。

use sakana_protocol::*;
use sakana_util_win::browser::Browser;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// 次级动作 ID(顺序即菜单顺序;PRIMARY = 搜索)。
const ACTION_COPY_URL: ActionId = ActionId(1);

/// 搜索行兜底字形:Segoe Search(§135;真浏览器图标未就绪时)。
const GLYPH_SEARCH: u32 = 0xE721;

/// 双浏览器都未装的提示行(ItemId 哨兵 0,与组合行 1/2 错开)。
const MISSING_ITEM_ID: ItemId = ItemId(0);

/// 一对固定的「引擎+浏览器」组合(表即全部语义,不做配置)。
struct ProviderSpec {
    /// usage 身份(固定 key,不按需增长,§50)。
    id: &'static str,
    /// 引擎中文名(进标题)。
    engine: &'static str,
    /// 搜索 URL 模板,尾部接 form-url 编码后的词。
    template: &'static str,
    /// 打开搜索结果页的目标浏览器。
    browser: Browser,
}

/// 两对组合(表序 = 无 usage 时的默认顺序,必应在前)。
const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "web:search:bing",
        engine: "必应",
        template: "https://www.bing.com/search?q=",
        browser: Browser::Edge,
    },
    ProviderSpec {
        id: "web:search:google",
        engine: "Google",
        template: "https://www.google.com/search?q=",
        browser: Browser::Chrome,
    },
];

/// form-url 编码(query 部分):unreserved 保留、空格 → '+'、
/// 其余字节 %XX 大写十六进制。手写 ~20 行,不引 url crate(§143);
/// 中文搜索词(§115 Unicode 粘贴)按 UTF-8 字节编码,四家引擎通吃。
fn form_url_encode(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for &b in query.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 一次搜索的完整启动 URL。
fn search_url(spec: &ProviderSpec, query: &str) -> String {
    format!("{}{}", spec.template, form_url_encode(query))
}

/// 行 payload:按查询合成的搜索项。spec 是静态表引用(零拷贝);
/// query/url 随查询分配,生命周期由 Arc 所有权表达(§11)。
struct WebSearchItem {
    spec: &'static ProviderSpec,
    query: String,
    url: String,
}

/// usage 加分(公式复制自 sakana-module-app,第三次使用;
/// 两行之间只比 usage,无需 §126 的封顶设计——重排即全部排序)。
fn usage_bonus(usage: Option<&UsageReader>, item_key: &str) -> i32 {
    let Some(stat) = usage.and_then(|u| u.stat(item_key, ActionId::PRIMARY)) else {
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

/// 纯查询逻辑(可测):`edge`/`chrome` 是浏览器 exe 探测结果。
/// 空查询/零预算无结果;组合全缺失给提示行;否则每对可用组合
/// 一行,usage 加分降序(stable sort 保底表序)。
fn search(
    usage: Option<&UsageReader>,
    edge: bool,
    chrome: bool,
    query: &str,
    limit: usize,
) -> Vec<ModuleItem> {
    let q = query.trim();
    if q.is_empty() || limit == 0 {
        return Vec::new();
    }
    let available = |s: &ProviderSpec| match s.browser {
        Browser::Edge => edge,
        Browser::Chrome => chrome,
    };
    let mut scored: Vec<(usize, i32)> = PROVIDERS
        .iter()
        .enumerate()
        .filter(|(_, s)| available(s))
        .map(|(i, s)| (i, usage_bonus(usage, s.id)))
        .collect();
    if scored.is_empty() {
        return vec![ModuleItem::new(MISSING_ITEM_ID, "missing")];
    }
    scored.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
    scored
        .into_iter()
        .take(limit)
        .map(|(i, _)| {
            let spec = &PROVIDERS[i];
            ModuleItem::new(
                ItemId(i as u64 + 1),
                WebSearchItem {
                    spec,
                    query: q.to_string(),
                    url: search_url(spec, q),
                },
            )
        })
        .collect()
}

/// WebModule,trigger `g`(§128 可自定义)。
pub struct WebModule {
    desc: ModuleDescriptor,
    usage: Option<UsageReader>,
    /// 浏览器图标(load 线程一次性提取,OnceLock 只到一次;
    /// IconImage 内部 Arc,clone 复用同一像素缓冲——UI 按指针缓存)。
    icons: Arc<OnceLock<HashMap<Browser, Arc<IconImage>>>>,
}

impl WebModule {
    pub fn new() -> Self {
        Self {
            desc: ModuleDescriptor {
                id: ModuleId::from_static("web"),
                name: "网页搜索",
                version: "0.1.0",
            },
            usage: None,
            icons: Arc::new(OnceLock::new()),
        }
    }
}

impl Default for WebModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for WebModule {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.desc
    }

    /// load 廉价:只提 usage 与事件 sink,图标提取挪 load 线程
    /// (≤2 枚 exe 图标,亚毫秒级;到位后 PresentationInvalidated
    /// 重绘可见行,§117 bookmark 同法)。
    fn load(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.usage = Some(ctx.usage.clone());
        let icons = Arc::clone(&self.icons);
        let sink = ctx.events.clone();
        let logger = ctx.logger.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let _com = sakana_util_win::com::ComGuard::new();
            let loaded = sakana_util_win::browser::load_icons();
            logger.log(
                LogLevel::Info,
                &format!(
                    "web ready: {} browser icons in {:?}",
                    loaded.len(),
                    started.elapsed()
                ),
            );
            if icons.set(loaded).is_ok() {
                let items = PROVIDERS
                    .iter()
                    .enumerate()
                    .map(|(i, _)| ItemId(i as u64 + 1))
                    .collect();
                sink.send(ModuleEvent::PresentationInvalidated { items });
            }
        });
        Ok(())
    }

    fn unload(&mut self) {
        self.usage = None;
        // icons OnceLock 不重建:幂等无害(同 bookmark 的处理)。
    }

    fn settings_schema(&self) -> SettingsSchema {
        Vec::new()
    }

    fn try_apply_settings(&mut self, _changes: SettingsChangeSet) -> Result<(), ModuleError> {
        Ok(())
    }
}

impl LauncherModule for WebModule {
    fn launcher_descriptor(&self) -> LauncherDescriptor {
        LauncherDescriptor {
            trigger: Some("g".into()),
            is_default: false,
        }
    }

    /// 创建 future 不触碰 IO;浏览器探测(is_file,微秒级)在
    /// future 内现探——缓存会错过中途安装(§126 休眠同款的
    /// 「能力门控」语义,只是探测时机从 load 挪到查询)。
    fn query(&mut self, ctx: QueryContext) -> QueryFuture {
        let usage = self.usage.clone();
        Box::pin(async move {
            let edge = Browser::Edge.exe_path().is_some();
            let chrome = Browser::Chrome.exe_path().is_some();
            let items = search(usage.as_ref(), edge, chrome, &ctx.query, ctx.result_limit);
            Ok(QueryResponse { items })
        })
    }

    fn present(&self, item: &ModuleItem) -> ResultPresentation {
        if let Some(search) = item.downcast_ref::<WebSearchItem>() {
            let mut p =
                ResultPresentation::new(format!("{}搜索「{}」", search.spec.engine, search.query));
            // 副标题 = 实际打开的 URL:透明可预期(§136 中间省略
            // 是 UI 的事)。
            p.subtitle = Some(Arc::from(search.url.as_str()));
            p.accessory = Some(ResultAccessory::Text(search.spec.browser.display().into()));
            p.icon = self
                .icons
                .get()
                .and_then(|m| m.get(&search.spec.browser))
                .map(|i| ResultIcon::Raster((**i).clone()))
                // 浏览器图标未就绪/缺失 → Search 字形兜底(§135)。
                .or_else(|| {
                    sakana_util_win::glyph::cached_glyph(
                        GLYPH_SEARCH,
                        sakana_util_win::glyph::DEFAULT_RGB,
                    )
                    .map(ResultIcon::Raster)
                });
            return p;
        }
        if item.downcast_ref::<&'static str>().is_some() {
            let mut p = ResultPresentation::new("未检测到 Edge 或 Chrome");
            p.subtitle = Some(Arc::from("搜索需要 Microsoft Edge 或 Google Chrome"));
            p.icon = sakana_util_win::glyph::cached_glyph(
                GLYPH_SEARCH,
                sakana_util_win::glyph::DEFAULT_RGB,
            )
            .map(ResultIcon::Raster);
            return p;
        }
        ResultPresentation::new("<unknown item>")
    }

    /// 搜索 / 复制链接。
    fn actions(&self, _item: &ModuleItem) -> Vec<ActionDescriptor> {
        vec![
            ActionDescriptor {
                id: ActionId::PRIMARY,
                label: "搜索".into(),
                shortcut: None,
            },
            ActionDescriptor {
                id: ACTION_COPY_URL,
                label: "复制链接".into(),
                shortcut: None,
            },
        ]
    }

    /// 搜索 = 配对浏览器打开搜索页(exe 缺失退回默认浏览器,
    /// §143「宁可降级」);复制链接 = 搜索 URL 进剪贴板。
    fn activate(&mut self, item: &ModuleItem, action: ActionId) -> ActivationFuture {
        let search = item
            .downcast_ref::<WebSearchItem>()
            .map(|s| (s.spec, s.url.clone()));
        let missing = item.downcast_ref::<&'static str>().copied();
        Box::pin(async move {
            if let Some((spec, url)) = search {
                let result = match action {
                    ActionId::PRIMARY => sakana_util_win::browser::open_url(spec.browser, &url),
                    ACTION_COPY_URL => sakana_util_win::clipboard::set_text(&url),
                    _ => Err(ModuleError::ActivationFailed(format!(
                        "unknown action {action:?}"
                    ))),
                };
                return match result {
                    Ok(()) => ModuleOutcome::success(
                        SessionDisposition::Close,
                        Some(UsageRecordRequest {
                            item_key: spec.id.to_string(),
                            action_id: action,
                        }),
                    ),
                    Err(e) => ModuleOutcome::failed(e),
                };
            }
            if missing.is_some() {
                return ModuleOutcome::failed(ModuleError::ActivationFailed(
                    "未检测到 Microsoft Edge 或 Google Chrome,无法搜索".into(),
                ));
            }
            ModuleOutcome::failed(ModuleError::InvalidState(
                "item payload is not a WebSearchItem".into(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::SystemTime;

    struct FakeUsage(HashMap<String, UsageStat>);
    impl UsageRead for FakeUsage {
        fn stat(&self, item_key: &str, _action: ActionId) -> Option<UsageStat> {
            self.0.get(item_key).copied()
        }
    }

    fn stat(count: u64) -> UsageStat {
        UsageStat {
            count,
            last_used: SystemTime::now(),
        }
    }

    /// 编码:unreserved 原样、空格 → +、非 ASCII 按 UTF-8 字节 %XX。
    #[test]
    fn form_url_encode_cases() {
        assert_eq!(form_url_encode("rust tutorial"), "rust+tutorial");
        assert_eq!(form_url_encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(form_url_encode("中文"), "%E4%B8%AD%E6%96%87");
        assert_eq!(form_url_encode("100%"), "100%25");
        assert_eq!(
            search_url(&PROVIDERS[0], "rust tutorial"),
            "https://www.bing.com/search?q=rust+tutorial"
        );
    }

    /// 空查询与零预算都无结果(无可枚举目录,不学 §126 空查询)。
    #[test]
    fn empty_query_and_zero_limit_yield_nothing() {
        let no_usage: Option<&UsageReader> = None;
        assert!(search(no_usage, true, true, "", 10).is_empty());
        assert!(search(no_usage, true, true, "  ", 10).is_empty());
        assert!(search(no_usage, true, true, "rust", 0).is_empty());
    }

    /// 两浏览器都在 → 两行,表序(必应在前);只有 Chrome → 只有
    /// Google 行;全缺 → 单行提示。
    #[test]
    fn capability_gating_shapes_rows() {
        let no_usage: Option<&UsageReader> = None;
        let both = search(no_usage, true, true, "rust", 10);
        assert_eq!(both.len(), 2);
        assert_eq!(both[0].id(), ItemId(1));
        assert_eq!(both[1].id(), ItemId(2));

        let only_chrome = search(no_usage, false, true, "rust", 10);
        assert_eq!(only_chrome.len(), 1);
        assert_eq!(only_chrome[0].id(), ItemId(2));

        let none = search(no_usage, false, false, "rust", 10);
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].id(), MISSING_ITEM_ID);
        assert!(none[0].downcast_ref::<&'static str>().is_some());
    }

    /// URL 形状的输入也按搜索词处理(`g` 单一职责,§143)。
    #[test]
    fn url_shaped_input_is_searched_not_opened() {
        let no_usage: Option<&UsageReader> = None;
        let r = search(no_usage, true, false, "https://example.com", 10);
        assert_eq!(r.len(), 1);
        let item = r[0].downcast_ref::<WebSearchItem>().unwrap();
        assert_eq!(
            item.url,
            "https://www.bing.com/search?q=https%3A%2F%2Fexample.com"
        );
    }

    /// usage 把常用组合顶到第 0 行;ItemId 随行身份,不随排序变。
    #[test]
    fn usage_floats_usual_pair_to_top() {
        let mut map = HashMap::new();
        map.insert("web:search:google".to_string(), stat(9));
        let usage: UsageReader = Arc::new(FakeUsage(map));
        let r = search(Some(&usage), true, true, "rust", 10);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].id(), ItemId(2)); // google 行在前…
        assert_eq!(r[1].id(), ItemId(1)); // …但 bing 的 ItemId 仍是 1
        let first = r[0].downcast_ref::<WebSearchItem>().unwrap();
        assert_eq!(first.spec.engine, "Google");
    }

    /// present/actions 形状:标题「引擎搜索「词」」、副标题是实际
    /// URL、accessory 是浏览器名;动作 = 搜索/复制链接。图标走
    /// 字形渲染,本测试不 load、不断言(真机肉眼验收,§135)。
    #[test]
    fn present_and_actions_shape() {
        let no_usage: Option<&UsageReader> = None;
        let items = search(no_usage, true, true, "rust", 10);
        let m = WebModule::new();
        let p = m.present(&items[0]);
        assert_eq!(&*p.title, "必应搜索「rust」");
        assert_eq!(
            p.subtitle.as_deref(),
            Some(search_url(&PROVIDERS[0], "rust").as_str())
        );
        assert!(matches!(
            &p.accessory,
            Some(ResultAccessory::Text(t)) if &**t == "Edge"
        ));
        let acts = m.actions(&items[0]);
        assert_eq!(
            acts.iter().map(|a| &*a.label).collect::<Vec<_>>(),
            ["搜索", "复制链接"]
        );

        let missing = search(no_usage, false, false, "rust", 10);
        let p = m.present(&missing[0]);
        assert_eq!(&*p.title, "未检测到 Edge 或 Chrome");
    }

    /// 触发词 `g`,非默认模块;查询词前后空白被 trim。
    #[test]
    fn descriptor_and_query_trim() {
        let m = WebModule::new();
        let d = m.launcher_descriptor();
        assert_eq!(d.trigger.as_deref(), Some("g"));
        assert!(!d.is_default);
        let no_usage: Option<&UsageReader> = None;
        let r = search(no_usage, true, false, "  rust  ", 10);
        let item = r[0].downcast_ref::<WebSearchItem>().unwrap();
        assert_eq!(item.query, "rust");
    }
}
