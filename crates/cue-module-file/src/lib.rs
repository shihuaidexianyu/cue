//! cue-module-file —— FileModule。
//!
//! `/` 触发的文件搜索:触发词是标点,verbatim 匹配(词边界只约束
//! 字母触发);`/` 之后的输入原样作为搜索串——查询语义(子串 AND、
//! `ext:` 子集、路径过滤)归模块,Core 不解析。数据源是**自建文件
//! 索引**(§138:限定范围遍历 + ReadDirectoryChangesW 增量维护,
//! 无第三方依赖、无管理员特权;取代 §118 的 Everything IPC)。
//! 文件与文件夹同一模态;FileEntry 只在模块内部,Core 只见 ItemId。
//!
//! 空查询返回空:UsageRead 只能按键查、不能枚举,给不出 Top Files,
//! 不显示任何推荐内容。排序 = 文件名命中优先,平手名字升序;
//! V1 不做 usage 重排。
//!
//! 噪声目录默认排除:工作文件几乎从不在系统目录、AppData、包缓存、
//! 编辑器扩展目录里,但它们会以"工具内脏"的形式淹没结果。名单是
//! 模块数据文件 `modules/file/data/excluded-paths.toml`——给人编辑
//! 的配置一律 TOML(literal string 数组,反斜杠免转义);设置页有
//! 一行指向它的 Path 设置,回车即用系统默认编辑器打开,保存后下
//! 一次查询生效(mtime 指纹重读,无 watcher;语法错误保留旧名单——
//! 编辑器里的半保存状态不该打烂搜索)。总开关是
//! `module.file.exclude_noise_paths`。名单双段生效(§138):爬取时
//! 对命中片段的目录整棵剪枝(不进索引),查询时对文件级片段再
//! 过滤;查询含 `\`(显式路径)时查询级过滤不生效——逃生口,但
//! 够不到被剪枝的子树。名单/开关变更触发索引后台全量重爬。

mod icon;
mod index;

use cue_protocol::*;
use index::{FileEntry, FileIndex};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
/// 额外索引根目录(§138):分号分隔;默认已覆盖 %USERPROFILE%
/// 与桌面/文档/下载。改动重启后生效(索引只在进程启动时建)。
const KEY_INDEX_DIRS: &str = "module.file.index_dirs";
/// 名单文件名(模块 data 目录下)。
const EXCLUDE_FILE_NAME: &str = "excluded-paths.toml";

/// 通用图标未就绪时的兜底字形(Segoe MDL2/Fluent,§135)。
mod glyph_cp {
    pub const FOLDER: u32 = 0xE8B7;
    pub const FILE: u32 = 0xE8A5;
}

/// 默认名单片段(§125):系统目录(含 ProgramData)+ 目录锚定的
/// 通用 `\AppData\`(任意用户配置,含多配置/沙箱配置)+ 依赖目录
/// (口径对齐 VS Code search.exclude 默认)+ 按 USERPROFILE 展开
/// 的工具缓存(项目级同名目录多是配置而非缓存,不按通用排除)。
/// 片段都以 `\` 结尾,锚定"目录"而非名字碰巧包含它的文件。
fn default_fragments() -> Vec<String> {
    let mut frags: Vec<String> = [
        r"C:\Windows\",
        r"C:\Program Files\",
        r"C:\Program Files (x86)\",
        r"C:\ProgramData\",
        r"\$Recycle.Bin\",
        r"\AppData\",
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
            ".vscode", ".cursor", ".cargo", ".rustup", ".gradle", ".m2", ".npm", ".nuget",
            ".docker", ".android",
        ] {
            frags.push(format!("{}\\", home.join(d).to_string_lossy()));
        }
    }
    frags
}

