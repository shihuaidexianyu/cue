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
//! 每个根一个可取消的 overlapped ReadDirectoryChangesW watcher。快照
//! `Arc<Vec>` 整体换入,查询零锁读。首爬完成前 query 在就绪门内
//! 等待(§115:不闪"无结果")。watcher 首次读取投递后才首爬,爬取窗口
//! 的变更积在队列里随后合批补上——不丢变更。

use sakana_protocol::{LogLevel, ModuleLogger};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, SyncSender, TrySendError, channel, sync_channel,
};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::{Duration, SystemTime};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ACTION, FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED,
    FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    ReadDirectoryChangesW,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};
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
/// 首爬期间排水的事件上限(§142)。超过即放弃逐条补齐,退回全量
/// 重扫——正确性优先于省一次爬取,内存有界。
const INITIAL_DRAIN_CAP: usize = 64 * 1024;
/// 单根 watcher 就绪握手的等待上限(§142):坏根(无响应 UNC /
/// 休眠 NAS)不得让首爬和退出无限期卡住。
const WATCH_READY_TIMEOUT: Duration = Duration::from_secs(5);
/// 退出时等待 watcher 线程收尾的上限(§142)。超时即 detach:
/// 进程正在退出,泄漏一个卡在取消 IO 上的线程好过 UI 永久假死。
const WATCH_JOIN_TIMEOUT: Duration = Duration::from_secs(2);
/// watcher 死亡后的重生退避表(§146):同根连续死亡按表递增等待,
/// 表尽(连续第 4 次)则永久放弃、留待下次进程启动;任何一次成功
/// 的读取周期把连续计数清零——瞬时抖动(睡眠唤醒/网络盘闪断)总能
/// 恢复,永久坏根不无限重扫。死亡上报照发,索引线程的全量重爬
/// 补齐缺口(§138 的语义不变)。
const WATCHER_RESPAWN_DELAYS: [Duration; 3] = [
    Duration::from_secs(10),
    Duration::from_secs(60),
    Duration::from_secs(300),
];
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
    rescan_tx: EventSender,
    service: Arc<Service>,
}

struct Service {
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Service {
    fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 队列满时只保留一个重扫标记,watcher 永不阻塞在发送上。
#[derive(Clone)]
struct EventSender {
    tx: SyncSender<WatchEvent>,
    overflow: Arc<AtomicBool>,
}

impl EventSender {
    fn send(&self, event: WatchEvent) -> Result<(), ()> {
        match self.tx.try_send(event) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.overflow.store(true, Ordering::Release);
                Ok(())
            }
            Err(TrySendError::Disconnected(_)) => Err(()),
        }
    }
}

enum WatchEvent {
    /// 某文件/目录的变更(全路径 + 动作)。
    Change { path: PathBuf, action: FILE_ACTION },
    /// watcher 缓冲区溢出,或模块手动请求:全量重爬。
    Overflow,
    /// watcher 线程致命错误退出(根消失/句柄失效):内容重爬补齐
    /// 缺口;watcher 侧按 §146 有界退避重开,表尽才永久放弃。
    WatcherDied(PathBuf),
}

impl FileIndex {
    /// 启动索引与 watcher;unload 主动停止,最后一个句柄释放也会回收。
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
        let (sender, rx) = sync_channel::<WatchEvent>(BATCH_CAP);
        let tx = EventSender {
            tx: sender,
            overflow: Arc::new(AtomicBool::new(false)),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_inner = Arc::clone(&inner);
        let worker_tx = tx.clone();
        let worker = std::thread::spawn(move || {
            index_thread_main(
                worker_inner,
                roots,
                exclude,
                exclude_noise,
                worker_tx,
                rx,
                logger,
                worker_stop,
            )
        });
        Self {
            inner: Arc::clone(&inner),
            rescan_tx: tx,
            service: Arc::new(Service {
                stop,
                worker: Mutex::new(Some(worker)),
            }),
        }
    }

    pub fn shutdown(&self) {
        self.service.shutdown();
    }

