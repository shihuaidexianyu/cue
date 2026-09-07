//! 自建文件索引(§138):限定范围遍历 + ReadDirectoryChangesW 增量
//! 维护,取代 Everything IPC——无第三方运行依赖、无管理员特权
//! (MFT/USN 路线的运行时特权被免管理员安装堵死,见 §138 记录)。
//!
//! 范围 = %USERPROFILE% ∪ 桌面/文档/下载(SHGetKnownFolderPath,
//! 接住 OneDrive 重定向)∪ `module.file.index_dirs`;根去重去嵌套。
//!
//! 噪声排除名单(§120–125)双段生效:爬取时对命中片段的目录整棵
//! 剪枝(性能:AppData/node_modules 不进索引);查询时对文件级片段
//! 再过滤(正确性)。查询含 `\` 时查询级过滤不生效(逃生口)——
//! 但被剪枝的子树本来就不在索引里,语义弱于 Everything 时代。
//! 名单/开关变更 → 下一次查询立即生效(查询级)+ 触发全量重爬
//! (索引级跟上)。
//!
//! 线程模型:一个索引线程(首爬 → 事件合批 → 增量应用/回退重爬);
//! 每个根一个阻塞 ReadDirectoryChangesW watcher 线程。快照
//! `Arc<Vec>` 整体换入,查询零锁读。首爬完成前 query 在就绪门内
//! 等待(§115:不闪"无结果")。watcher 先开、首爬后做,爬取窗口
//! 的变更积在队列里随后合批补上——不丢变更。

use cue_protocol::{LogLevel, ModuleLogger};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::{Duration, SystemTime};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ACTION, FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED,
    FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::core::PCWSTR;

use crate::{ExcludeState, refreshed_fragments};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
/// 单批事件合批的静默窗口:用户操作(解压、git checkout)是一阵
/// 事件风暴,合批后一次重建快照,而不是每事件一次。
const BATCH_QUIET_MS: u64 = 200;
/// 单批事件上限:风暴再长也先落地一批(内存有界)。
const BATCH_CAP: usize = 4096;
const WATCH_BUF_BYTES: usize = 64 * 1024;

// ---- 业务对象(原 everything.rs 的形状;Core 只见 ItemId)----

#[derive(Clone, Debug)]
pub struct FileEntry {
    /// 全路径(原样,即 usage 的 item_key)。
    pub path: Arc<str>,
    /// 文件名部分(展示标题)。
    pub name: Arc<str>,
    /// 父目录部分(展示副标题);盘符根目录为空。
    pub parent: Arc<str>,
    pub is_dir: bool,
    pub size: Option<u64>,
    /// V1 不展示;保留以支撑后续按修改时间排序。
    #[allow(dead_code)]
    pub modified: Option<SystemTime>,
    id: u64,
}

impl FileEntry {
    pub fn item_id(&self) -> u64 {
        self.id
    }
}

fn make_entry(
    path: String,
    is_dir: bool,
    size: Option<u64>,
    modified: Option<SystemTime>,
) -> FileEntry {
    use std::hash::{Hash, Hasher};
    let id = {
        // DefaultHasher::new() 键固定:同路径同 id,跨快照稳定
        // (PresentationInvalidated 寻址依赖)。
        let mut h = std::collections::hash_map::DefaultHasher::new();
        path.hash(&mut h);
        h.finish()
    };
    let (parent, name) = split_parent_name(&path);
    FileEntry {
        path: path.into(),
        name: name.into(),
        parent: parent.into(),
        is_dir,
        size,
        modified,
        id,
    }
}

/// "C:\\a\\b.txt" → ("C:\\a", "b.txt");盘符根 "C:\\" → ("", "C:\\")。
fn split_parent_name(path: &str) -> (String, String) {
    if path.len() == 3 && path.ends_with(":\\") {
        return (String::new(), path.to_string());
    }
    let trimmed = path.trim_end_matches('\\');
    match trimmed.rsplit_once('\\') {
        Some((parent, name)) if !name.is_empty() => (parent.to_string(), name.to_string()),
        _ => (String::new(), trimmed.to_string()),
    }
}

/// 测试构造入口(lib.rs 的 present 测试等用)。
#[cfg(test)]
pub(crate) fn test_entry(path: &str, is_dir: bool, size: Option<u64>) -> FileEntry {
    make_entry(path.to_string(), is_dir, size, None)
}

