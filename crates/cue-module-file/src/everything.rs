//! Everything 1.4 IPC 客户端:专用线程 + 容量 1 的 latest-wins
//! 请求槽 + WM_COPYDATA 同步往返。
//!
//! 协议逐字节对齐官方 SDK(`ipc/everything_ipc.h`、`src/Everything.c`,
//! SDK 压缩包随取随弃,不进仓库、不链 Everything.dll):
//!
//! - 找服务:`FindWindowW("EVERYTHING_TASKBAR_NOTIFICATION")`,版本握手
//!   `SendMessageW(hwnd, WM_USER, 0, 0)` → major(QUERY2W 需要 1.4.1+);
//! - 查询:`WM_COPYDATA`,`dwData = 18`,负载为 pack(1) 的 QUERY2
//!   (7 个 u32 头 + 以 NUL 结尾的 UTF-16 搜索串);
//! - 应答:Everything 把 WM_COPYDATA 发回我们指定的 reply_hwnd,`dwData`
//!   回声我们给的 id(官方 dll 用 0),负载为 pack(1) 的 LIST2
//!   (5 个 u32 头 + ITEM2[numitems](flags, data_offset) + 变长数据);
//! - 变长数据按 request flag 位升序排列:字符串 = u32 字符数(不含
//!   NUL)+ 文本;SIZE/DATE = 8 字节。所有读取先查边界。
//!
//! 应答窗口必须在发查询的线程上创建并用消息泵等待——应答是"发送"
//! 消息,只有泵(GetMessage)才会派发它(官方 dll 同款流程)。超时由
//! WM_TIMER 兜底:Everything 已接收但迟迟不应答时不至于吊死线程。

use cue_protocol::ModuleError;
use futures::channel::oneshot;
use std::cell::{Cell, RefCell};
use std::sync::{Arc, Condvar, Mutex, Once};
use std::time::{Duration, SystemTime};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

// ---- 协议常量(everything_ipc.h) ----

const EVERYTHING_WNDCLASS: &[u16] = &[
    0x45, 0x56, 0x45, 0x52, 0x59, 0x54, 0x48, 0x49, 0x4E, 0x47, 0x5F, 0x54, 0x41, 0x53, 0x4B, 0x42,
    0x41, 0x52, 0x5F, 0x4E, 0x4F, 0x54, 0x49, 0x46, 0x49, 0x43, 0x41, 0x54, 0x49, 0x4F, 0x4E, 0x00,
]; // "EVERYTHING_TASKBAR_NOTIFICATION\0"
const EVERYTHING_WM_IPC: u32 = 0x0400; // WM_USER
const IPC_GET_MAJOR_VERSION: usize = 0;
const IPC_COPYDATA_QUERY2W: usize = 18;
/// 应答 WM_COPYDATA 的 dwData:任意值,Everything 原样回声;官方 dll 用 0。
const REPLY_ID: usize = 0;
const SORT_NAME_ASCENDING: u32 = 1; // SDK:名字升序永远无性能损失
const REQ_FULL_PATH_AND_NAME: u32 = 0x04;
const REQ_SIZE: u32 = 0x10;
const REQ_DATE_MODIFIED: u32 = 0x40;
const REQUEST_FLAGS: u32 = REQ_FULL_PATH_AND_NAME | REQ_SIZE | REQ_DATE_MODIFIED;
const ITEM_FOLDER: u32 = 0x01;
const QUERY_TIMEOUT_MS: u32 = 2000;
const REPLY_CLASS: &[u16] = &[
    0x43, 0x55, 0x45, 0x2E, 0x45, 0x76, 0x65, 0x72, 0x79, 0x74, 0x68, 0x69, 0x6E, 0x67, 0x49, 0x70,
    0x63, 0x00,
]; // "CUE.EverythingIpc\0"

/// FILETIME(1601 纪元,100ns 单位)→ Unix 纪元的差值。
const FILETIME_UNIX_EPOCH_DELTA: u64 = 116_444_736_000_000_000;

