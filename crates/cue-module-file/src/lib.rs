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
//! 片段"`),让这些路径根本不占结果位。名单是模块数据文件
//! `modules/file/data/excluded-paths.toml`——给人编辑的配置一律
//! TOML(literal string 数组,反斜杠免转义;默认口径参照 VS Code
//! search.exclude 与 Windows Search 默认索引范围);设置页有一行
//! 指向它的 Path 设置,回车即用系统默认编辑器打开,保存后下一次
//! 查询生效(mtime 指纹重读,无 watcher;语法错误保留旧子句——
//! 编辑器里的半保存状态不该打烂搜索)。总开关是
//! `module.file.exclude_noise_paths`。查询含 `\`(用户在写显式
//! 路径)时原样发送、不加排除——刻意找系统文件时不会被拦。

mod everything;
mod icon;

use cue_protocol::*;
use everything::{EverythingBackend, FileEntry};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

/// 次级动作 ID(顺序即菜单顺序;PRIMARY = 打开)。
const ACTION_REVEAL: ActionId = ActionId(1);
const ACTION_COPY_PATH: ActionId = ActionId(2);

/// 噪声目录排除总开关(设置 UI 里的 Bool 行)。
pub const KEY_EXCLUDE_NOISE: &str = "module.file.exclude_noise_paths";
/// 名单文件的 Path 设置(设置 UI 里回车打开;值只是指针,
/// 名单内容归模块数据文件,不是设置值)。
pub const KEY_EXCLUDE_FILE: &str = "module.file.excluded_paths_file";
/// 名单文件名(模块 data 目录下)。
const EXCLUDE_FILE_NAME: &str = "excluded-paths.toml";

/// 默认名单片段:全系统(node_modules 等依赖目录,任意位置都排除;
/// 口径对齐 VS Code search.exclude 默认)+ 按 USERPROFILE 展开的
/// AppData(Windows Search 默认即不索引)与各工具缓存目录。
/// 片段都以 `\` 结尾,锚定"目录"而非名字碰巧包含它的文件。
fn default_fragments() -> Vec<String> {
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
        let home = PathBuf::from(home);
        for d in [
            "AppData", ".vscode", ".cursor", ".cargo", ".rustup", ".gradle", ".m2", ".npm",
            ".nuget", ".docker", ".android",
        ] {
            frags.push(format!("{}\\", home.join(d).to_string_lossy()));
        }
    }
    frags
}

/// 首启播种:注释头(格式与逃生口说明)+ 默认片段。片段都是
/// Windows 路径——TOML literal string(单引号)内容逐字,
/// 反斜杠免转义,是这类名单的天然容器。
fn seed_exclude_file(path: &Path) -> std::io::Result<()> {
    let mut text = String::from(
        "# CUE 文件搜索排除名单\n\
         # excluded 数组:每个片段按全路径子串匹配;以 \\ 结尾锚定目录。\n\
         # 保存后下一次查询生效;清空数组 = 不排除任何路径。\n\
         # 查询含 \\ 时本名单整体不生效(显式路径逃生口)。\n\
         # 路径用单引号 literal string:反斜杠逐字,无需转义。\n\
         \n\
         excluded = [\n",
    );
    for f in default_fragments() {
        // 默认片段均不含单引号/换行(literal string 的两个禁区)。
        text.push_str(&format!("  '{f}',\n"));
    }
    text.push_str("]\n");
    std::fs::write(path, text)
}

/// Path 设置的默认值:与编排层同一公式解析的模块数据路径
/// (schema 在 load 之前注册,拿不到 ModuleContext,只能从环境重算;
/// 生产环境两者一致,测试里这行只是展示值)。
fn default_exclude_file() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(|p| PathBuf::from(p).join("CUE"))
        .unwrap_or_else(|_| PathBuf::from("CUE"))
        .join("modules")
        .join("file")
        .join("data")
        .join(EXCLUDE_FILE_NAME)
}

/// TOML → 片段数组:无 `excluded` 键 = 空名单;语法错误、
/// 非字符串元素报 Err(调用方保留旧子句)。
fn parse_fragments(content: &str) -> Result<Vec<String>, String> {
    let doc: toml::Value = toml::from_str(content).map_err(|e| e.to_string())?;
    let Some(items) = doc.get("excluded").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    items
        .iter()
        .enumerate()
        .map(|(i, v)| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("excluded[{i}] 不是字符串"))
        })
        .collect()
}