// ---- 索引条目与快照 ----

/// 查询热数据:预计算的小写串(匹配零分配)。条目不可变,快照
/// 是 `Arc<Vec<Arc<IndexEntry>>>`——增量重建只付 Arc 克隆。
pub struct IndexEntry {
    entry: FileEntry,
    path_lower: Box<str>,
    name_lower: Box<str>,
}

impl IndexEntry {
    fn new(entry: FileEntry) -> Arc<Self> {
        let path_lower = entry.path.to_lowercase().into();
        let name_lower = entry.name.to_lowercase().into();
        Arc::new(Self {
            entry,
            path_lower,
            name_lower,
        })
    }
}

struct Inner {
    /// None = 首爬未完成(查询在就绪门内等待)。
    snapshot: Option<Arc<Vec<Arc<IndexEntry>>>>,
    wakers: Vec<Waker>,
}

/// 索引对外句柄。clone 廉价(内部全 Arc)。
#[derive(Clone)]
pub struct FileIndex {
    inner: Arc<Mutex<Inner>>,
    /// 手动请求全量重爬(排除开关/名单变更时由模块调用)。
    rescan_tx: Sender<WatchEvent>,
}

enum WatchEvent {
    /// 某文件/目录的变更(全路径 + 动作)。
    Change { path: PathBuf, action: FILE_ACTION },
    /// watcher 缓冲区溢出,或模块手动请求:全量重爬。
    Overflow,
    /// watcher 线程致命错误退出(根消失/句柄失效):内容重爬
    /// 一次;该根的实时更新丢失到下次进程启动(记录在案的限制)。
    WatcherDied(PathBuf),
}

impl FileIndex {
    /// 启动索引线程与 watcher 线程。线程随进程生命(同 AppModule
    /// catalog 线程的先例,模块 unload 不回收)。
    pub fn start(
        roots: Vec<PathBuf>,
        exclude: Arc<Mutex<ExcludeState>>,
        exclude_noise: Arc<AtomicBool>,
        logger: ModuleLogger,
    ) -> Self {
        let inner = Arc::new(Mutex::new(Inner {
            snapshot: None,
            wakers: Vec::new(),
        }));
        let (tx, rx) = channel::<WatchEvent>();
        let handle = Self {
            inner: Arc::clone(&inner),
            rescan_tx: tx.clone(),
        };
        std::thread::spawn(move || {
            index_thread_main(inner, roots, exclude, exclude_noise, tx, rx, logger)
        });
        handle
    }

    /// 等待首个快照就绪(首爬完成)。克隆代价 = 一个 Arc。
    pub async fn wait_snapshot(&self) -> Arc<Vec<Arc<IndexEntry>>> {
        let inner = Arc::clone(&self.inner);
        futures::future::poll_fn(move |cx| {
            let mut st = inner.lock().unwrap();
            match &st.snapshot {
                Some(snap) => Poll::Ready(Arc::clone(snap)),
                None => {
                    st.wakers.push(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await
    }

    /// 请求一次全量重爬(排除口径变更后调用;结果是异步的,
    /// 调用方不等待)。
    pub fn request_rescan(&self) {
        let _ = self.rescan_tx.send(WatchEvent::Overflow);
    }
}

fn publish(inner: &Mutex<Inner>, entries: Vec<Arc<IndexEntry>>) {
    let wakers = {
        let mut st = inner.lock().unwrap();
        st.snapshot = Some(Arc::new(entries));
        std::mem::take(&mut st.wakers)
    };
    for w in wakers {
        w.wake();
    }
}

// ---- 根目录解析 ----

/// 默认索引根:%USERPROFILE% + 桌面/文档/下载(Known Folder 解析,
/// OneDrive 重定向后的真实位置),去重去嵌套。
pub fn default_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(p) = std::env::var_os("USERPROFILE") {
        roots.push(PathBuf::from(p));
    }
    for id in [&FOLDERID_Desktop, &FOLDERID_Documents, &FOLDERID_Downloads] {
        if let Some(p) = known_folder(id) {
            roots.push(p);
        }
    }
    dedup_roots(roots)
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.as_ptr() as *const core::ffi::c_void));
        s.map(PathBuf::from)
    }
}

/// 根去重去嵌套:大小写不敏感、忽略尾部反斜杠;被另一根包含的
/// 根丢弃(重复爬 = 重复条目)。
pub(crate) fn dedup_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut norm: Vec<(String, PathBuf)> = roots
        .into_iter()
        .map(|p| {
            (
                p.to_string_lossy()
                    .trim_end_matches('\\')
                    .to_lowercase(),
                p,
            )
        })
        .collect();
    norm.sort_by(|a, b| a.0.cmp(&b.0));
    norm.dedup_by(|a, b| a.0 == b.0);
    let mut kept: Vec<(String, PathBuf)> = Vec::new();
    'outer: for (key, path) in norm {
        for (parent, _) in &kept {
            if key.len() > parent.len()
                && key.starts_with(parent.as_str())
                && key.as_bytes()[parent.len()] == b'\\'
            {
                continue 'outer;
            }
        }
        kept.push((key, path));
    }
    kept.into_iter().map(|(_, p)| p).collect()
}