/// 业务对象只留在模块内部;Core 只见 ItemId。
#[derive(Clone, Debug)]
pub struct FileEntry {
    /// Everything 返回的全路径(原样,即 usage 的 item_key)。
    pub path: Arc<str>,
    /// 文件名部分(展示标题)。
    pub name: Arc<str>,
    /// 父目录部分(展示副标题);盘符根目录为空。
    pub parent: Arc<str>,
    pub is_dir: bool,
    pub size: Option<u64>,
    /// V1 不展示;请求该字段是为让变长解析的 flag 位序有第三个数据点
    /// (测试断言其值域),V1.x 展示/排序会直接用。
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

/// 一次 IPC 往返的请求;被更新的请求顶掉时,旧 sender 直接 drop——
/// 其 rx 以 Canceled 结束(latest-wins;Core 反正会按 ticket 丢弃)。
struct Request {
    search: String,
    max_results: u32,
    reply: oneshot::Sender<Result<Vec<FileEntry>, ModuleError>>,
}

/// 专用 IPC 线程 + latest-wins 槽。clone 廉价(内部全 Arc)。
#[derive(Clone)]
pub struct EverythingBackend {
    slot: Arc<(Mutex<Option<Request>>, Condvar)>,
    init_failed: Arc<std::sync::atomic::AtomicBool>,
}

impl EverythingBackend {
    /// 启动专用线程(窗口与消息泵都在线程内)。线程随进程生命
    /// (模块 unload 不回收,同 AppModule catalog 线程的处理)。
    pub fn start(logger: cue_protocol::ModuleLogger) -> Self {
        let backend = Self {
            slot: Arc::new((Mutex::new(None), Condvar::new())),
            init_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let slot = Arc::clone(&backend.slot);
        let init_failed = Arc::clone(&backend.init_failed);
        std::thread::spawn(move || ipc_thread_main(slot, init_failed, logger));
        backend
    }

    /// 投递一个查询:覆盖槽内旧请求(如有),立即返回 async 等待句柄。
    /// 调用本身不触碰 IO。
    pub fn query(
        &self,
        search: String,
        max_results: u32,
    ) -> oneshot::Receiver<Result<Vec<FileEntry>, ModuleError>> {
        let (tx, rx) = oneshot::channel();
        if self.init_failed.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = tx.send(Err(ModuleError::Unavailable(
                "Everything IPC 初始化失败".into(),
            )));
            return rx;
        }
        let (lock, cv) = &*self.slot;
        {
            let mut slot = lock.lock().unwrap();
            *slot = Some(Request {
                search,
                max_results,
                reply: tx,
            });
        }
        cv.notify_one();
        rx
    }
}

thread_local! {
    /// 应答负载暂存(wndproc 与泵同线程,无需锁)。
    static REPLY_STASH: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
    static REPLY_TIMED_OUT: Cell<bool> = const { Cell::new(false) };
}

fn ipc_thread_main(
    slot: Arc<(Mutex<Option<Request>>, Condvar)>,
    init_failed: Arc<std::sync::atomic::AtomicBool>,
    logger: cue_protocol::ModuleLogger,
) {
    let window = match create_reply_window() {
        Some(w) => w,
        None => {
            init_failed.store(true, std::sync::atomic::Ordering::Relaxed);
            logger.log(
                cue_protocol::LogLevel::Error,
                "file: Everything IPC 应答窗口创建失败,模块不可用",
            );
            return;
        }
    };
    let (lock, cv) = &*slot;
    loop {
        let req = {
            let mut guard = lock.lock().unwrap();
            loop {
                if let Some(req) = guard.take() {
                    break req;
                }
                guard = cv.wait(guard).unwrap();
            }
        };
        let outcome = round_trip(window, &req.search, req.max_results);
        // 接收方可能已因 generation 过期被 Core 丢弃——send 失败属正常。
        let _ = req.reply.send(outcome);
    }
}

/// 单次查询的完整往返:可用性检查 → 发 QUERY2W → 泵等应答/超时 → 解析。
fn round_trip(window: HWND, search: &str, max_results: u32) -> Result<Vec<FileEntry>, ModuleError> {
    let everything = find_everything()?;
    let query = build_query(window, search, max_results);
    REPLY_STASH.with(|s| *s.borrow_mut() = None);
    REPLY_TIMED_OUT.with(|t| t.set(false));
    unsafe {
        let cds = COPYDATASTRUCT {
            dwData: IPC_COPYDATA_QUERY2W,
            cbData: query.len() as u32,
            lpData: query.as_ptr() as *mut core::ffi::c_void,
        };
        // 已接收但无应答的兜底:超时由 WM_TIMER 退出泵。
        SetTimer(Some(window), 1, QUERY_TIMEOUT_MS, None);
        // wParam = 我们的应答窗口(官方 dll 同款)。SendMessage 返回 TRUE
        // 表示 Everything 接受查询;0 = 被拒(版本/权限,如 UIPI 拦截)。
        let accepted = SendMessageW(
            everything,
            WM_COPYDATA,
            Some(WPARAM(window.0 as usize)),
            Some(LPARAM(&cds as *const COPYDATASTRUCT as isize)),
        );
        if accepted == LRESULT(0) {
            KillTimer(Some(window), 1).ok();
            return Err(ModuleError::QueryFailed(
                "Everything 拒绝了 IPC 查询(版本过旧或权限隔离)".into(),
            ));
        }
        // 官方 dll 同款泵:sent 消息在 GetMessage 等待中被派发;
        // wndproc 收到应答(或超时)后 PostQuitMessage 退出泵。
        loop {
            let mut msg = MSG::default();
            match GetMessageW(&mut msg, None, 0, 0).0 {
                0 | -1 => break, // WM_QUIT 或错误
                _ => {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
        KillTimer(Some(window), 1).ok();
    }
    if REPLY_TIMED_OUT.with(|t| t.get()) {
        return Err(ModuleError::QueryFailed("Everything 查询超时".into()));
    }
    let bytes = REPLY_STASH.with(|s| s.borrow_mut().take());
    match bytes {
        Some(b) => parse_list2(&b),
        None => Err(ModuleError::QueryFailed(
            "Everything 应答缺失(泵异常退出)".into(),
        )),
    }
}

/// 找 Everything 的 IPC 窗口并握手主版本(1 = 1.4.x,QUERY2W 需要 1.4.1+)。
fn find_everything() -> Result<HWND, ModuleError> {
    unsafe {
        let hwnd = FindWindowW(PCWSTR(EVERYTHING_WNDCLASS.as_ptr()), PCWSTR::null())
            .map_err(|_| ModuleError::Unavailable("Everything 未运行".into()))?;
        if hwnd.0.is_null() {
            return Err(ModuleError::Unavailable("Everything 未运行".into()));
        }
        let major = SendMessageW(
            hwnd,
            EVERYTHING_WM_IPC,
            Some(WPARAM(IPC_GET_MAJOR_VERSION)),
            None,
        );
        if major.0 != 1 {
            return Err(ModuleError::Unavailable(format!(
                "Everything 主版本 {major:?} 不受支持(需要 1.4.x)"
            )));
        }
        Ok(hwnd)
    }
}

/// QUERY2(pack 1):7 个 u32 头 + NUL 结尾的 UTF-16 搜索串。
fn build_query(reply: HWND, search: &str, max_results: u32) -> Vec<u8> {
    let wide: Vec<u16> = search.encode_utf16().chain(Some(0)).collect();
    let mut buf = Vec::with_capacity(28 + wide.len() * 2);
    let mut push = |v: u32| buf.extend_from_slice(&v.to_le_bytes());
    push(reply.0 as u32); // SDK:x64 下窗口句柄也只有效于低 32 位
    push(REPLY_ID as u32);
    push(0); // search_flags:Everything 默认(子串、忽略大小写)
    push(0); // offset
    push(max_results);
    push(REQUEST_FLAGS);
    push(SORT_NAME_ASCENDING);
    for u in wide {
        buf.extend_from_slice(&u.to_le_bytes());
    }
    buf
}

/// LIST2 解析:5 个 u32 头 + ITEM2[numitems](各 8 字节)+ 变长数据。
/// 变长字段按 request flag 位升序;一切读取查边界(外部数据永不 panic)。
fn parse_list2(bytes: &[u8]) -> Result<Vec<FileEntry>, ModuleError> {
    fn bad(reason: &str) -> ModuleError {
        ModuleError::QueryFailed(format!("Everything 应答格式异常:{reason}"))
    }
    if bytes.len() < 20 {
        return Err(bad("头部不足 20 字节"));
    }
    let rd = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let num = rd(4) as usize;
    let request_flags = rd(12);
    let items_end = 20 + num * 8;
    if bytes.len() < items_end {
        return Err(bad("条目表越界"));
    }
    let mut out = Vec::with_capacity(num);
    for i in 0..num {
        let flags = rd(20 + i * 8);
        let mut p = rd(24 + i * 8) as usize;
        let mut path: Option<String> = None;
        let mut size: Option<u64> = None;
        let mut modified: Option<SystemTime> = None;
        // 位序升序:0x04 FULL_PATH_AND_NAME → 0x10 SIZE → 0x40 DATE_MODIFIED。
        if request_flags & REQ_FULL_PATH_AND_NAME != 0 {
            let (s, next) = read_len_wstr(bytes, p).ok_or_else(|| bad("路径字符串越界"))?;
            path = Some(s);
            p = next;
        }
        if request_flags & REQ_SIZE != 0 {
            let (v, next) = read_u64(bytes, p).ok_or_else(|| bad("size 越界"))?;
            // 文件夹等无 size 时 Everything 给 u64::MAX。
            size = (v != u64::MAX).then_some(v);
            p = next;
        }
        if request_flags & REQ_DATE_MODIFIED != 0 {
            let (v, next) = read_u64(bytes, p).ok_or_else(|| bad("date 越界"))?;
            modified = filetime_to_system_time(v);
            p = next;
        }
        let _ = p;
        let Some(path) = path else {
            return Err(bad("缺少全路径字段"));
        };
        out.push(make_entry(path, flags & ITEM_FOLDER != 0, size, modified));
    }
    Ok(out)
}

/// u32 字符数(不含 NUL)+ UTF-16 文本 + NUL。返回 (String, 下一偏移)。
fn read_len_wstr(bytes: &[u8], at: usize) -> Option<(String, usize)> {
    let len = u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
    let start = at + 4;
    let end = start + len * 2;
    let raw = bytes.get(start..end)?;
    let wide: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    // 跳过 NUL 终止符(协议必有;缺也不致命,不查)。
    Some((String::from_utf16_lossy(&wide), end + 2))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<(u64, usize)> {
    let v = u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?);
    Some((v, at + 8))
}

/// FILETIME(100ns,1601 纪元)→ SystemTime;0 = 无效 → None。
fn filetime_to_system_time(ft: u64) -> Option<SystemTime> {
    if ft == 0 {
        return None;
    }
    let ticks = ft.checked_sub(FILETIME_UNIX_EPOCH_DELTA)?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_nanos(ticks * 100))
}

// ---- 应答窗口(协议的基础设施,全部在 IPC 线程上) ----

static REGISTER_CLASS: Once = Once::new();

fn create_reply_window() -> Option<HWND> {
    unsafe {
        let hmod = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok()?;
        REGISTER_CLASS.call_once(|| {
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                hInstance: windows::Win32::Foundation::HINSTANCE(hmod.0),
                lpfnWndProc: Some(reply_wndproc),
                lpszClassName: PCWSTR(REPLY_CLASS.as_ptr()),
                ..Default::default()
            };
            let atom = RegisterClassExW(&class);
            debug_assert!(atom != 0, "RegisterClassExW failed");
        });
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(REPLY_CLASS.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(windows::Win32::Foundation::HINSTANCE(hmod.0)),
            None,
        )
        .ok()?;
        (!hwnd.0.is_null()).then_some(hwnd)
    }
}

unsafe extern "system" fn reply_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_COPYDATA => {
                let cds = &*(lparam.0 as *const COPYDATASTRUCT);
                if cds.dwData == REPLY_ID && !cds.lpData.is_null() {
                    let bytes =
                        std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize)
                            .to_vec();
                    REPLY_STASH.with(|s| *s.borrow_mut() = Some(bytes));
                    PostQuitMessage(0);
                }
                LRESULT(1) // SDK:TRUE = 已处理
            }
            WM_TIMER => {
                REPLY_TIMED_OUT.with(|t| t.set(true));
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QUERY2 布局:头部 7 个 u32 小端,随后 UTF-16 + NUL。
    #[test]
    fn query_layout_matches_sdk() {
        let hwnd = HWND(0x11223344 as *mut core::ffi::c_void);
        let buf = build_query(hwnd, "cue", 8);
        let rd = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
        assert_eq!(rd(0), 0x11223344);
        assert_eq!(rd(4), REPLY_ID as u32);
        assert_eq!(rd(8), 0); // search flags
        assert_eq!(rd(12), 0); // offset
        assert_eq!(rd(16), 8); // max_results
        assert_eq!(rd(20), REQUEST_FLAGS);
        assert_eq!(rd(24), SORT_NAME_ASCENDING);
        let tail: Vec<u16> = buf[28..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(tail, vec![0x63, 0x75, 0x65, 0]); // "cue\0"
    }

    /// 造一个 LIST2 缓冲(2 条:一文件夹一文件),验证解析与字段顺序。
    fn synthetic_list() -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut push32 = |v: u32| bytes.extend_from_slice(&v.to_le_bytes());
        // 头部:num=2, request_flags=REQUEST_FLAGS, sort=1
        push32(2); // totitems
        push32(2); // numitems
        push32(0); // offset
        push32(REQUEST_FLAGS);
        push32(1);
        // 变长数据区在 20 + 2*8 = 36 之后
        let mut data = Vec::new();
        // item 0:文件夹 "C:\\Alpha"(8 chars;wire = len 前缀(不含 NUL)+ 文本 + NUL)
        let off0 = 36 + data.len() as u32;
        let p0: Vec<u16> = "C:\\Alpha".encode_utf16().chain(Some(0)).collect();
        data.extend_from_slice(&(8u32).to_le_bytes());
        for u in &p0 {
            data.extend_from_slice(&u.to_le_bytes());
        }
        data.extend_from_slice(&u64::MAX.to_le_bytes()); // size:文件夹无
        data.extend_from_slice(&0u64.to_le_bytes()); // date:无
        // item 1:文件 "C:\\Alpha\\beta.txt",size=1234,date=某时刻
        let off1 = 36 + data.len() as u32;
        let p1: Vec<u16> = "C:\\Alpha\\beta.txt"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        data.extend_from_slice(&(17u32).to_le_bytes());
        for u in &p1 {
            data.extend_from_slice(&u.to_le_bytes());
        }
        data.extend_from_slice(&1234u64.to_le_bytes());
        let ft = FILETIME_UNIX_EPOCH_DELTA + 1_000_000_000; // +100s
        data.extend_from_slice(&ft.to_le_bytes());
        // 条目表
        push32(ITEM_FOLDER); // flags
        push32(off0);
        push32(0); // flags:文件
        push32(off1);
        bytes.extend_from_slice(&data);
        bytes
    }

    #[test]
    fn list2_parses_entries() {
        let entries = parse_list2(&synthetic_list()).expect("parse");
        assert_eq!(entries.len(), 2);
        let dir = &entries[0];
        assert!(dir.is_dir);
        assert_eq!(&*dir.path, "C:\\Alpha");
        assert_eq!(&*dir.name, "Alpha");
        assert_eq!(&*dir.parent, "C:");
        assert_eq!(dir.size, None);
        assert_eq!(dir.modified, None);
        let file = &entries[1];
        assert!(!file.is_dir);
        assert_eq!(&*file.name, "beta.txt");
        assert_eq!(&*file.parent, "C:\\Alpha");
        assert_eq!(file.size, Some(1234));
        assert_eq!(
            file.modified,
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(100))
        );
    }