    /// 等待首个快照就绪(首爬完成)。克隆代价 = 一个 Arc。
    pub async fn wait_snapshot(&self) -> Arc<Vec<Arc<IndexEntry>>> {
        let inner = Arc::clone(&self.inner);
        futures::future::poll_fn(move |cx| {
            let mut st = inner.lock().unwrap();
            match &st.snapshot {
                Some(snap) => Poll::Ready(Arc::clone(snap)),
                None => {
                    if !st.wakers.iter().any(|w| w.will_wake(cx.waker())) {
                        st.wakers.push(cx.waker().clone());
                    }
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
        .map(|p| (p.to_string_lossy().trim_end_matches('\\').to_lowercase(), p))
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
    stop: &AtomicBool,
    drain: &mut dyn FnMut(),
) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if stop.load(Ordering::Acquire) {
            return;
        }
        drain();
        let rd = match std::fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(_) => {
                stats.errors += 1;
                continue;
            }
        };
        for item in rd {
            if stop.load(Ordering::Acquire) {
                return;
            }
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

/// 全量爬取。`drain` 在每个目录访问时回调一次:首爬与 watcher 并发,
/// 调用方借此排空事件队列(§142)。
fn crawl(
    roots: &[PathBuf],
    fragments_lower: &[String],
    exclude_noise: bool,
    logger: &ModuleLogger,
    stop: &AtomicBool,
    drain: &mut dyn FnMut(),
) -> Vec<Arc<IndexEntry>> {
    let started = std::time::Instant::now();
    let mut out = Vec::new();
    let mut stats = CrawlStats::default();
    for root in roots {
        if stop.load(Ordering::Acquire) {
            break;
        }
        match std::fs::metadata(root) {
            Ok(meta) => {
                out.push(entry_for(root, &meta));
                walk_into(
                    root,
                    fragments_lower,
                    exclude_noise,
                    &mut out,
                    &mut stats,
                    stop,
                    drain,
                );
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

fn spawn_watchers(
    roots: &[PathBuf],
    tx: &EventSender,
    logger: &ModuleLogger,
    stop: &Arc<AtomicBool>,
) -> (Vec<std::thread::JoinHandle<()>>, Receiver<()>) {
    let mut workers = Vec::new();
    let (done_tx, done_rx) = channel();
    for root in roots {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let tx = tx.clone();
        let root = root.clone();
        let logger = logger.clone();
        let stop = Arc::clone(stop);
        let done = done_tx.clone();
        // 超时日志用副本:root / logger 都要移进 watcher 线程。
        let log_root = root.clone();
        let log = logger.clone();
        let (ready_tx, ready_rx) = sync_channel(1);
        workers.push(std::thread::spawn(move || {
            watch_root(root, tx, logger, stop, ready_tx);
            let _ = done.send(());
        }));
        // 首个异步读取已投递或打开失败后才首爬,关闭启动竞态窗口。
        // §142:坏根(无响应 UNC / 休眠 NAS)可能让 CreateFileW 长时间
        // 不返回——等待必须有上限,否则首爬与退出都会无限期卡住。
        // 打开失败是断开连接(线程退出即 drop ready_tx),不占超时。
        if ready_rx.recv_timeout(WATCH_READY_TIMEOUT).is_err() {
            log.log(
                LogLevel::Warn,
                &format!("file watcher: 就绪超时,跳过等待 {}", log_root.display()),
            );
        }
    }
    drop(done_tx);
    (workers, done_rx)
}

/// §146:第 `deaths` 次连续死亡后的重生等待;None = 放弃重生。
fn respawn_delay(deaths: usize) -> Option<Duration> {
    WATCHER_RESPAWN_DELAYS.get(deaths).copied()
}

/// 可中断睡眠:退避期间 stop 置位要在亚秒内醒来(退出 join 只等
/// 2 s,睡死的线程由 detach 兜底,但能醒则醒)。
fn sleep_stoppable(dur: Duration, stop: &AtomicBool) {
    let deadline = std::time::Instant::now() + dur;
    while !stop.load(Ordering::Acquire) {
        let now = std::time::Instant::now();
        if now >= deadline {
            return;
        }
        std::thread::sleep((deadline - now).min(Duration::from_millis(200)));
    }
}

/// 单根异步 watcher:句柄打不开/读失败 → 上报 WatcherDied,按
/// §146 的有界退避在同线程内重开(连续死亡表尽才永久放弃);索引
/// 线程收到 WatcherDied 仍走全量重爬补齐缺口。缓冲区溢出
/// (returned == 0)→ Overflow(内容全量重爬,watch 本身继续——
/// 溢出不会杀死句柄)。
fn watch_root(
    root: PathBuf,
    tx: EventSender,
    logger: ModuleLogger,
    stop: Arc<AtomicBool>,
    ready: SyncSender<()>,
) {
    let wide: Vec<u16> = root
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // §146:连续死亡计数,成功的读取周期清零;ready 握手只在
    // 首次成功投递后发出(spawn 侧的就绪等待不重复参与)。
    let mut ready = Some(ready);
    let mut deaths: usize = 0;
    'rewatch: loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let handle: HANDLE = match unsafe {
            CreateFileW(
                PCWSTR::from_raw(wide.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                None,
            )
        } {
            Ok(h) => h,
            Err(e) => {
                logger.log(
                    LogLevel::Warn,
                    &format!("file watcher: 打开失败 {}: {e}", root.display()),
                );
                let _ = tx.send(WatchEvent::WatcherDied(root.clone()));
                match respawn_delay(deaths) {
                    Some(delay) => {
                        logger.log(
                            LogLevel::Warn,
                            &format!(
                                "file watcher: {}s 后第 {} 次重开 {}(§146)",
                                delay.as_secs(),
                                deaths + 1,
                                root.display()
                            ),
                        );
                        sleep_stoppable(delay, &stop);
                        deaths += 1;
                        continue 'rewatch;
                    }
                    None => {
                        logger.log(
                            LogLevel::Warn,
                            &format!(
                                "file watcher: 连续死亡 {} 次,放弃重开 {}(留待下次进程启动)",
                                deaths,
                                root.display()
                            ),
                        );
                        return;
                    }
                }
            }
        };
        let event = match unsafe { CreateEventW(None, true, false, None) } {
            Ok(event) => event,
            Err(_) => {
                let _ = tx.send(WatchEvent::WatcherDied(root.clone()));
                unsafe {
                    let _ = CloseHandle(handle);
                }
                match respawn_delay(deaths) {
                    Some(delay) => {
                        sleep_stoppable(delay, &stop);
                        deaths += 1;
                        continue 'rewatch;
                    }
                    None => return,
                }
            }
        };
        // u64 缓冲保证 FILE_NOTIFY_INFORMATION 的 4 字节对齐。
        let mut buf = vec![0u64; WATCH_BUF_BYTES / 8];
        // 读循环只可能三种退场:stop(干净)、队列关闭(模块在
        // 退出,干净)、读失败(死亡 → 上报 + 重开)。
        let mut died = false;
        loop {
            if stop.load(Ordering::Acquire) {
                break;
            }
            let mut returned = 0u32;
            let mut overlapped = OVERLAPPED {
                hEvent: event,
                ..Default::default()
            };
            let mut ok = unsafe {
                let _ = ResetEvent(event);
                ReadDirectoryChangesW(
                    handle,
                    buf.as_mut_ptr() as *mut core::ffi::c_void,
                    (buf.len() * 8) as u32,
                    true, // bWatchSubtree:一句柄覆盖整棵子树
                    FILE_NOTIFY_CHANGE_FILE_NAME
                        | FILE_NOTIFY_CHANGE_DIR_NAME
                        | FILE_NOTIFY_CHANGE_SIZE
                        | FILE_NOTIFY_CHANGE_LAST_WRITE,
                    None,
                    Some(&mut overlapped),
                    None,
                )
            };
            if let Some(ready) = ready.take() {
                let _ = ready.send(());
            }
            if ok.is_ok() {
                unsafe {
                    while WaitForSingleObject(event, 100) == WAIT_TIMEOUT {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                    }
                    if stop.load(Ordering::Acquire) {
                        let _ = CancelIoEx(handle, Some(&overlapped));
                    }
                    // 取消只提交请求;必须等待完成后才能释放缓冲与 OVERLAPPED。
                    ok = GetOverlappedResult(handle, &overlapped, &mut returned, true);
                }
            }
            if stop.load(Ordering::Acquire) {
                break;
            }
            match ok {
                Err(e) => {
                    logger.log(
                        LogLevel::Warn,
                        &format!("file watcher: 读取失败 {}: {e}", root.display()),
                    );
                    let _ = tx.send(WatchEvent::WatcherDied(root.clone()));
                    died = true;
                    break;
                }
                Ok(()) if returned == 0 => {
                    if tx.send(WatchEvent::Overflow).is_err() {
                        break;
                    }
                    deaths = 0;
                }
                Ok(()) => {
                    deaths = 0;
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
            let _ = CloseHandle(event);
            let _ = CloseHandle(handle);
        }
        if stop.load(Ordering::Acquire) || !died {
            // 干净退场:stop,或事件队列已关闭(索引线程先走)。
            return;
        }
        // 读失败死亡:退避后重开。
        match respawn_delay(deaths) {
            Some(delay) => {
                logger.log(
                    LogLevel::Warn,
                    &format!(
                        "file watcher: {}s 后第 {} 次重开 {}(§146)",
                        delay.as_secs(),
                        deaths + 1,
                        root.display()
                    ),
                );
                sleep_stoppable(delay, &stop);
                deaths += 1;
                continue 'rewatch;
            }
            None => {
                logger.log(
                    LogLevel::Warn,
                    &format!(
                        "file watcher: 连续死亡 {} 次,放弃重开 {}(留待下次进程启动)",
                        deaths,
                        root.display()
                    ),
                );
                return;
            }
        }
    }
}

// ---- 索引线程:首爬 → 事件合批 → 增量应用 / 回退重爬 ----

#[allow(clippy::too_many_arguments)]
fn index_thread_main(
    inner: Arc<Mutex<Inner>>,
    roots: Vec<PathBuf>,
    exclude: Arc<Mutex<ExcludeState>>,
    exclude_noise: Arc<AtomicBool>,
    tx: EventSender,
    rx: Receiver<WatchEvent>,
    logger: ModuleLogger,
    stop: Arc<AtomicBool>,
) {
    // watcher 先开:首爬期间的变更积进队列,爬完后合批补上。
    let (watchers, watcher_done) = spawn_watchers(&roots, &tx, &logger, &stop);
    let mut policy = (
        refreshed_fragments(&exclude).0,
        exclude_noise.load(Ordering::Acquire),
    );
    // §142:首爬是同步遍历,期间没有别的消费者,4096 队列很容易在
    // 长首爬(实测 20 s)里溢出——溢出标记会让首爬刚结束就再来一次
    // 全量重爬。这里每访问一个目录排空一次队列,事件照常在首爬后
    // 合批应用。
    let mut batch: Vec<WatchEvent> = Vec::new();
    {
        let overflow = &tx.overflow;
        let mut drain = || {
            while batch.len() < INITIAL_DRAIN_CAP {
                match rx.try_recv() {
                    Ok(ev) => batch.push(ev),
                    Err(_) => break,
                }
            }
            if batch.len() >= INITIAL_DRAIN_CAP {
                overflow.store(true, Ordering::Release);
            }
        };
        publish(
            &inner,
            crawl(&roots, &policy.0, policy.1, &logger, &stop, &mut drain),
        );
    }
    while !stop.load(Ordering::Acquire) {
        // 首爬排水可能已经填了 batch:跳过这次接收,直接进合批。
        if batch.is_empty() {
            match rx.recv_timeout(Duration::from_millis(BATCH_QUIET_MS)) {
                Ok(first) => batch.push(first),
                Err(RecvTimeoutError::Timeout) => {
                    if !tx.overflow.load(Ordering::Acquire) {
                        continue;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        // 合批:静默窗结束或攒够上限,取先者。
        let started = std::time::Instant::now();
        while !batch.is_empty()
            && batch.len() < BATCH_CAP
            && !stop.load(Ordering::Acquire)
            && started.elapsed() < Duration::from_millis(BATCH_QUIET_MS)
        {
            match rx.recv_timeout(Duration::from_millis(BATCH_QUIET_MS)) {
                Ok(ev) => batch.push(ev),
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
        let mut rescan = tx.overflow.swap(false, Ordering::AcqRel);
        for ev in &batch {
            match ev {
                // §146:watcher 侧已按有界退避自行重开(表尽才放弃),
                // 这里只负责全量重爬补齐死亡窗口期的缺口。
                WatchEvent::WatcherDied(root) => {
                    logger.log(
                        LogLevel::Warn,
                        &format!(
                            "file watcher 死亡上报:{};全量重爬补齐缺口(重开在 watcher 侧)",
                            root.display()
                        ),
                    );
                    rescan = true;
                }
                WatchEvent::Overflow => rescan = true,
                WatchEvent::Change { .. } => {}
            }
        }
        // 和已发布快照的策略比较,不依赖哪个线程先读取配置的 mtime。
        let next_policy = (
            refreshed_fragments(&exclude).0,
            exclude_noise.load(Ordering::Acquire),
        );
        if rescan || policy != next_policy {
            // 丢弃重扫前积压的旧事件;重扫期间的新事件留在队列里补齐。
            for _ in 0..BATCH_CAP {
                if rx.try_recv().is_err() {
                    break;
                }
            }
            publish(
                &inner,
                crawl(
                    &roots,
                    &next_policy.0,
                    next_policy.1,
                    &logger,
                    &stop,
                    &mut || {},
                ),
            );
            policy = next_policy;
        } else {
            apply_batch(&inner, &batch, &policy.0, policy.1, &logger, &stop);
        }
        batch.clear();
    }
    stop.store(true, Ordering::Release);
    // §142:有界等待收尾。卡在取消 IO 上的 watcher 直接 detach——
    // 这里在退出路径上,泄漏一个线程好过 UI 永久假死。
    let deadline = std::time::Instant::now() + WATCH_JOIN_TIMEOUT;
    for _ in 0..watchers.len() {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if watcher_done.recv_timeout(remaining).is_err() {
            logger.log(
                LogLevel::Warn,
                "file watcher: 退出超时,放弃等待(线程随进程回收)",
            );
            break;
        }
    }
    drop(watchers);
    // 首爬取消也唤醒等待者,不遗留永远 Pending 的查询。
    publish(&inner, Vec::new());
}

/// 一批变更对旧快照的增量应用。rename 拆成"删旧 + 增新"(不配对:
/// 配对只是省一次 remove+insert,乱序/跨批到达还要配对状态机,
/// 不值得)。整棵子树的删除/搬入:RDCW 只报目录本身,删除按
/// "前缀移除",新增目录则就地补爬它的子树。
fn apply_batch(
    inner: &Arc<Mutex<Inner>>,
    batch: &[WatchEvent],
    fragments: &[String],
    noise: bool,
    logger: &ModuleLogger,
    stop: &AtomicBool,
) {
    // 首爬剪掉的目录,其后代的文件事件也必须剪掉。先过滤以免
    // AppData 等噪声变更触发 O(N) 快照重建。
    let batch: Vec<_> = batch
        .iter()
        .filter(|ev| match ev {
            WatchEvent::Change { path, .. } => {
                !(noise && path.parent().is_some_and(|p| dir_pruned(p, fragments)))
            }
            _ => false,
        })
        .collect();
    if batch.is_empty() {
        return;
    }
    let old = {
        let st = inner.lock().unwrap();
        match &st.snapshot {
            Some(s) => Arc::clone(s),
            None => return, // 首爬前的积压在首爬后才有意义;为 None 直接丢
        }
    };

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
        if stop.load(Ordering::Acquire) {
            return;
        }
        let WatchEvent::Change { path, action } = ev else {
            continue;
        };
        if *action == FILE_ACTION_REMOVED || *action == FILE_ACTION_RENAMED_OLD_NAME {
            remove_path(
                path,
                &known_dirs,
                &mut removed,
                &mut removed_prefixes,
                &mut upserts,
            );
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
                    let lower: Box<str> = path.to_string_lossy().to_lowercase().into();
                    if meta.is_dir() {
                        let reparse = meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
                        if reparse || (noise && dir_pruned(path, fragments)) {
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
                            walk_into(
                                path,
                                fragments,
                                noise,
                                &mut children,
                                &mut stats,
                                stop,
                                &mut || {},
                            );
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
            || removed_prefixes
                .iter()
                .any(|pre| p.starts_with(pre.as_ref()));
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
        .filter_map(|e| {
            let hit = tokens.iter().all(|t| match t {
                Token::Text(s) => e.path_lower.contains(s.as_str()),
                Token::ExtSuffix(suffix) => e.name_lower.ends_with(suffix.as_str()),
            });
            if !hit {
                return None;
            }
            // §142:排除名单只在命中项上判定——两个谓词是合取,顺序
            // 不影响结果集,但把 O(条目 × 片段) 降成 O(条目 × token
            // + 命中 × 片段)。34 万条目 × 20 片段曾是每键 ~170 ms。
            if filter
                && fragments_lower
                    .iter()
                    .any(|f| e.path_lower.contains(f.as_str()))
            {
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
        let root = std::env::temp_dir().join(format!("sakana-file-crawl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub\\deep")).unwrap();
        std::fs::create_dir_all(root.join("node_modules\\pkg")).unwrap();
        std::fs::write(root.join("a.txt"), b"x").unwrap();
        std::fs::write(root.join("sub\\b.txt"), b"xx").unwrap();
        std::fs::write(root.join("node_modules\\pkg\\c.js"), b"x").unwrap();

        let frags = vec![r"\node_modules\".to_string()];
        let logger: ModuleLogger = Arc::new(TestLog);
        let entries = crawl(
            std::slice::from_ref(&root),
            &frags,
            true,
            &logger,
            &AtomicBool::new(false),
            &mut || {},
        );
        let names: Vec<String> = entries.iter().map(|e| e.entry.path.to_string()).collect();
        let root_s = root.to_string_lossy().to_string();
        assert!(names.contains(&root_s));
        assert!(names.contains(&format!("{root_s}\\a.txt")));
        assert!(names.contains(&format!("{root_s}\\sub\\deep")));
        assert!(!names.iter().any(|p| p.contains("node_modules")));
        // 尺寸:文件有、目录无
        let a = entries
            .iter()
            .find(|e| e.entry.name.as_ref() == "a.txt")
            .unwrap();
        assert_eq!(a.entry.size, Some(1));
        let sub = entries
            .iter()
            .find(|e| e.entry.name.as_ref() == "sub")
            .unwrap();
        assert_eq!(sub.entry.size, None);
        // 开关关闭 → 不剪枝
        let entries = crawl(
            std::slice::from_ref(&root),
            &frags,
            false,
            &logger,
            &AtomicBool::new(false),
            &mut || {},
        );
        assert!(
            entries
                .iter()
                .any(|e| e.entry.path.contains("node_modules"))
        );

        std::fs::remove_dir_all(&root).ok();
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "sakana-index-{label}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn exclusion(path: Option<PathBuf>) -> Arc<Mutex<ExcludeState>> {
        Arc::new(Mutex::new(ExcludeState {
            path,
            mtime: None,
            fragments: vec![r"\node_modules\".into()],
            logger: None,
        }))
    }
    fn wait_for(mut predicate: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !predicate() {
            assert!(
                std::time::Instant::now() < deadline,
                "index update timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn indexed(index: &FileIndex, path: &Path) -> bool {
        index
            .inner
            .lock()
            .unwrap()
            .snapshot
            .as_ref()
            .is_some_and(|s| {
                s.iter()
                    .any(|e| e.entry.path.as_ref() == path.to_string_lossy())
            })
    }

    #[test]
    fn incremental_updates_preserve_crawl_pruning() {
        let fixture = Fixture::new("pruning");
        std::fs::create_dir_all(fixture.0.join("node_modules")).unwrap();
        let hidden = fixture.0.join("node_modules\\hidden.txt");
        std::fs::write(&hidden, "fixture").unwrap();
        let fragments = vec![r"\node_modules\".into()];
        let logger: ModuleLogger = Arc::new(TestLog);
        let stop = AtomicBool::new(false);
        let old = Arc::new(crawl(
            std::slice::from_ref(&fixture.0),
            &fragments,
            true,
            &logger,
            &stop,
            &mut || {},
        ));
        let inner = Arc::new(Mutex::new(Inner {
            snapshot: Some(old.clone()),
            wakers: vec![],
        }));
        apply_batch(
            &inner,
            &[WatchEvent::Change {
                path: hidden,
                action: FILE_ACTION_ADDED,
            }],
            &fragments,
            true,
            &logger,
            &stop,
        );
        assert!(
            Arc::ptr_eq(&old, inner.lock().unwrap().snapshot.as_ref().unwrap()),
            "noise must not rebuild the snapshot"
        );
    }

    #[test]
    fn watcher_tracks_create_rename_delete_and_consumed_policy_change() {
        let fixture = Fixture::new("watch");
        std::fs::create_dir_all(fixture.0.join("node_modules")).unwrap();
        let hidden = fixture.0.join("node_modules\\restore.txt");
        std::fs::write(&hidden, "fixture").unwrap();
        let exclude = exclusion(None);
        let index = FileIndex::start(
            vec![fixture.0.clone()],
            exclude.clone(),
            Arc::new(AtomicBool::new(true)),
            Arc::new(TestLog),
        );
        wait_for(|| index.inner.lock().unwrap().snapshot.is_some());
        assert!(!indexed(&index, &hidden));
        let created = fixture.0.join("created.txt");
        std::fs::write(&created, "new").unwrap();
        wait_for(|| indexed(&index, &created));
        let renamed = fixture.0.join("renamed.txt");
        std::fs::rename(&created, &renamed).unwrap();
        wait_for(|| indexed(&index, &renamed) && !indexed(&index, &created));
        std::fs::remove_file(&renamed).unwrap();
        wait_for(|| !indexed(&index, &renamed));

        // 模拟另一个后台读取者先消费了名单版本,查询看不到 changed。
        let config = fixture.0.join("excluded.toml");
        std::fs::write(&config, "excluded = []\n").unwrap();
        exclude.lock().unwrap().path = Some(config);
        let _ = refreshed_fragments(&exclude);
        assert!(!refreshed_fragments(&exclude).1);
        index
            .rescan_tx
            .send(WatchEvent::Change {
                path: created,
                action: FILE_ACTION_REMOVED,
            })
            .unwrap();
        wait_for(|| indexed(&index, &hidden));
        let clone = index.clone();
        index.shutdown();
        assert!(clone.service.worker.lock().unwrap().is_none());
        assert!(futures::executor::block_on(clone.wait_snapshot()).is_empty());
    }

    #[test]
    fn bounded_event_queue_requests_recovery_instead_of_growing() {
        let (tx, rx) = sync_channel(1);
        let sender = EventSender {
            tx,
            overflow: Arc::new(AtomicBool::new(false)),
        };
        for _ in 0..10_000 {
            sender.send(WatchEvent::Overflow).unwrap();
        }
        assert!(sender.overflow.load(Ordering::Acquire));
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn dropping_index_stops_workers_even_without_file_events() {
        let fixture = Fixture::new("stop");
        let index = FileIndex::start(
            vec![fixture.0.clone()],
            exclusion(None),
            Arc::new(AtomicBool::new(true)),
            Arc::new(TestLog),
        );
        let stopped = Arc::clone(&index.service.stop);
        drop(index);
        assert!(stopped.load(Ordering::Acquire));
        // 删除成功也验证 watcher 未遗留一个正在使用的目录句柄。
        std::fs::remove_dir(&fixture.0).unwrap();
    }

    struct TestLog;
    impl sakana_protocol::ModuleLog for TestLog {
        fn log(&self, _level: LogLevel, _message: &str) {}
    }

    /// §146:重生退避表——前三次延迟递增,表尽放弃。
    #[test]
    fn respawn_delay_schedule() {
        assert_eq!(respawn_delay(0), Some(Duration::from_secs(10)));
        assert_eq!(respawn_delay(1), Some(Duration::from_secs(60)));
        assert_eq!(respawn_delay(2), Some(Duration::from_secs(300)));
        assert_eq!(respawn_delay(3), None);
        assert_eq!(respawn_delay(9), None);
    }

    /// §146:退避睡眠被 stop 立即打断(第一次循环就返回),
    /// 不真睡满 300 s。
    #[test]
    fn sleep_stoppable_wakes_on_stop() {
        let stop = AtomicBool::new(true);
        let started = std::time::Instant::now();
        sleep_stoppable(Duration::from_secs(300), &stop);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// §146:坏根(打不开)立即上报 WatcherDied,不等退避;
    /// 退避睡眠可被 stop 打断,线程随后干净退出(无第二次上报)。
    #[test]
    fn dead_root_reports_and_backoff_is_stoppable() {
        let (tx, rx) = sync_channel(4);
        let sender = EventSender {
            tx,
            overflow: Arc::new(AtomicBool::new(false)),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, _ready_guard) = sync_channel::<()>(1);
        let stop_for_thread = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            watch_root(
                PathBuf::from(r"C:\sakana-test-no-such-root-9f3a"),
                sender,
                Arc::new(TestLog),
                stop_for_thread,
                ready_tx,
            );
        });
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(WatchEvent::WatcherDied(_)) => {}
            _ => panic!("expected WatcherDied for bad root"),
        }
        stop.store(true, Ordering::Release);
        let started = std::time::Instant::now();
        handle.join().unwrap();
        // 退避 10 s 被打断:join 应在亚秒级回来(5 s 留裕量)。
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(rx.try_recv().is_err());
    }
}