// ---- 爬取 ----

#[derive(Default)]
struct CrawlStats {
    pruned: u32,
    errors: u32,
}

fn entry_for(path: &Path, meta: &std::fs::Metadata) -> Arc<IndexEntry> {
    let is_dir = meta.is_dir();
    IndexEntry::new(make_entry(
        // 非 UTF-8 路径会被 U+FFFD 错位(与 Everything 时代的
        // lossy 转换同一水位;此类文件名激活会失败,记录在案)。
        path.to_string_lossy().into_owned(),
        is_dir,
        if is_dir { None } else { Some(meta.len()) },
        meta.modified().ok(),
    ))
}

/// 目录剪枝判定:片段按"目录全路径(补尾 \)小写子串"匹配。
fn dir_pruned(dir: &Path, fragments_lower: &[String]) -> bool {
    let mut p = dir.to_string_lossy().to_lowercase();
    p.push('\\');
    fragments_lower.iter().any(|f| p.contains(f.as_str()))
}

/// 迭代遍历 dir 的子树(不含 dir 本身):reparse point 目录跳过
/// (junction/symlink 防环;OneDrive 占位文件是普通文件,照常索引
/// ——只读属性不触发内容回调);命中排除片段的目录整棵剪枝。
fn walk_into(
    dir: &Path,
    fragments_lower: &[String],
    exclude_noise: bool,
    out: &mut Vec<Arc<IndexEntry>>,
    stats: &mut CrawlStats,
) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = match std::fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(_) => {
                stats.errors += 1;
                continue;
            }
        };
        for item in rd {
            let (path, meta) = match item.and_then(|de| de.metadata().map(|m| (de.path(), m))) {
                Ok(v) => v,
                Err(_) => {
                    stats.errors += 1;
                    continue;
                }
            };
            if meta.is_dir() {
                if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    stats.pruned += 1;
                    continue;
                }
                if exclude_noise && dir_pruned(&path, fragments_lower) {
                    stats.pruned += 1;
                    continue;
                }
                out.push(entry_for(&path, &meta));
                stack.push(path);
            } else {
                out.push(entry_for(&path, &meta));
            }
        }
    }
}

fn crawl(
    roots: &[PathBuf],
    fragments_lower: &[String],
    exclude_noise: bool,
    logger: &ModuleLogger,
) -> Vec<Arc<IndexEntry>> {
    let started = std::time::Instant::now();
    let mut out = Vec::new();
    let mut stats = CrawlStats::default();
    for root in roots {
        match std::fs::metadata(root) {
            Ok(meta) => {
                out.push(entry_for(root, &meta));
                walk_into(root, fragments_lower, exclude_noise, &mut out, &mut stats);
            }
            Err(_) => stats.errors += 1,
        }
    }
    logger.log(
        LogLevel::Info,
        &format!(
            "file index: {} entries ({} roots, {} pruned, {} errors) in {:?}",
            out.len(),
            roots.len(),
            stats.pruned,
            stats.errors,
            started.elapsed()
        ),
    );
    out
}

// ---- watcher ----

fn spawn_watchers(roots: &[PathBuf], tx: &Sender<WatchEvent>, logger: &ModuleLogger) {
    for root in roots {
        let tx = tx.clone();
        let root = root.clone();
        let logger = logger.clone();
        std::thread::spawn(move || watch_root(root, tx, logger));
    }
}