    /// 截断/畸形应答一律 Err,不 panic。
    #[test]
    fn list2_never_panics_on_garbage() {
        assert!(parse_list2(&[]).is_err());
        assert!(parse_list2(&[0u8; 19]).is_err());
        let mut buf = synthetic_list();
        buf.truncate(30); // 条目表都不全
        assert!(parse_list2(&buf).is_err());
        let mut buf = synthetic_list();
        buf.truncate(buf.len() - 3); // 数据区咬掉一口
        assert!(parse_list2(&buf).is_err());
        // 全零头部(num=0)合法:空结果
        let zero = {
            let mut b = Vec::new();
            for v in [0u32, 0, 0, REQUEST_FLAGS, 1] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b
        };
        assert_eq!(parse_list2(&zero).unwrap().len(), 0);
    }

    #[test]
    fn parent_name_split() {
        assert_eq!(
            split_parent_name("C:\\a\\b.txt"),
            ("C:\\a".to_string(), "b.txt".to_string())
        );
        assert_eq!(
            split_parent_name("C:\\"),
            (String::new(), "C:\\".to_string())
        );
        assert_eq!(
            split_parent_name("\\\\nas\\share\\x"),
            ("\\\\nas\\share".to_string(), "x".to_string())
        );
    }

    /// 真机冒烟(本机 Everything 1.4 常驻):查任何 Windows 都有的
    /// explorer.exe。Everything 不在时静默跳过(CI/其他机器)。
    #[test]
    fn live_everything_round_trip() {
        if find_everything().is_err() {
            eprintln!("Everything not running, skipping live test");
            return;
        }
        let logger: cue_protocol::ModuleLogger = std::sync::Arc::new(NullLog);
        let backend = EverythingBackend::start(logger);
        let result = futures::executor::block_on(backend.query("explorer.exe".into(), 8))
            .expect("canceled")
            .expect("query");
        assert!(!result.is_empty(), "本机应能搜到 explorer.exe");
        // 任一文件条目都应有 size;字段顺序若错会解出 None/荒谬值。
        let file = result.iter().find(|e| !e.is_dir).expect("应有文件条目");
        assert!(file.size.is_some(), "文件应有 size(字段顺序正确性)");
        for e in &result {
            assert!(!e.name.is_empty());
            assert!(!e.path.contains('\0'), "路径不应带 NUL(len 不含 NUL)");
            if let Some(m) = e.modified {
                let secs = m.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
                assert!(
                    (946684800..4102444800).contains(&secs),
                    "date 应在 2000..2100"
                );
            }
        }
    }

    struct NullLog;
    impl cue_protocol::ModuleLog for NullLog {
        fn log(&self, _level: cue_protocol::LogLevel, _message: &str) {}
    }
}