/// 片段 → Everything 否定子句:含反斜杠的词按全路径子串匹配,
/// `!"…"` 即"路径不含该片段"。带引号容忍空格;大小写不敏感。
fn clause_from_fragments(frags: &[String]) -> String {
    frags
        .iter()
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .map(|f| format!("!\"{f}\""))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 名单文件内容 → Everything 否定子句。
fn build_clause(content: &str) -> Result<String, String> {
    Ok(clause_from_fragments(&parse_fragments(content)?))
}

/// 构造发给 Everything 的搜索串。查询含 `\` 视为显式路径输入,
/// 原样发送(逃生口:刻意找系统文件时不被默认排除拦截)。
fn effective_search(query: &str, exclude_noise: bool, clause: &str) -> String {
    if !exclude_noise || clause.is_empty() || query.contains('\\') {
        return query.to_string();
    }
    format!("{query} {clause}")
}

/// 名单的共享视图:query future 在后台线程做 mtime 指纹检查,
/// 变了才重读重编译(UI 线程零 IO,查询创建预算不破)。
struct ExcludeState {
    /// 名单文件路径(load 后才有;测试里 None = 固定内置子句)。
    path: Option<PathBuf>,
    /// 上次读到的文件修改时间(含解析失败的版本——见过即记,
    /// 免得每次查询都重读重报)。
    mtime: Option<SystemTime>,
    /// 当前编译出的否定子句。
    clause: String,
    /// 解析失败告警(load 后才有)。
    logger: Option<ModuleLogger>,
}

/// 后台线程侧:mtime 变了才重读文件、重编译子句;stat/读失败
/// 或 TOML 语法错误保留旧子句(编辑器里的半保存状态不该打烂
/// 搜索)。返回当前子句。
fn refreshed_clause(state: &Mutex<ExcludeState>) -> String {
    let path = state.lock().unwrap().path.clone();
    if let Some(p) = path {
        let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
        let stale = mtime.is_some() && mtime != state.lock().unwrap().mtime;
        if stale && let Ok(content) = std::fs::read_to_string(&p) {
            let mut g = state.lock().unwrap();
            match build_clause(&content) {
                Ok(clause) => g.clause = clause,
                Err(e) => {
                    if let Some(logger) = &g.logger {
                        logger.log(
                            LogLevel::Warn,
                            &format!("file: 排除名单解析失败({e}),沿用旧名单"),
                        );
                    }
                }
            }
            g.mtime = mtime;
        }
    }
    state.lock().unwrap().clause.clone()
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
    /// 排除名单(模块数据文件 + mtime 指纹;future 后台重读)。
    exclude: Arc<Mutex<ExcludeState>>,
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
            exclude: Arc::new(Mutex::new(ExcludeState {
                path: None,
                mtime: None,
                clause: clause_from_fragments(&default_fragments()),
                logger: None,
            })),
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
    /// 线程内)与图标提取线程(两次 SHGetFileInfoW,毫秒级)。名单文件
    /// 播种 + 首读是两次小文件 IO(缺失才写),微秒级。
    fn load(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        if let Some(SettingValue::Bool(v)) = ctx.settings.get("exclude_noise_paths") {
            self.exclude_noise = *v;
        }
        // 名单文件:缺失则播种默认名单;读取/解析失败沿用 new()
        // 里的内置默认子句——排除是体验优化,不该阻塞 load。
        let file = ctx.storage.data.join(EXCLUDE_FILE_NAME);
        if !file.exists()
            && let Err(e) = seed_exclude_file(&file)
        {
            ctx.logger.log(
                LogLevel::Warn,
                &format!("file: 排除名单播种失败({e}),沿用内置默认"),
            );
        }
        {
            let mut g = self.exclude.lock().unwrap();
            match std::fs::read_to_string(&file)
                .map_err(|e| e.to_string())
                .and_then(|c| build_clause(&c))
            {
                Ok(clause) => {
                    g.clause = clause;
                    g.mtime = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
                }
                Err(e) => ctx.logger.log(
                    LogLevel::Warn,
                    &format!("file: 排除名单读取/解析失败({e}),沿用内置默认"),
                ),
            }
            g.path = Some(file);
            g.logger = Some(ctx.logger.clone());
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
                key: SettingKey(KEY_EXCLUDE_FILE.into()),
                label: "文件搜索:排除名单文件".into(),
                description: Some("回车用系统默认编辑器打开;TOML 数组,保存后下一次查询生效".into()),
                kind: SettingKind::Path,
                // schema 注册先于 load(拿不到 ModuleContext),路径按
                // 编排层同一公式从环境重算。
                default: SettingValue::Path(default_exclude_file()),
                apply_policy: ApplyPolicy::Immediate,
            },
        ]
    }

    fn try_apply_settings(&mut self, changes: SettingsChangeSet) -> Result<(), ModuleError> {
        for (key, value) in &changes.changes {
            match (key.0.as_ref(), value) {
                (KEY_EXCLUDE_NOISE, SettingValue::Bool(v)) => self.exclude_noise = *v,
                (KEY_EXCLUDE_NOISE, _) => {
                    return Err(ModuleError::InvalidState(format!("{} 类型不符", key.0)));
                }
                // Path 行的值只是文件指针,打开动作不产生变更;
                // 名单内容模块自己从文件读,不经设置事务。
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
    /// await 应答。空查询直接返回空(见模块头注释)。名单的 mtime
    /// 指纹检查也在 future 里做(后台线程,一次 stat 亚毫秒)。
    fn query(&mut self, ctx: QueryContext) -> QueryFuture {
        // 标点触发的剩余输入不去空白;Everything 语义上前导
        // 空白无意义,trim 掉。
        let search = ctx.query.trim().to_string();
        if search.is_empty() {
            return Box::pin(async { Ok(QueryResponse { items: Vec::new() }) });
        }
        let exclude_noise = self.exclude_noise;
        let exclude = Arc::clone(&self.exclude);
        let Some(backend) = self.backend.clone() else {
            return Box::pin(async {
                Err(ModuleError::Unavailable("file module not loaded".into()))
            });
        };
        let limit = ctx.result_limit;
        let last_items = Arc::clone(&self.last_items);
        Box::pin(async move {
            let clause = refreshed_clause(&exclude);
            let search = effective_search(&search, exclude_noise, &clause);
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
        let clause = clause_from_fragments(&default_fragments());
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

    /// TOML 解析:literal string 反斜杠逐字、注释与空行自由、
    /// 无 excluded 键 = 空名单;语法错误与非字符串元素报 Err。
    #[test]
    fn build_clause_parses_toml_list() {
        assert_eq!(build_clause("").unwrap(), "");
        assert_eq!(build_clause("# 只有注释\n").unwrap(), "");
        assert_eq!(
            build_clause(
                "# 系统目录\nexcluded = [\n  'C:\\Windows\\',\n  '\\node_modules\\', # 依赖\n]\n"
            )
            .unwrap(),
            r#"!"C:\Windows\" !"\node_modules\""#
        );
        // 单行数组 + 基本字符串(双引号,反斜杠需转义)也能解析。
        assert_eq!(
            build_clause("excluded = [\"C:\\\\Windows\\\\\"]").unwrap(),
            r#"!"C:\Windows\""#
        );
        assert!(build_clause("excluded = ['unterminated").is_err());
        assert!(build_clause("excluded = [1]").is_err());
    }

    /// 播种的文件带注释头与默认片段,且能编译出子句。
    #[test]
    fn seed_file_roundtrips_into_clause() {
        let dir = std::env::temp_dir().join(format!("cue-file-seed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(EXCLUDE_FILE_NAME);
        seed_exclude_file(&file).unwrap();
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.starts_with("# CUE"));
        let clause = build_clause(&content).expect("seed parses");
        assert!(clause.contains(r#"!"\node_modules\""#));
        assert!(clause.contains(r#"\AppData\""#));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// mtime 指纹:变了才重读重编译;文件消失/读取失败保留旧子句。
    #[test]
    fn refreshed_clause_follows_mtime() {
        let dir = std::env::temp_dir().join(format!("cue-file-mtime-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("list.toml");
        std::fs::write(&file, "excluded = ['\\alpha\\']\n").unwrap();

        let state = Mutex::new(ExcludeState {
            path: Some(file.clone()),
            mtime: None,
            clause: "old".into(),
            logger: None,
        });
        // mtime None ≠ Some → 首读
        assert_eq!(refreshed_clause(&state), r#"!"\alpha\""#);
        let first_mtime = state.lock().unwrap().mtime.unwrap();

        // 内容变了但 mtime 没变(写后强制回拨)→ 不重读
        std::fs::write(&file, "excluded = ['\\beta\\']\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(first_mtime)
            .unwrap();
        assert_eq!(refreshed_clause(&state), r#"!"\alpha\""#);

        // 推进 mtime → 重读
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(first_mtime + std::time::Duration::from_secs(10))
            .unwrap();
        assert_eq!(refreshed_clause(&state), r#"!"\beta\""#);

        // 文件消失 → 保留旧子句,不 panic
        std::fs::remove_file(&file).unwrap();
        assert_eq!(refreshed_clause(&state), r#"!"\beta\""#);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 语法错误(编辑器半保存):保留旧子句,但 mtime 照记——
    /// 同一坏版本不重复重读重报;改对之后正常生效。
    #[test]
    fn malformed_toml_keeps_previous_clause() {
        let dir = std::env::temp_dir().join(format!("cue-file-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("list.toml");
        std::fs::write(&file, "excluded = ['\\alpha\\']\n").unwrap();

        let state = Mutex::new(ExcludeState {
            path: Some(file.clone()),
            mtime: None,
            clause: "old".into(),
            logger: None,
        });
        assert_eq!(refreshed_clause(&state), r#"!"\alpha\""#);
        let bump = |secs: u64| {
            let t = state.lock().unwrap().mtime.unwrap() + std::time::Duration::from_secs(secs);
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(t)
                .unwrap();
        };

        // 写坏 + 推进 mtime → 子句不动,mtime 已记
        std::fs::write(&file, "excluded = ['oops\n").unwrap();
        bump(10);
        assert_eq!(refreshed_clause(&state), r#"!"\alpha\""#);
        let seen = state.lock().unwrap().mtime.unwrap();

        // 同一坏版本再查:不重读(把文件改回 alpha 但回拨 mtime,子句不变)
        std::fs::write(&file, "excluded = ['\\alpha\\']\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(seen)
            .unwrap();
        assert_eq!(refreshed_clause(&state), r#"!"\alpha\""#);

        // 改对 + 推进 → 生效
        std::fs::write(&file, "excluded = ['\\beta\\']\n").unwrap();
        bump(10);
        assert_eq!(refreshed_clause(&state), r#"!"\beta\""#);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// schema 声明 Bool 总开关 + Path 名单文件;try_apply 只管开关,
    /// 类型错误返回 Err 而不是 panic。
    #[test]
    fn exclude_settings_roundtrip() {
        let mut m = FileModule::new();
        assert!(m.exclude_noise);
        assert!(!m.exclude.lock().unwrap().clause.is_empty());
        let schema = m.settings_schema();
        assert_eq!(schema.len(), 2);
        assert_eq!(schema[0].key.0.as_ref(), KEY_EXCLUDE_NOISE);
        assert_eq!(schema[0].kind, SettingKind::Bool);
        assert_eq!(schema[1].key.0.as_ref(), KEY_EXCLUDE_FILE);
        assert_eq!(schema[1].kind, SettingKind::Path);
        assert!(
            matches!(&schema[1].default, SettingValue::Path(p) if p.ends_with(EXCLUDE_FILE_NAME))
        );

        let mut off = SettingsChangeSet::default();
        off.changes.push((
            SettingKey(KEY_EXCLUDE_NOISE.into()),
            SettingValue::Bool(false),
        ));
        m.try_apply_settings(off).expect("apply ok");
        assert!(!m.exclude_noise);

        let mut bad = SettingsChangeSet::default();
        bad.changes.push((
            SettingKey(KEY_EXCLUDE_NOISE.into()),
            SettingValue::String("x".into()),
        ));
        assert!(m.try_apply_settings(bad).is_err());
        assert!(!m.exclude_noise); // 失败不留半拉子状态
    }
}
