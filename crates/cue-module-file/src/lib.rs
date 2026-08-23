//! cue-module-file —— FileModule(§31–33、§118)。
//!
//! `/` 触发的文件搜索:触发词是标点,verbatim 匹配(§5.2 词边界只约束
//! 字母触发);`/` 之后的输入原样作为 Everything 搜索串——查询语法
//! (子串、`ext:`、路径过滤等)归模块与 Everything,Core 不解析(§3)。
//! 数据源是本机已安装运行的 Everything 1.4(§31 依赖决策:依赖第三方
//! 服务,不自建索引、不链 Everything.dll),IPC 走 WM_COPYDATA(§99:
//! 专用线程 + latest-wins 槽)。文件与文件夹同一模态(§33);FileEntry
//! 只在模块内部,Core 只见 ItemId(§32)。
//!
//! 空查询返回空:UsageRead 只能按键查、不能枚举,给不出 Top Files
//! (§50);不显示任何推荐内容(§115 精神)。排序保持 Everything 的
//! NAME_ASCENDING(SDK 保证该序无性能损失),V1 不做 usage 重排。

mod com;
mod everything;
mod icon;

use cue_protocol::*;
use everything::{EverythingBackend, FileEntry};
use std::sync::{Arc, Mutex, OnceLock};

/// §31:FileModule,trigger `/`。
pub struct FileModule {
    descriptor: ModuleDescriptor,
    /// load 时启动(专用 IPC 线程);未 load 时 None → query 报 Unavailable。
    backend: Option<EverythingBackend>,
    /// 文件夹 / 通用文件图标(load 后台线程一次性提取;§14 要求同一
    /// Arc 复用,UI 按 rgba 指针缓存纹理)。
    icons: Arc<OnceLock<icon::FileIcons>>,
    /// 最近一次 query 返回的 item id:图标晚到时据此发
    /// PresentationInvalidated(§109),让 Core 重画可见行。
    last_items: Arc<Mutex<Vec<ItemId>>>,
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
    /// 线程内,§99)与图标提取线程(两次 SHGetFileInfoW,毫秒级)。
    fn load(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.backend = Some(EverythingBackend::start(ctx.logger.clone()));

        let icons = Arc::clone(&self.icons);
        let last_items = Arc::clone(&self.last_items);
        let sink = ctx.events.clone();
        let logger = ctx.logger.clone();
        std::thread::spawn(move || {
            let _com = com::ComGuard::new();
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
        // IPC 线程随进程生命(§99,同 AppModule catalog 线程);
        // icons OnceLock 不重建:幂等无害。
    }

    fn settings_schema(&self) -> SettingsSchema {
        Vec::new()
    }

    fn try_apply_settings(&mut self, _changes: SettingsChangeSet) -> Result<(), ModuleError> {
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

    /// §93:创建不触碰 IO——只往 latest-wins 槽投一个请求,future 内
    /// await 应答。空查询直接返回空(见模块头注释)。
    fn query(&mut self, ctx: QueryContext) -> QueryFuture {
        // 标点触发的剩余输入不去空白(§5.2);Everything 语义上前导
        // 空白无意义,trim 掉。
        let search = ctx.query.trim().to_string();
        if search.is_empty() {
            return Box::pin(async { Ok(QueryResponse { items: Vec::new() }) });
        }
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
                // oneshot Canceled = 被更新的输入顶掉(§99 latest-wins);
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
                // UI 按该指针缓存纹理(§14)。
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

    /// V1 只有 Open;打开所在文件夹等次级 action 等 P1 action menu。
    fn actions(&self, _item: &ModuleItem) -> Vec<ActionDescriptor> {
        vec![ActionDescriptor {
            id: ActionId::PRIMARY,
            label: "Open".into(),
            shortcut: None,
        }]
    }

    /// Open = ShellExecute 默认动词:文件由系统关联程序打开,文件夹
    /// 进资源管理器。usage 身份 = 全路径(§51,稳定启动标识)。
    fn activate(&mut self, item: &ModuleItem, action: ActionId) -> ActivationFuture {
        let entry = item.downcast_ref::<FileEntry>().cloned();
        Box::pin(async move {
            let Some(entry) = entry else {
                return ModuleOutcome::failed(ModuleError::InvalidState(
                    "item payload is not a FileEntry".into(),
                ));
            };
            match shell_execute(&entry.path) {
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

/// ShellExecuteExW(Rule of Three 第三次:app launch_win32 / bookmark
/// shell_execute 同型;util 下沉单独提交)。lpVerb = None(默认动词)。
fn shell_execute(file: &str) -> Result<(), ModuleError> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let file_w: Vec<u16> = file.encode_utf16().chain(Some(0)).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        lpFile: PCWSTR(file_w.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info)
            .map_err(|e| ModuleError::ActivationFailed(format!("{file}: {e}")))
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

    /// 空查询(含纯空白)返回空,不触碰 backend(§50 给不出 Top Files)。
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

    /// `/  cue` 这种带前导空白的剩余输入(§5.2 标点触发不去空白),
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
}
