//! cue-module-file —— FileModule。
//!
//! `/` 触发的文件搜索:触发词是标点,verbatim 匹配(词边界只约束
//! 字母触发);`/` 之后的输入原样作为 Everything 搜索串——查询语法
//! (子串、`ext:`、路径过滤等)归模块与 Everything,Core 不解析。
//! 数据源是本机已安装运行的 Everything 1.4(依赖第三方服务,
//! 不自建索引、不链 Everything.dll),IPC 走 WM_COPYDATA(专用
//! 线程 + latest-wins 槽)。文件与文件夹同一模态;FileEntry
//! 只在模块内部,Core 只见 ItemId。
//!
//! 空查询返回空:UsageRead 只能按键查、不能枚举,给不出 Top Files,
//! 不显示任何推荐内容。排序保持 Everything 的
//! NAME_ASCENDING(SDK 保证该序无性能损失),V1 不做 usage 重排。
//!
//! 噪声目录默认排除:工作文件几乎从不在系统目录、AppData、包缓存、
//! 编辑器扩展目录里,但它们会以"工具内脏"的形式淹没结果。排除不做
//! 结果后过滤,而是把否定子句拼进发给 Everything 的查询串(`!"路径
//! 片段"`),让这些路径根本不占结果位。名单本身即设置
//! (`module.file.excluded_paths`,分号分隔的路径片段,参照 VS Code
//! search.exclude 与 Windows Search 默认索引范围的口径给默认值),
//! 总开关是 `module.file.exclude_noise_paths`。查询含 `\`(用户在写
//! 显式路径)时原样发送、不加排除——刻意找系统文件时不会被拦。

mod everything;
mod icon;

use cue_protocol::*;
use everything::{EverythingBackend, FileEntry};
use std::sync::{Arc, Mutex, OnceLock};

/// 次级动作 ID(顺序即菜单顺序;PRIMARY = 打开)。
const ACTION_REVEAL: ActionId = ActionId(1);
const ACTION_COPY_PATH: ActionId = ActionId(2);

/// 噪声目录排除总开关(设置 UI 里的 Bool 行)。
pub const KEY_EXCLUDE_NOISE: &str = "module.file.exclude_noise_paths";
/// 排除名单(设置 UI 里的 String 行,分号分隔的路径片段)。
pub const KEY_EXCLUDED_PATHS: &str = "module.file.excluded_paths";

/// 默认名单:全系统片段(node_modules 等依赖目录,任意位置都排除;
/// 口径对齐 VS Code search.exclude 默认)+ 按 USERPROFILE 展开的
/// AppData(Windows Search 默认即不索引)与各工具缓存目录。
/// 片段都以 `\` 结尾,锚定"目录"而非名字碰巧包含它的文件。
fn default_excluded_paths() -> String {
    let mut frags: Vec<String> = [
        r"C:\Windows\",
        r"C:\Program Files\",
        r"C:\Program Files (x86)\",
        r"\$Recycle.Bin\",
        r"\node_modules\",
        r"\.git\",
        r"\.svn\",
        r"\.hg\",
        r"\__pycache__\",
        r"\.venv\",
        r"\bower_components\",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let home = std::path::PathBuf::from(home);
        for d in [
            "AppData", ".vscode", ".cursor", ".cargo", ".rustup", ".gradle", ".m2", ".npm",
            ".nuget", ".docker", ".android",
        ] {
            frags.push(format!("{}\\", home.join(d).to_string_lossy()));
        }
    }
    frags.join(";")
}