/// 旧版默认名单(§121,仅用于存量升级判定):AppData 与工具缓存
/// 都按 USERPROFILE 展开,其他配置的同名目录管不到;无 ProgramData。
fn legacy_default_fragments() -> Vec<String> {
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

/// 存量一次性升级(§125):文件内容恰为旧版默认名单(用户一个
/// 片段都没动过)时重写为新默认;用户增删过任何片段即不触碰。
/// 读取/解析失败、写入失败都不致命(沿用现状)。返回是否升级。
fn upgrade_seed_if_legacy(path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(frags) = parse_fragments(&content) else {
        return false;
    };
    if frags != legacy_default_fragments() {
        return false;
    }
    seed_exclude_file(path).is_ok()
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

/// 片段归一化:trim、去空、小写——索引匹配是大小写不敏感的
/// 全路径子串(§138;不再是 Everything 查询语法,引号无需特判)。
fn normalize_fragments(frags: &[String]) -> Vec<String> {
    frags
        .iter()
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .map(|f| f.to_lowercase())
        .collect()
}

/// 名单文件内容 → 归一化片段数组。
fn build_fragments(content: &str) -> Result<Vec<String>, String> {
    parse_fragments(content).map(|f| normalize_fragments(&f))
}

/// 名单的共享视图:query future 在后台线程做 mtime 指纹检查,
/// 变了才重读(UI 线程零 IO,查询创建预算不破)。
pub(crate) struct ExcludeState {
    /// 名单文件路径(load 后才有;测试里 None = 固定内置名单)。
    path: Option<PathBuf>,
    /// 上次读到的文件修改时间(含解析失败的版本——见过即记,
    /// 免得每次查询都重读重报)。
    mtime: Option<SystemTime>,
    /// 当前生效的归一化片段(小写)。
    fragments: Vec<String>,
    /// 解析失败告警(load 后才有)。
    logger: Option<ModuleLogger>,
}

/// 后台线程侧:mtime 变了才重读文件;stat/读失败或 TOML 语法错误
/// 保留旧名单(编辑器里的半保存状态不该打烂搜索)。两阶段:先快照
/// 路径与已知 mtime,IO 在锁外做,提交时再锁——并发查询重复读同
/// 一版本无害(同内容幂等,后到覆盖)。返回 (片段, 本次是否变更)——
/// 变更时调用方应触发索引全量重爬(§138:名单的爬取剪枝口径变了)。
pub(crate) fn refreshed_fragments(state: &Mutex<ExcludeState>) -> (Vec<String>, bool) {
    let (path, known_mtime) = {
        let g = state.lock().unwrap();
        (g.path.clone(), g.mtime)
    };
    let mut changed = false;
    if let Some(p) = path {
        let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
        if mtime.is_some()
            && mtime != known_mtime
            && let Ok(content) = std::fs::read_to_string(&p)
        {
            let mut g = state.lock().unwrap();
            match build_fragments(&content) {
                Ok(fragments) => g.fragments = fragments,
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
            changed = true;
        }
    }
    (state.lock().unwrap().fragments.clone(), changed)
}

/// FileModule,trigger `/`。
pub struct FileModule {
    descriptor: ModuleDescriptor,
    /// load 时启动(索引线程 + watcher 线程);未 load 时 None →
    /// query 报 Unavailable。
    index: Option<FileIndex>,
    /// 文件夹 / 通用文件图标(worker 启动时最先提取;同一 Arc
    /// 复用,UI 按 rgba 指针缓存纹理)。具体文件的真实图标走
    /// worker 的按路径/扩展名缓存。
    icons: Arc<OnceLock<icon::FileIcons>>,
    /// 真实图标提取队列(load 后才有)。
    icon_worker: Option<icon::IconWorker>,
    /// 最近一次 query 返回的 item id:图标晚到时据此发
    /// PresentationInvalidated,让 Core 重画可见行。
    last_items: Arc<Mutex<Vec<ItemId>>>,
    /// 噪声目录排除开关(设置即时生效;索引线程与查询共享)。
    exclude_noise: Arc<AtomicBool>,
    /// 排除名单(模块数据文件 + mtime 指纹;future 后台重读)。
    exclude: Arc<Mutex<ExcludeState>>,
}

impl FileModule {
    pub fn new() -> Self {
        Self {
            descriptor: ModuleDescriptor {
                id: ModuleId::from_static("file"),
                name: "文件",
                version: "0.1.0",
            },
            index: None,
            icons: Arc::new(OnceLock::new()),
            icon_worker: None,
            last_items: Arc::new(Mutex::new(Vec::new())),
            exclude_noise: Arc::new(AtomicBool::new(true)),
            exclude: Arc::new(Mutex::new(ExcludeState {
                path: None,
                mtime: None,
                fragments: normalize_fragments(&default_fragments()),
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

impl FileModule {
    /// 图标:非文件夹先试真实图标(worker 按路径/扩展名缓存,
    /// 未命中登记提取、本帧走通用);文件夹与各种兜底走
    /// 通用图标,再退 Segoe 字形(§135)。
    fn icon_for(&self, entry: &FileEntry) -> Option<ResultIcon> {
        if !entry.is_dir
            && let Some(worker) = &self.icon_worker
            && let Some(icon) = worker.get_or_queue(Path::new(entry.path.as_ref()), false)
        {
            return Some(icon);
        }
        match self.icons.get() {
            Some(icons) => {
                // IconImage.rgba 是 Arc<[u8]>,clone 保持指针不变——
                // UI 按该指针缓存纹理。
                let img = if entry.is_dir {
                    &icons.folder
                } else {
                    &icons.file
                };
                Some(ResultIcon::Raster((**img).clone()))
            }
            // 通用图标集未初始化(单测/worker 未起)→ 字形兜底;
            // 字体缺失时返回 None(空槽位,不影响激活)。
            None => {
                let cp = if entry.is_dir {
                    glyph_cp::FOLDER
                } else {
                    glyph_cp::FILE
                };
                cue_util_win::glyph::cached_glyph(cp, cue_util_win::glyph::DEFAULT_RGB)
                    .map(ResultIcon::Raster)
            }
        }
    }
}

impl Module for FileModule {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    /// load 廉价:起三拨线程——索引线程(首爬在后台,查询经就绪
    /// 门等待)与每根一个 watcher、图标 worker(先提两枚通用图标,
    /// 随后串行服务按文件真实图标的提取队列)。名单文件播种 + 存量
    /// 升级判定 + 首读是几次小文件 IO(缺失才写;内容恰为旧默认才
    /// 重写),微秒级。
    fn load(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        if let Some(SettingValue::Bool(v)) = ctx.settings.get("exclude_noise_paths") {
            self.exclude_noise.store(*v, Ordering::Relaxed);
        }
        // 名单文件:缺失则播种默认名单;读取/解析失败沿用 new()
        // 里的内置默认名单——排除是体验优化,不该阻塞 load。
        let file = ctx.storage.data.join(EXCLUDE_FILE_NAME);
        if !file.exists()
            && let Err(e) = seed_exclude_file(&file)
        {
            ctx.logger.log(
                LogLevel::Warn,
                &format!("file: 排除名单播种失败({e}),沿用内置默认"),
            );
        }
        // 存量一次性升级:内容恰为旧默认(用户没改过)才重写(§125)。
        if file.exists() && upgrade_seed_if_legacy(&file) {
            ctx.logger.log(
                LogLevel::Info,
                "file: 排除名单为旧版默认,已升级为新默认(通用 \\AppData\\ + ProgramData)",
            );
        }
        {
            let mut g = self.exclude.lock().unwrap();
            match std::fs::read_to_string(&file)
                .map_err(|e| e.to_string())
                .and_then(|c| build_fragments(&c))
            {
                Ok(fragments) => {
                    g.fragments = fragments;
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
        // §138:默认根(%USERPROFILE% + 桌面/文档/下载)∪ 用户声明
        // 的额外根;去重去嵌套由 index 内部完成。
        let mut roots = index::default_roots();
        if let Some(SettingValue::String(s)) = ctx.settings.get("index_dirs") {
            roots.extend(
                s.split(';')
                    .map(|p| p.trim())
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from),
            );
        }
        self.index = Some(FileIndex::start(
            index::dedup_roots(roots),
            Arc::clone(&self.exclude),
            Arc::clone(&self.exclude_noise),
            ctx.logger.clone(),
        ));
        self.icon_worker = Some(icon::IconWorker::new(
            Arc::clone(&self.icons),
            Arc::clone(&self.last_items),
            ctx.events.clone(),
        ));
        Ok(())
    }

    fn unload(&mut self) {
        // 索引/watcher 线程随进程生命(同 AppModule catalog 线程);
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
            SettingSpec {
                key: SettingKey(KEY_INDEX_DIRS.into()),
                label: "文件搜索:额外索引目录".into(),
                description: Some(
                    "分号分隔的目录列表,加入文件索引;默认已覆盖用户目录与桌面/文档/下载;改动重启 CUE 后生效"
                        .into(),
                ),
                kind: SettingKind::String,
                default: SettingValue::String(String::new()),
                apply_policy: ApplyPolicy::RestartApplication,
            },
        ]
    }

    fn try_apply_settings(&mut self, changes: SettingsChangeSet) -> Result<(), ModuleError> {
        for (key, value) in &changes.changes {
            match (key.0.as_ref(), value) {
                (KEY_EXCLUDE_NOISE, SettingValue::Bool(v)) => {
                    self.exclude_noise.store(*v, Ordering::Relaxed);
                    // 剪枝口径变了:后台全量重爬让索引跟上(查询级
                    // 过滤即时生效,索引级靠这次重爬)。
                    if let Some(index) = &self.index {
                        index.request_rescan();
                    }
                }
                (KEY_EXCLUDE_NOISE, _) => {
                    return Err(ModuleError::InvalidState(format!("{} 类型不符", key.0)));
                }
                // index_dirs 是 RestartApplication 策略:Core 直接提交
                // 并标记待重启,不经模块 try-apply——这里无事可做。
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

    /// 创建不触碰 IO;首爬完成前 future 在就绪门内挂起(不阻塞
    /// UI 线程),过期完成由 Core 的 ticket 判定丢弃。空查询直接
    /// 返回空(见模块头注释)。名单的 mtime 指纹检查在 future 里做
    /// (后台线程,一次 stat 亚毫秒);名单变更时触发索引后台重爬。
    fn query(&mut self, ctx: QueryContext) -> QueryFuture {
        // 标点触发的剩余输入不去空白;前导空白无意义,trim 掉。
        let search = ctx.query.trim().to_string();
        if search.is_empty() {
            return Box::pin(async { Ok(QueryResponse { items: Vec::new() }) });
        }
        let Some(index) = self.index.clone() else {
            return Box::pin(async {
                Err(ModuleError::Unavailable("file module not loaded".into()))
            });
        };
        let exclude = Arc::clone(&self.exclude);
        let exclude_noise = Arc::clone(&self.exclude_noise);
        let limit = ctx.result_limit;
        let last_items = Arc::clone(&self.last_items);
        Box::pin(async move {
            let snapshot = index.wait_snapshot().await;
            let (fragments, changed) = refreshed_fragments(&exclude);
            if changed {
                // 名单变了:查询级过滤立即用新名单,索引级剪枝
                // 由这次重爬跟上。
                index.request_rescan();
            }
            let noise = exclude_noise.load(Ordering::Relaxed);
            let items: Vec<ModuleItem> =
                index::search_entries(&snapshot, &search, noise, &fragments, limit)
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
        p.icon = self.icon_for(entry);
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
        index::test_entry(path, is_dir, size)
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
        // 通用图标集未初始化 → Segoe 字形兜底(§135)
        assert!(matches!(p.icon, Some(ResultIcon::Raster(_))));

        let file = ModuleItem::new(ItemId(2), entry("C:\\Alpha\\beta.txt", false, Some(2048)));
        let p = m.present(&file);
        assert_eq!(&*p.title, "beta.txt");
        assert_eq!(p.subtitle.as_deref(), Some("C:\\Alpha"));
        assert!(matches!(&p.accessory, Some(ResultAccessory::Text(t)) if &**t == "2.0 KB"));
        assert!(matches!(p.icon, Some(ResultIcon::Raster(_))));

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
    /// 模块内 trim。
    #[test]
    fn query_trims_whitespace() {
        let mut m = FileModule::new();
        // trim 后为空 → 走空查询路径(不需要索引)
        let r = futures::executor::block_on(m.query(QueryContext {
            query: "  ".into(),
            result_limit: 8,
        }))
        .expect("ok");
        assert!(r.items.is_empty());
    }

    // 排除语义(默认生效 / 开关关闭 / `\` 逃生口 / ext: 子集 /
    // 排序)的覆盖在 index::tests(search_entries 纯函数)。
    // 这里保留名单文件链路的测试。

    /// TOML 解析:literal string 反斜杠逐字、注释与空行自由、
    /// 无 excluded 键 = 空名单;语法错误与非字符串元素报 Err。
    /// 归一化:trim、去空、小写。
    #[test]
    fn build_fragments_parses_toml_list() {
        assert!(build_fragments("").unwrap().is_empty());
        assert!(build_fragments("# 只有注释\n").unwrap().is_empty());
        assert_eq!(
            build_fragments(
                "# 系统目录\nexcluded = [\n  'C:\\Windows\\',\n  '\\Node_Modules\\', # 依赖\n]\n"
            )
            .unwrap(),
            vec![r"c:\windows\".to_string(), r"\node_modules\".to_string()]
        );
        // 单行数组 + 基本字符串(双引号,反斜杠需转义)也能解析。
        assert_eq!(
            build_fragments("excluded = [\"C:\\\\Windows\\\\\"]").unwrap(),
            vec![r"c:\windows\".to_string()]
        );
        assert!(build_fragments("excluded = ['unterminated").is_err());
        assert!(build_fragments("excluded = [1]").is_err());
    }

    /// 播种的文件带注释头与默认片段,且能解析归一化(小写)。
    #[test]
    fn seed_file_roundtrips_into_fragments() {
        let dir = std::env::temp_dir().join(format!("cue-file-seed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(EXCLUDE_FILE_NAME);
        seed_exclude_file(&file).unwrap();
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(content.starts_with("# CUE"));
        let frags = build_fragments(&content).expect("seed parses");
        assert!(frags.contains(&r"\node_modules\".to_string()));
        assert!(frags.contains(&r"\appdata\".to_string()));
        assert!(frags.contains(&r"c:\programdata\".to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 存量升级(§125):内容恰为旧默认 → 重写为新默认(幂等);
    /// 用户增删过片段 → 不触碰。
    #[test]
    fn upgrade_rewrites_only_untouched_legacy_seed() {
        let dir = std::env::temp_dir().join(format!("cue-file-upgrade-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("list.toml");

        // 伪造一份旧默认名单(旧 seed 的写盘格式)。
        let mut legacy = String::from("# 旧默认\nexcluded = [\n");
        for f in legacy_default_fragments() {
            legacy.push_str(&format!("  '{f}',\n"));
        }
        legacy.push_str("]\n");
        std::fs::write(&file, &legacy).unwrap();

        assert!(upgrade_seed_if_legacy(&file));
        let upgraded = parse_fragments(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(upgraded, default_fragments());
        assert!(upgraded.iter().any(|f| f == r"\AppData\"));
        // 幂等:新默认 ≠ 旧默认,不再触发。
        assert!(!upgrade_seed_if_legacy(&file));

        // 用户增删过片段 → 不动。
        let custom = legacy.replace("  '\\node_modules\\',", "  '\\custom\\',");
        assert_ne!(custom, legacy);
        std::fs::write(&file, &custom).unwrap();
        assert!(!upgrade_seed_if_legacy(&file));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), custom);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// mtime 指纹:变了才重读;文件消失/读取失败保留旧名单。
    /// 返回的 changed 标记驱动索引重爬(§138)。
    #[test]
    fn refreshed_fragments_follow_mtime() {
        let dir = std::env::temp_dir().join(format!("cue-file-mtime-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("list.toml");
        std::fs::write(&file, "excluded = ['\\alpha\\']\n").unwrap();

        let state = Mutex::new(ExcludeState {
            path: Some(file.clone()),
            mtime: None,
            fragments: vec!["old".into()],
            logger: None,
        });
        // mtime None ≠ Some → 首读,标记 changed
        assert_eq!(
            refreshed_fragments(&state),
            (vec![r"\alpha\".to_string()], true)
        );
        // 同一版本再查:不重读,changed = false
        assert!(!refreshed_fragments(&state).1);
        let first_mtime = state.lock().unwrap().mtime.unwrap();

        // 内容变了但 mtime 没变(写后强制回拨)→ 不重读
        std::fs::write(&file, "excluded = ['\\beta\\']\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(first_mtime)
            .unwrap();
        assert_eq!(refreshed_fragments(&state).0, vec![r"\alpha\".to_string()]);

        // 推进 mtime → 重读
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(first_mtime + std::time::Duration::from_secs(10))
            .unwrap();
        assert_eq!(
            refreshed_fragments(&state),
            (vec![r"\beta\".to_string()], true)
        );

        // 文件消失 → 保留旧名单,不 panic
        std::fs::remove_file(&file).unwrap();
        assert_eq!(refreshed_fragments(&state).0, vec![r"\beta\".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 语法错误(编辑器半保存):保留旧名单,但 mtime 照记——
    /// 同一坏版本不重复重读重报;改对之后正常生效。
    #[test]
    fn malformed_toml_keeps_previous_fragments() {
        let dir = std::env::temp_dir().join(format!("cue-file-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("list.toml");
        std::fs::write(&file, "excluded = ['\\alpha\\']\n").unwrap();

        let state = Mutex::new(ExcludeState {
            path: Some(file.clone()),
            mtime: None,
            fragments: vec!["old".into()],
            logger: None,
        });
        assert_eq!(refreshed_fragments(&state).0, vec![r"\alpha\".to_string()]);
        let bump = |secs: u64| {
            let t = state.lock().unwrap().mtime.unwrap() + std::time::Duration::from_secs(secs);
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(t)
                .unwrap();
        };

        // 写坏 + 推进 mtime → 名单不动,mtime 已记
        std::fs::write(&file, "excluded = ['oops\n").unwrap();
        bump(10);
        assert_eq!(refreshed_fragments(&state).0, vec![r"\alpha\".to_string()]);
        let seen = state.lock().unwrap().mtime.unwrap();

        // 同一坏版本再查:不重读(把文件改回 alpha 但回拨 mtime,名单不变)
        std::fs::write(&file, "excluded = ['\\alpha\\']\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(seen)
            .unwrap();
        assert_eq!(refreshed_fragments(&state).0, vec![r"\alpha\".to_string()]);

        // 改对 + 推进 → 生效
        std::fs::write(&file, "excluded = ['\\beta\\']\n").unwrap();
        bump(10);
        assert_eq!(refreshed_fragments(&state).0, vec![r"\beta\".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// schema 声明 Bool 总开关 + Path 名单文件 + String 额外索引
    /// 目录(RestartApplication);try_apply 只管开关,类型错误返回
    /// Err 而不是 panic。
    #[test]
    fn exclude_settings_roundtrip() {
        let mut m = FileModule::new();
        assert!(m.exclude_noise.load(Ordering::Relaxed));
        assert!(!m.exclude.lock().unwrap().fragments.is_empty());
        let schema = m.settings_schema();
        assert_eq!(schema.len(), 3);
        assert_eq!(schema[0].key.0.as_ref(), KEY_EXCLUDE_NOISE);
        assert_eq!(schema[0].kind, SettingKind::Bool);
        assert_eq!(schema[1].key.0.as_ref(), KEY_EXCLUDE_FILE);
        assert_eq!(schema[1].kind, SettingKind::Path);
        assert!(
            matches!(&schema[1].default, SettingValue::Path(p) if p.ends_with(EXCLUDE_FILE_NAME))
        );
        assert_eq!(schema[2].key.0.as_ref(), KEY_INDEX_DIRS);
        assert!(matches!(
            schema[2].apply_policy,
            ApplyPolicy::RestartApplication
        ));

        let mut off = SettingsChangeSet::default();
        off.changes.push((
            SettingKey(KEY_EXCLUDE_NOISE.into()),
            SettingValue::Bool(false),
        ));
        m.try_apply_settings(off).expect("apply ok");
        assert!(!m.exclude_noise.load(Ordering::Relaxed));

        let mut bad = SettingsChangeSet::default();
        bad.changes.push((
            SettingKey(KEY_EXCLUDE_NOISE.into()),
            SettingValue::String("x".into()),
        ));
        assert!(m.try_apply_settings(bad).is_err());
        assert!(!m.exclude_noise.load(Ordering::Relaxed)); // 失败不留半拉子状态
    }
}