/// 单根阻塞式 watcher:句柄打不开/读失败 → WatcherDied 后退出。
/// 缓冲区溢出(returned == 0)→ Overflow(内容全量重爬,watch
/// 本身继续——溢出不会杀死句柄)。
fn watch_root(root: PathBuf, tx: Sender<WatchEvent>, logger: ModuleLogger) {
    let wide: Vec<u16> = root
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            PCWSTR::from_raw(wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    let handle: HANDLE = match handle {
        Ok(h) => h,
        Err(e) => {
            logger.log(
                LogLevel::Warn,
                &format!("file watcher: 打开失败 {}: {e}", root.display()),
            );
            let _ = tx.send(WatchEvent::WatcherDied(root));
            return;
        }
    };
    // u64 缓冲保证 FILE_NOTIFY_INFORMATION 的 4 字节对齐。
    let mut buf = vec![0u64; WATCH_BUF_BYTES / 8];
    loop {
        let mut returned = 0u32;
        let ok = unsafe {
            ReadDirectoryChangesW(
                handle,
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                (buf.len() * 8) as u32,
                true, // bWatchSubtree:一句柄覆盖整棵子树
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_LAST_WRITE,
                Some(&mut returned),
                None,
                None,
            )
        };
        match ok {
            Err(e) => {
                logger.log(
                    LogLevel::Warn,
                    &format!("file watcher: 读取失败 {}: {e}", root.display()),
                );
                let _ = tx.send(WatchEvent::WatcherDied(root));
                break;
            }
            Ok(()) if returned == 0 => {
                if tx.send(WatchEvent::Overflow).is_err() {
                    break;
                }
            }
            Ok(()) => {
                let base = buf.as_ptr() as *const u8;
                let name_off = core::mem::offset_of!(FILE_NOTIFY_INFORMATION, FileName);
                let mut off = 0usize;
                let mut alive = true;
                loop {
                    if off + name_off > returned as usize {
                        break;
                    }
                    let info = unsafe { &*(base.add(off) as *const FILE_NOTIFY_INFORMATION) };
                    let name_len = info.FileNameLength as usize / 2;
                    if off + name_off + name_len * 2 > returned as usize {
                        break; // 截断记录:不越界读,等下一批
                    }
                    let name =
                        unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), name_len) };
                    let event = WatchEvent::Change {
                        path: root.join(OsString::from_wide(name)),
                        action: info.Action,
                    };
                    if tx.send(event).is_err() {
                        alive = false;
                        break;
                    }
                    if info.NextEntryOffset == 0 {
                        break;
                    }
                    off += info.NextEntryOffset as usize;
                }
                if !alive {
                    break;
                }
            }
        }
    }
    unsafe {
        let _ = CloseHandle(handle);
    }
}

// ---- 索引线程:首爬 → 事件合批 → 增量应用 / 回退重爬 ----