/// 名单 → Everything 否定子句:含反斜杠的词按全路径子串匹配,
/// `!"…"` 即"路径不含该片段"。带引号容忍空格;大小写不敏感。
fn build_clause(list: &str) -> String {
    list.split(';')
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(|f| format!("!\"{f}\""))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 构造发给 Everything 的搜索串。查询含 `\` 视为显式路径输入,
/// 原样发送(逃生口:刻意找系统文件时不被默认排除拦截)。
fn effective_search(query: &str, exclude_noise: bool, clause: &str) -> String {
    if !exclude_noise || clause.is_empty() || query.contains('\\') {
        return query.to_string();
    }
    format!("{query} {clause}")
}

/// FileModule,trigger `/`。
pub struct FileModule {
    descriptor: ModuleDescriptor,
    /// load 时启动(专用 IPC 线程);未 load 时 None → query 报 Unavailable。
    backend: Option<EverythingBackend>,
    /// 文件夹 / 通用文件图标(load 后台线程一次性提取;同一 Arc
    /// 复用,UI 按 rgba 指针缓存纹理)。
    icons: Arc<OnceLock<icon::FileIcons>>,
    /// 最近一次 query 返回的 item id:图标晚到时据此发
    /// PresentationInvalidated,让 Core 重画可见行。
    last_items: Arc<Mutex<Vec<ItemId>>>,
    /// 噪声目录排除开关的当前值(load 时从设置快照读,try_apply 更新)。
    exclude_noise: bool,
    /// 排除名单编译出的 Everything 否定子句(load/try_apply 时重建)。
    exclude_clause: String,
}

impl FileModule {
    pub fn new() -> Self {
        Self {
            descriptor: ModuleDescriptor {
                id: ModuleId::from_static("file"),
                name: "Files",
                version: "0.1.0",
            },
            backend: None,
            icons: Arc::new(OnceLock::new()),
            last_items: Arc::new(Mutex::new(Vec::new())),
            exclude_noise: true,
            exclude_clause: build_clause(&default_excluded_paths()),
        }
    }
}

impl Default for FileModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for FileModule {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    /// load 廉价:只起两个线程——Everything IPC 线程(窗口与消息泵都在
    /// 线程内)与图标提取线程(两次 SHGetFileInfoW,毫秒级)。
    fn load(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        if let Some(SettingValue::Bool(v)) = ctx.settings.get("exclude_noise_paths") {
            self.exclude_noise = *v;
        }
        if let Some(SettingValue::String(v)) = ctx.settings.get("excluded_paths") {
            self.exclude_clause = build_clause(v);
        }
        self.backend = Some(EverythingBackend::start(ctx.logger.clone()));

        let icons = Arc::clone(&self.icons);
        let last_items = Arc::clone(&self.last_items);
        let sink = ctx.events.clone();
        let logger = ctx.logger.clone();
        std::thread::spawn(move || {
            let _com = cue_util_win::com::ComGuard::new();
            match icon::load_file_icons() {
                Some(loaded) => {
                    if icons.set(loaded).is_ok() {
                        let items = std::mem::take(&mut *last_items.lock().unwrap());
                        if !items.is_empty() {
                            sink.send(ModuleEvent::PresentationInvalidated { items });
                        }
                    }
                }
                None => logger.log(
                    LogLevel::Warn,
                    "file: 系统图标提取失败,行图标走 SystemIcon 兜底",
                ),
            }
        });
        Ok(())
    }

    fn unload(&mut self) {
        // IPC 线程随进程生命(同 AppModule catalog 线程);
        // icons OnceLock 不重建:幂等无害。
    }

    fn settings_schema(&self) -> SettingsSchema {
        vec![
            SettingSpec {
                key: SettingKey(KEY_EXCLUDE_NOISE.into()),
                label: "文件搜索:排除噪声目录".into(),
                description: Some(
                    "排除名单里的路径不出现在结果中;输入含 \\ 的显式路径时不过滤".into(),
                ),
                kind: SettingKind::Bool,
                default: SettingValue::Bool(true),
                apply_policy: ApplyPolicy::Immediate,
            },
            SettingSpec {
                key: SettingKey(KEY_EXCLUDED_PATHS.into()),
                label: "文件搜索:排除名单".into(),
                description: Some(
                    "分号分隔的路径片段,按全路径子串匹配;片段以 \\ 结尾锚定目录".into(),
                ),
                kind: SettingKind::String,
                default: SettingValue::String(default_excluded_paths()),
                apply_policy: ApplyPolicy::Immediate,
            },
        ]
    }

    fn try_apply_settings(&mut self, changes: SettingsChangeSet) -> Result<(), ModuleError> {
        for (key, value) in &changes.changes {
            match (key.0.as_ref(), value) {
                (KEY_EXCLUDE_NOISE, SettingValue::Bool(v)) => self.exclude_noise = *v,
                (KEY_EXCLUDED_PATHS, SettingValue::String(v)) => {
                    self.exclude_clause = build_clause(v);
                }
                (KEY_EXCLUDE_NOISE | KEY_EXCLUDED_PATHS, _) => {
                    return Err(ModuleError::InvalidState(format!("{} 类型不符", key.0)));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl LauncherModule for FileModule {
    fn launcher_descriptor(&self) -> LauncherDescriptor {
        LauncherDescriptor {
            trigger: Some("/".to_string()),
            is_default: false,
        }
    }

    /// 创建不触碰 IO——只往 latest-wins 槽投一个请求,future 内
    /// await 应答。空查询直接返回空(见模块头注释)。
    fn query(&mut self, ctx: QueryContext) -> QueryFuture {
        // 标点触发的剩余输入不去空白;Everything 语义上前导
        // 空白无意义,trim 掉。
        let search = ctx.query.trim().to_string();
        if search.is_empty() {
            return Box::pin(async { Ok(QueryResponse { items: Vec::new() }) });
        }
        let search = effective_search(&search, self.exclude_noise, &self.exclude_clause);
        let Some(backend) = self.backend.clone() else {
            return Box::pin(async {
                Err(ModuleError::Unavailable("file module not loaded".into()))
            });
        };
        let limit = ctx.result_limit;
        let last_items = Arc::clone(&self.last_items);
        Box::pin(async move {
            let entries = backend
                .query(search, limit as u32)
                .await
                // oneshot Canceled = 被更新的输入顶掉(latest-wins);
                // Core 反正会按 ticket 丢弃这个过期完成。
                .map_err(|_| ModuleError::QueryFailed("IPC 请求被取代".into()))??;
            let items: Vec<ModuleItem> = entries
                .into_iter()
                .map(|e| ModuleItem::new(ItemId(e.item_id()), e))
                .collect();
            *last_items.lock().unwrap() = items.iter().map(|i| i.id()).collect();
            Ok(QueryResponse { items })
        })
    }

    fn present(&self, item: &ModuleItem) -> ResultPresentation {
        let Some(entry) = item.downcast_ref::<FileEntry>() else {
            return ResultPresentation::new("<unknown item>");
        };
        let mut p = ResultPresentation::new(entry.name.clone());
        if !entry.parent.is_empty() {
            p.subtitle = Some(entry.parent.clone());
        }
        p.accessory = if entry.is_dir {
            Some(ResultAccessory::Text("文件夹".into()))
        } else {
            entry
                .size
                .map(|s| ResultAccessory::Text(format_size(s).into()))
        };
        p.icon = Some(match self.icons.get() {
            Some(icons) => {
                // IconImage.rgba 是 Arc<[u8]>,clone 保持指针不变——
                // UI 按该指针缓存纹理。
                let img = if entry.is_dir {
                    &icons.folder
                } else {
                    &icons.file
                };
                ResultIcon::Raster((**img).clone())
            }
            None => ResultIcon::SystemIcon(if entry.is_dir {
                SystemIconId::Folder
            } else {
                SystemIconId::File
            }),
        });
        p
    }

    /// 打开 / 打开所在文件夹 / 复制路径。
    fn actions(&self, _item: &ModuleItem) -> Vec<ActionDescriptor> {
        vec![
            ActionDescriptor {
                id: ActionId::PRIMARY,
                label: "打开".into(),
                shortcut: None,
            },
            ActionDescriptor {
                id: ACTION_REVEAL,
                label: "打开所在文件夹".into(),
                shortcut: None,
            },
            ActionDescriptor {
                id: ACTION_COPY_PATH,
                label: "复制路径".into(),
                shortcut: None,
            },
        ]
    }

    /// Open = ShellExecute 默认动词:文件由系统关联程序打开,文件夹
    /// 进资源管理器。usage 身份 = 全路径(稳定启动标识)。
    fn activate(&mut self, item: &ModuleItem, action: ActionId) -> ActivationFuture {
        let entry = item.downcast_ref::<FileEntry>().cloned();
        Box::pin(async move {
            let Some(entry) = entry else {
                return ModuleOutcome::failed(ModuleError::InvalidState(
                    "item payload is not a FileEntry".into(),
                ));
            };
            let result = match action {
                ActionId::PRIMARY => cue_util_win::shell::shell_execute(&entry.path, None, None),
                ACTION_REVEAL => cue_util_win::shell::reveal_in_explorer(&entry.path),
                ACTION_COPY_PATH => cue_util_win::clipboard::set_text(&entry.path),
                _ => Err(ModuleError::ActivationFailed(format!(
                    "unknown action {action:?}"
                ))),
            };
            match result {
                Ok(()) => ModuleOutcome::success(
                    SessionDisposition::Close,
                    Some(UsageRecordRequest {
                        item_key: entry.path.to_string(),
                        action_id: action,
                    }),
                ),
                Err(e) => ModuleOutcome::failed(e),
            }
        })
    }
}

/// 行右 accessory 的尺寸文案:B 整数,KB/MB/GB/TB 一位小数。
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;
    let (v, unit) = if bytes >= TB {
        (bytes as f64 / TB as f64, "TB")
    } else if bytes >= GB {
        (bytes as f64 / GB as f64, "GB")
    } else if bytes >= MB {
        (bytes as f64 / MB as f64, "MB")
    } else if bytes >= KB {
        (bytes as f64 / KB as f64, "KB")
    } else {
        return format!("{bytes} B");
    };
    format!("{v:.1} {unit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool, size: Option<u64>) -> FileEntry {
        everything::test_entry(path, is_dir, size)
    }

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(12_345), "12.1 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_size(2 * 1024 * 1024 * 1024), "2.0 GB");
        assert_eq!(format_size(3 * 1024u64.pow(4)), "3.0 TB");
    }

    /// 打开 / 打开所在文件夹 / 复制路径,顺序即菜单顺序。
    #[test]
    fn actions_are_open_reveal_copy() {
        let m = FileModule::new();
        let file = ModuleItem::new(ItemId(1), entry("C:\\Alpha\\beta.txt", false, None));
        let actions = m.actions(&file);
        assert_eq!(
            actions.iter().map(|a| a.id).collect::<Vec<_>>(),
            [ActionId::PRIMARY, ACTION_REVEAL, ACTION_COPY_PATH]
        );
        assert_eq!(
            actions.iter().map(|a| &*a.label).collect::<Vec<_>>(),
            ["打开", "打开所在文件夹", "复制路径"]
        );
    }

    #[test]
    fn present_file_and_folder() {
        let m = FileModule::new();
        let dir = ModuleItem::new(ItemId(1), entry("C:\\Alpha", true, None));
        let p = m.present(&dir);
        assert_eq!(&*p.title, "Alpha");
        assert_eq!(p.subtitle.as_deref(), Some("C:"));
        assert!(matches!(&p.accessory, Some(ResultAccessory::Text(t)) if &**t == "文件夹"));
        // 图标线程未跑 → SystemIcon 兜底
        assert!(matches!(
            p.icon,
            Some(ResultIcon::SystemIcon(SystemIconId::Folder))
        ));

        let file = ModuleItem::new(ItemId(2), entry("C:\\Alpha\\beta.txt", false, Some(2048)));
        let p = m.present(&file);
        assert_eq!(&*p.title, "beta.txt");
        assert_eq!(p.subtitle.as_deref(), Some("C:\\Alpha"));
        assert!(matches!(&p.accessory, Some(ResultAccessory::Text(t)) if &**t == "2.0 KB"));
        assert!(matches!(
            p.icon,
            Some(ResultIcon::SystemIcon(SystemIconId::File))
        ));

        // 盘符根:parent 为空 → 无副标题
        let root = ModuleItem::new(ItemId(3), entry("C:\\", true, None));
        let p = m.present(&root);
        assert_eq!(&*p.title, "C:\\");
        assert!(p.subtitle.is_none());
    }

    /// 空查询(含纯空白)返回空,不触碰 backend(给不出 Top Files)。
    #[test]
    fn empty_query_returns_empty_without_backend() {
        let mut m = FileModule::new();
        for q in ["", "   "] {
            let r = futures::executor::block_on(m.query(QueryContext {
                query: q.into(),
                result_limit: 8,
            }))
            .expect("empty query ok");
            assert!(r.items.is_empty());
        }
    }

    /// load 之前的非空查询 → Unavailable,不 panic。
    #[test]
    fn query_before_load_is_unavailable() {
        let mut m = FileModule::new();
        let r = futures::executor::block_on(m.query(QueryContext {
            query: "cue".into(),
            result_limit: 8,
        }));
        assert!(matches!(r, Err(ModuleError::Unavailable(_))));
    }

    /// `/ cue` 这种带前导空白的剩余输入(标点触发不去空白),
    /// 模块内 trim——Everything 语义上前导空白无意义。
    #[test]
    fn query_trims_whitespace() {
        let mut m = FileModule::new();
        // trim 后为空 → 走空查询路径(不需要 backend)
        let r = futures::executor::block_on(m.query(QueryContext {
            query: "  ".into(),
            result_limit: 8,
        }))
        .expect("ok");
        assert!(r.items.is_empty());
    }

    /// 默认排除噪声目录:普通查询拼上否定子句;关掉开关、空名单、
    /// 或查询含 `\`(显式路径)时原样发送。
    #[test]
    fn effective_search_excludes_noise_by_default() {
        let clause = build_clause(&default_excluded_paths());
        let noisy = effective_search("ds", true, &clause);
        assert!(noisy.starts_with("ds "));
        for frag in [
            r#"!"C:\Windows\""#,
            r#"!"C:\Program Files\""#,
            r#"!"C:\Program Files (x86)\""#,
            r#"!"\$Recycle.Bin\""#,
            r#"!"\node_modules\""#,
            r#"!"\.git\""#,
            r#"\AppData\""#,
            r#"\.vscode\""#,
            r#"\.cargo\""#,
        ] {
            assert!(noisy.contains(frag), "missing {frag} in {noisy}");
        }
        assert_eq!(effective_search("ds", false, &clause), "ds");
        assert_eq!(effective_search("ds", true, ""), "ds");
        assert_eq!(
            effective_search(r"C:\Windows\explorer", true, &clause),
            r"C:\Windows\explorer"
        );
        // Everything 查询函数(如 ext:)不含反斜杠,仍走默认排除。
        assert!(effective_search("ext:pdf report", true, &clause).contains(r#"!"C:\Windows\""#));
    }

    /// 名单解析:分号分隔、trim、空段跳过,编译成 `!"片段"` 子句。
    #[test]
    fn build_clause_parses_semicolon_list() {
        assert_eq!(build_clause(""), "");
        assert_eq!(build_clause(" ; ; "), "");
        assert_eq!(
            build_clause(r" C:\Windows\ ; \node_modules\;"),
            r#"!"C:\Windows\" !"\node_modules\""#
        );
    }

    /// 名单即设置:schema 声明 Bool 总开关 + String 名单,try_apply
    /// 重建子句;类型错误返回 Err 而不是 panic。
    #[test]
    fn exclude_settings_roundtrip() {
        let mut m = FileModule::new();
        assert!(m.exclude_noise);
        assert!(!m.exclude_clause.is_empty());
        let schema = m.settings_schema();
        assert_eq!(schema.len(), 2);
        assert_eq!(schema[0].key.0.as_ref(), KEY_EXCLUDE_NOISE);
        assert_eq!(schema[0].kind, SettingKind::Bool);
        assert_eq!(schema[1].key.0.as_ref(), KEY_EXCLUDED_PATHS);
        assert_eq!(schema[1].kind, SettingKind::String);
        assert!(matches!(&schema[1].default, SettingValue::String(s) if s.contains(r"\AppData\")));

        let mut cs = SettingsChangeSet::default();
        cs.changes.push((
            SettingKey(KEY_EXCLUDED_PATHS.into()),
            SettingValue::String(r"\scratch\".into()),
        ));
        m.try_apply_settings(cs).expect("apply ok");
        assert_eq!(m.exclude_clause, r#"!"\scratch\""#);

        let mut off = SettingsChangeSet::default();
        off.changes.push((
            SettingKey(KEY_EXCLUDE_NOISE.into()),
            SettingValue::Bool(false),
        ));
        m.try_apply_settings(off).expect("apply ok");
        assert!(!m.exclude_noise);

        let mut bad = SettingsChangeSet::default();
        bad.changes.push((
            SettingKey(KEY_EXCLUDED_PATHS.into()),
            SettingValue::Bool(true),
        ));
        assert!(m.try_apply_settings(bad).is_err());
        assert_eq!(m.exclude_clause, r#"!"\scratch\""#); // 失败不留半拉子状态
    }
}
