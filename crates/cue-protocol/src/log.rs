//! 诊断日志:全局 sink,单文件有界,后台写线程。
//!
//! 热路径纪律(§55):调用方只 `format!` + 一次 channel send(纳秒级),
//! 文件 IO 全在专属写线程——杀毒/磁盘抖动卡的是写线程,不是 UI 线程。
//! 不做 fsync:WriteFile 进 OS 页缓存后进程崩溃也不丢(缓存归内核管),
//! 电源失效级别的持久化不是诊断日志的目标。
//!
//! 平台中立:只用 std(UTC 时间戳手算,不碰本地时区),§110 不受影响。

use std::io::Write as _;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::mpsc::{SendError, Sender};
use std::time::SystemTime;

pub const LOG_FILE_NAME: &str = "cue.log";
pub const LOG_FILE_OLD: &str = "cue.log.old";
/// 超界滚动:启动时 cue.log > 1MB 就改名 .old(覆盖上一代)从零开始,
/// 总量封顶 2MB——与 usage.tsv 的"有界"同一纪律。
const ROTATE_AT: u64 = 1024 * 1024;

static SINK: OnceLock<Sender<String>> = OnceLock::new();

/// 进程启动时调用一次(重复调用后者落空)。任何 IO 失败都退回纯
/// stderr——日志不许压垮主功能(§63 同款纪律)。
pub fn init(path: &Path) {
    if SINK.get().is_some() {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(meta) = std::fs::metadata(path)
        && meta.len() > ROTATE_AT
    {
        let _ = std::fs::rename(path, path.with_file_name(LOG_FILE_OLD));
    }
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
        return; // 无文件也能活:write() 全部落 stderr
    };
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    if SINK.set(tx).is_err() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("log-writer".into())
        .spawn(move || {
            // File 不带用户态缓冲:writeln 即一次 WriteFile 进页缓存
            // (µs 级),写线程内无所谓;不缓冲 = 崩溃不留未冲刷残段。
            while let Ok(line) = rx.recv() {
                let _ = writeln!(file, "{line}");
                eprintln!("{line}");
            }
        });
}

/// 一行入口(logln! 宏与 Core 的 ModuleLogger 实现共用)。
/// 补 UTC 时间戳后交给写线程;未 init / 写线程已死 → 纯 stderr。
pub fn write(msg: String) {
    let line = format!("{} {msg}", utc_now());
    let fallback = match SINK.get() {
        Some(tx) => match tx.send(line) {
            Ok(()) => return,
            Err(SendError(line)) => line,
        },
        None => line,
    };
    eprintln!("{fallback}");
}

/// `eprintln!` 的同形替换:`logln!("[tag] {x:?}")`。
#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => { $crate::log::write(format!($($arg)*)) };
}

/// UTC 时间戳(RFC3339 秒级)。SystemTime 异常时退化为纪元起点,
/// 日志行永不因时钟 panic。
fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    let sod = secs % 86400;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", sod / 3600, sod % 3600 / 60, sod % 60)
}

/// Howard Hinnant 的 civil_from_days(公历,1970 纪元),纯整数。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_epoch_and_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19723), (2024, 1, 1)); // 1704067200 / 86400
        assert_eq!(civil_from_days(19782), (2024, 2, 29)); // 闰日
        assert_eq!(civil_from_days(-1), (1969, 12, 31)); // 纪元前不绕断
    }

    #[test]
    fn utc_now_formats_rfc3339_seconds() {
        let ts = utc_now();
        assert_eq!(ts.len(), 20);
        assert!(ts.ends_with('Z'));
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[10..11], "T");
    }

    #[test]
    fn write_without_init_falls_back_to_stderr() {
        // 不 panic 就是全部断言(测试进程未 init)。
        write("test line".to_string());
    }

    #[test]
    fn init_rotates_oversize_log_and_roundtrips() {
        let dir = std::env::temp_dir().join(format!("cue-log-test-{}", std::process::id()));
        let path = dir.join(LOG_FILE_NAME);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, vec![b'x'; (ROTATE_AT + 1) as usize]).unwrap();

        // 同进程只能 init 一次(OnceLock):本测试依赖先于其他 init
        // 调用运行——把"已 init"分支也一并覆盖:重复 init 是 no-op。
        init(&path);
        init(&path);

        let rotated = dir.join(LOG_FILE_OLD);
        if SINK.get().is_some() {
            // 本进程首次 init:滚动已发生,写线程在线。
            assert!(rotated.exists());
            write("roundtrip line".to_string());
            let mut seen = String::new();
            for _ in 0..100 {
                seen = std::fs::read_to_string(&path).unwrap_or_default();
                if seen.contains("roundtrip line") {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(seen.contains("roundtrip line"), "log file: {seen}");
        } else {
            // 已被其他测试 init:重复 init 是 no-op,不产生滚动。
            assert!(!rotated.exists());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