fn index_thread_main(
    inner: Arc<Mutex<Inner>>,
    roots: Vec<PathBuf>,
    exclude: Arc<Mutex<ExcludeState>>,
    exclude_noise: Arc<AtomicBool>,
    tx: Sender<WatchEvent>,
    rx: Receiver<WatchEvent>,
    logger: ModuleLogger,
) {
    // watcher 先开:首爬期间的变更积进队列,爬完后合批补上。
    spawn_watchers(&roots, &tx, &logger);
    full_rescan(&inner, &roots, &exclude, &exclude_noise, &logger);
    let mut batch: Vec<WatchEvent> = Vec::new();
    while let Ok(first) = rx.recv() {
        batch.push(first);
        // 合批:静默窗结束或攒够上限,取先者。
        while batch.len() < BATCH_CAP {
            match rx.recv_timeout(Duration::from_millis(BATCH_QUIET_MS)) {
                Ok(ev) => batch.push(ev),
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
        let mut rescan = false;
        for ev in &batch {
            match ev {
                // WatcherDied 不重生该根的 watcher(记录在案的限制:
                // 根级致命错误极少见,留给下次进程启动;内容本身已由
                // 全量重爬修正)。
                WatchEvent::WatcherDied(root) => {
                    logger.log(
                        LogLevel::Warn,
                        &format!("file watcher 已退出:{};该根实时更新丢失到下次启动", root.display()),
                    );
                    rescan = true;
                }
                WatchEvent::Overflow => rescan = true,
                WatchEvent::Change { .. } => {}
            }
        }
        if rescan {
            full_rescan(&inner, &roots, &exclude, &exclude_noise, &logger);
        } else {
            apply_batch(&inner, &batch, &exclude, &exclude_noise, &logger);
        }
        batch.clear();
    }
}

fn full_rescan(
    inner: &Arc<Mutex<Inner>>,
    roots: &[PathBuf],
    exclude: &Arc<Mutex<ExcludeState>>,
    exclude_noise: &AtomicBool,
    logger: &ModuleLogger,
) {
    let fragments = refreshed_fragments(exclude).0;
    let noise = exclude_noise.load(Ordering::Relaxed);
    publish(inner, crawl(roots, &fragments, noise, logger));
}

/// 一批变更对旧快照的增量应用。rename 拆成"删旧 + 增新"(不配对:
/// 配对只是省一次 remove+insert,乱序/跨批到达还要配对状态机,
/// 不值得)。整棵子树的删除/搬入:RDCW 只报目录本身,删除按
/// "前缀移除",新增目录则就地补爬它的子树。
fn apply_batch(
    inner: &Arc<Mutex<Inner>>,
    batch: &[WatchEvent],
    exclude: &Arc<Mutex<ExcludeState>>,
    exclude_noise: &AtomicBool,
    logger: &ModuleLogger,
) {
    let old = {
        let st = inner.lock().unwrap();
        match &st.snapshot {
            Some(s) => Arc::clone(s),
            None => return, // 首爬前的积压在首爬后才有意义;为 None 直接丢
        }
    };
    let fragments = refreshed_fragments(exclude).0;
    let noise = exclude_noise.load(Ordering::Relaxed);

    // 目录成员集:删除子树需要知道被删路径是不是目录(文件已消失,
    // 只能靠旧索引判断)。
    let known_dirs: HashSet<&str> = old
        .iter()
        .filter(|e| e.entry.is_dir)
        .map(|e| e.path_lower.as_ref())
        .collect();
    let mut removed: HashSet<Box<str>> = HashSet::new();
    let mut removed_prefixes: Vec<Box<str>> = Vec::new();
    let mut upserts: HashMap<Box<str>, Arc<IndexEntry>> = HashMap::new();

    let remove_path = |path: &Path,
                           known_dirs: &HashSet<&str>,
                           removed: &mut HashSet<Box<str>>,
                           removed_prefixes: &mut Vec<Box<str>>,
                           upserts: &mut HashMap<Box<str>, Arc<IndexEntry>>| {
        let lower = path.to_string_lossy().to_lowercase();
        if known_dirs.contains(lower.as_str()) {
            let mut prefix = lower.clone();
            prefix.push('\\');
            removed_prefixes.push(prefix.into());
        }
        upserts.remove(lower.as_str());
        removed.insert(lower.into());
    };

    let mut stats = CrawlStats::default();
    for ev in batch {
        let WatchEvent::Change { path, action } = ev else {
            continue;
        };
        if *action == FILE_ACTION_REMOVED || *action == FILE_ACTION_RENAMED_OLD_NAME {
            remove_path(path, &known_dirs, &mut removed, &mut removed_prefixes, &mut upserts);
        } else if *action == FILE_ACTION_ADDED
            || *action == FILE_ACTION_MODIFIED
            || *action == FILE_ACTION_RENAMED_NEW_NAME
        {
            match std::fs::metadata(path) {
                    // 事件到 stat 之间文件已消失:按删除处理。
                    Err(_) => remove_path(
                        path,
                        &known_dirs,
                        &mut removed,
                        &mut removed_prefixes,
                        &mut upserts,
                    ),
                    Ok(meta) => {
                        let lower: Box<str> =
                            path.to_string_lossy().to_lowercase().into();
                        if meta.is_dir() {
                            let reparse =
                                meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
                            if reparse || (noise && dir_pruned(path, &fragments)) {
                                // 改名进了被剪枝/接合的名字:连子树移除。
                                remove_path(
                                    path,
                                    &known_dirs,
                                    &mut removed,
                                    &mut removed_prefixes,
                                    &mut upserts,
                                );
                            } else {
                                upserts.insert(lower, entry_for(path, &meta));
                                // 搬入/新建的目录可能自带内容:RDCW 不报
                                // 子项,就地补爬子树。
                                let mut children = Vec::new();
                                walk_into(path, &fragments, noise, &mut children, &mut stats);
                                for c in children {
                                    upserts.insert(c.path_lower.clone(), c);
                                }
                            }
                        } else {
                            upserts.insert(lower, entry_for(path, &meta));
                        }
                    }
                }
            }
    }

    let mut next: Vec<Arc<IndexEntry>> = Vec::with_capacity(old.len() + upserts.len());
    for e in old.iter() {
        let p = e.path_lower.as_ref();
        let gone = removed.contains(p)
            || upserts.contains_key(p)
            || removed_prefixes.iter().any(|pre| p.starts_with(pre.as_ref()));
        if !gone {
            next.push(Arc::clone(e));
        }
    }
    next.extend(upserts.into_values());
    if stats.errors > 0 {
        logger.log(
            LogLevel::Warn,
            &format!("file index: 增量补爬 {} 个路径失败", stats.errors),
        );
    }
    publish(inner, next);
}

// ---- 查询 ----

enum Token {
    /// 小写子串,对全路径匹配。
    Text(String),
    /// ext:pdf → 文件名后缀 ".pdf"(Everything 函数语法的保留子集,
    /// 其余函数正式放弃,见 §138)。
    ExtSuffix(String),
}

fn parse_tokens(query: &str) -> Vec<Token> {
    query
        .split_whitespace()
        .map(|t| {
            let lower = t.to_lowercase();
            match lower.strip_prefix("ext:") {
                Some(ext) if !ext.is_empty() => Token::ExtSuffix(format!(".{ext}")),
                _ => Token::Text(lower),
            }
        })
        .collect()
}

/// 纯函数查询:token 空白 AND;text token 子串匹配小写全路径;
/// `ext:` 匹配文件名后缀。排序 = 文件名命中(全部 text token 落在
/// 文件名内)> 路径命中,平手按名字升序。排除名单在查询级过滤
/// (爬取剪枝之外的文件级片段);查询含 `\` 时不过滤(逃生口)。
pub fn search_entries(
    entries: &[Arc<IndexEntry>],
    query: &str,
    exclude_noise: bool,
    fragments_lower: &[String],
    limit: usize,
) -> Vec<FileEntry> {
    let tokens = parse_tokens(query);
    if tokens.is_empty() || limit == 0 {
        return Vec::new();
    }
    let filter = exclude_noise && !query.contains('\\') && !fragments_lower.is_empty();
    let mut scored: Vec<(bool, &Arc<IndexEntry>)> = entries
        .iter()
        .filter(|e| {
            !(filter && fragments_lower.iter().any(|f| e.path_lower.contains(f.as_str())))
        })
        .filter_map(|e| {
            let hit = tokens.iter().all(|t| match t {
                Token::Text(s) => e.path_lower.contains(s.as_str()),
                Token::ExtSuffix(suffix) => e.name_lower.ends_with(suffix.as_str()),
            });
            if !hit {
                return None;
            }
            let name_hit = tokens.iter().all(|t| match t {
                Token::Text(s) => e.name_lower.contains(s.as_str()),
                Token::ExtSuffix(_) => true,
            });
            Some((name_hit, e))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.name_lower.cmp(&b.1.name_lower))
            .then_with(|| a.1.path_lower.cmp(&b.1.path_lower))
    });
    scored.truncate(limit);
    scored.into_iter().map(|(_, e)| e.entry.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool) -> Arc<IndexEntry> {
        IndexEntry::new(make_entry(path.to_string(), is_dir, None, None))
    }

    fn paths(entries: &[FileEntry]) -> Vec<String> {
        entries.iter().map(|e| e.path.to_string()).collect()
    }

    /// 根去重:大小写/尾斜杠归一;嵌套根丢弃。
    #[test]
    fn roots_dedup_and_unnest() {
        let roots = vec![
            PathBuf::from(r"C:\Users\x\"),
            PathBuf::from(r"c:\users\x"),
            PathBuf::from(r"C:\Users\x\Desktop"),
            PathBuf::from(r"D:\dev"),
        ];
        let got = dedup_roots(roots);
        assert_eq!(got.len(), 2);
        assert!(got.contains(&PathBuf::from(r"C:\Users\x\")));
        assert!(got.contains(&PathBuf::from(r"D:\dev")));
    }

    /// 查询语义:AND、大小写不敏感、ext: 子集、文件名命中优先。
    #[test]
    fn search_semantics() {
        let entries = vec![
            entry(r"C:\Users\x\Documents\report.pdf", false),
            entry(r"C:\Users\x\Downloads\Report-Final.pdf", false),
            entry(r"C:\Users\x\Documents\reports", true),
        ];
        // AND + 大小写
        let got = search_entries(&entries, "report", false, &[], 8);
        assert_eq!(got.len(), 3);
        // 三者都 name_hit,平手按小写名字升序:"report-final.pdf"
        // ('-' 0x2d < '.' 0x2e)最先。
        assert!(paths(&got)[0].contains("Report-Final"));
        // ext: 子集
        let got = search_entries(&entries, "ext:pdf", false, &[], 8);
        assert_eq!(got.len(), 2);
        let got = search_entries(&entries, "report ext:pdf", false, &[], 8);
        assert_eq!(got.len(), 2);
        // 路径片段命中(非文件名)
        let got = search_entries(&entries, "downloads", false, &[], 8);
        assert_eq!(got.len(), 1);
        assert!(paths(&got)[0].contains("Downloads"));
        // limit
        let got = search_entries(&entries, "report", false, &[], 1);
        assert_eq!(got.len(), 1);
    }

    /// 排除名单查询级过滤:默认生效;查询含 `\` 整体不生效(逃生口)。
    #[test]
    fn search_exclusion_and_escape_hatch() {
        let entries = vec![
            entry(r"C:\Users\x\node_modules\pkg\index.js", false),
            entry(r"C:\Users\x\dev\pkg\index.js", false),
        ];
        let frags = vec![r"\node_modules\".to_string()];
        let got = search_entries(&entries, "index.js", true, &frags, 8);
        assert_eq!(got.len(), 1);
        assert!(paths(&got)[0].contains("dev"));
        // 关掉开关 → 不过滤
        let got = search_entries(&entries, "index.js", false, &frags, 8);
        assert_eq!(got.len(), 2);
        // 逃生口:显式路径不过滤
        let got = search_entries(&entries, r"x\node_modules", true, &frags, 8);
        assert_eq!(got.len(), 1);
    }

    /// 目录剪枝判定:片段按带尾 \ 的小写全路径子串匹配。
    #[test]
    fn dir_pruning() {
        let frags = vec![r"\appdata\".to_string()];
        assert!(dir_pruned(Path::new(r"C:\Users\x\AppData"), &frags));
        assert!(dir_pruned(Path::new(r"C:\Users\x\appdata\Local"), &frags));
        assert!(!dir_pruned(Path::new(r"C:\Users\x\Documents"), &frags));
    }

    /// 临时目录树爬取:文件与目录都进索引;命中片段的子树不进。
    #[test]
    fn crawl_temp_tree() {
        let root = std::env::temp_dir().join(format!("cue-file-crawl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub\\deep")).unwrap();
        std::fs::create_dir_all(root.join("node_modules\\pkg")).unwrap();
        std::fs::write(root.join("a.txt"), b"x").unwrap();
        std::fs::write(root.join("sub\\b.txt"), b"xx").unwrap();
        std::fs::write(root.join("node_modules\\pkg\\c.js"), b"x").unwrap();

        let frags = vec![r"\node_modules\".to_string()];
        let logger: ModuleLogger = Arc::new(TestLog);
        let entries = crawl(std::slice::from_ref(&root), &frags, true, &logger);
        let names: Vec<String> = entries.iter().map(|e| e.entry.path.to_string()).collect();
        let root_s = root.to_string_lossy().to_string();
        assert!(names.contains(&root_s));
        assert!(names.contains(&format!("{root_s}\\a.txt")));
        assert!(names.contains(&format!("{root_s}\\sub\\deep")));
        assert!(!names.iter().any(|p| p.contains("node_modules")));
        // 尺寸:文件有、目录无
        let a = entries.iter().find(|e| e.entry.name.as_ref() == "a.txt").unwrap();
        assert_eq!(a.entry.size, Some(1));
        let sub = entries.iter().find(|e| e.entry.name.as_ref() == "sub").unwrap();
        assert_eq!(sub.entry.size, None);
        // 开关关闭 → 不剪枝
        let entries = crawl(std::slice::from_ref(&root), &frags, false, &logger);
        assert!(entries.iter().any(|e| e.entry.path.contains("node_modules")));

        std::fs::remove_dir_all(&root).ok();
    }

    struct TestLog;
    impl cue_protocol::ModuleLog for TestLog {
        fn log(&self, _level: LogLevel, _message: &str) {}
    }
}
