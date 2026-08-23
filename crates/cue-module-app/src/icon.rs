//! 图标管线(§14、§109):异步提取,Arc 复用,完成后推
//! `PresentationInvalidated` 让 Core 重跑可见行的 present()。
//! 提取本身由 cue-util-win::icon 提供;Packaged app 的 logo 在 WinRT
//! 资源里,V1 用 SystemIcon 兜底,不提取。
//!
//! 线程模型(§99):module 自有 worker 线程串行提取,负缓存防重试风暴。

use cue_protocol::{IconImage, ItemId, ModuleEvent, ModuleEventSink, ResultIcon};
use cue_util_win::com::ComGuard;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

enum Slot {
    Pending,
    Failed,
    Ready(Arc<IconImage>),
}

struct Request {
    item_id: ItemId,
    key: String,
    exe: PathBuf,
}

pub struct IconPipeline {
    cache: Arc<Mutex<HashMap<String, Slot>>>,
    tx: Option<Sender<Request>>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl IconPipeline {
    pub fn new(sink: ModuleEventSink) -> Self {
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = channel::<Request>();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker = {
            let cache = Arc::clone(&cache);
            let shutdown = Arc::clone(&shutdown);
            std::thread::spawn(move || worker_loop(rx, cache, sink, shutdown))
        };
        Self {
            cache,
            tx: Some(tx),
            shutdown,
            worker: Some(worker),
        }
    }

    /// present() 热路径(§105 < 1 ms,无 IO):命中返回缓存图标;
    /// 未命中登记 Pending 并投递提取请求,本帧返回 None(§108 留空槽位)。
    pub fn get_or_queue(&self, item_id: ItemId, key: &str, exe: &Path) -> Option<ResultIcon> {
        let mut cache = self.cache.lock().unwrap();
        match cache.get(key) {
            // IconImage 内 rgba 是 Arc<[u8]>,clone 保持指针不变——
            // UI 按该指针缓存纹理(§14)。
            Some(Slot::Ready(icon)) => Some(ResultIcon::Raster((**icon).clone())),
            Some(Slot::Pending) | Some(Slot::Failed) => None,
            None => {
                cache.insert(key.to_string(), Slot::Pending);
                if let Some(tx) = &self.tx {
                    let _ = tx.send(Request {
                        item_id,
                        key: key.to_string(),
                        exe: exe.to_path_buf(),
                    });
                }
                None
            }
        }
    }
}

impl Drop for IconPipeline {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // 先断 sender 再 join:worker 的 recv 断开后退出。
        self.tx = None;
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

fn worker_loop(
    rx: Receiver<Request>,
    cache: Arc<Mutex<HashMap<String, Slot>>>,
    sink: ModuleEventSink,
    shutdown: Arc<AtomicBool>,
) {
    let _com = ComGuard::new();
    while let Ok(req) = rx.recv() {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let icon = cue_util_win::icon::extract_file_icon(&req.exe);
        let mut ready = Vec::new();
        {
            let mut cache = cache.lock().unwrap();
            match icon {
                Some(icon) => {
                    cache.insert(req.key, Slot::Ready(Arc::new(icon)));
                    ready.push(req.item_id);
                }
                // 负缓存:失败不重试(图标缺失不是致命问题,§63)
                None => {
                    cache.insert(req.key, Slot::Failed);
                }
            }
        }
        if !ready.is_empty() {
            sink.send(ModuleEvent::PresentationInvalidated { items: ready });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{catalog, packaged, start_menu};
    use cue_protocol::{LogLevel, ModuleLog};
    use std::path::Path;

    struct TestLog;
    impl ModuleLog for TestLog {
        fn log(&self, _level: LogLevel, _message: &str) {}
    }

    /// 离线审计(诊断工具,非常规测试):对全量 catalog 跑提取管线,
    /// 按 alpha 分布分类并落 PNG 样本到 target/icon-audit/。
    /// 运行:cargo test -p cue-module-app icon_audit -- --ignored --nocapture
    #[test]
    #[ignore]
    fn icon_audit() {
        let logger: cue_protocol::ModuleLogger = std::sync::Arc::new(TestLog);
        let mut entries = start_menu::discover(&logger);
        entries.extend(packaged::discover(&logger));
        catalog::dedup(&mut entries);

        let mut ok = 0u32;
        let mut zero_alpha: Vec<String> = Vec::new();
        let mut partial: Vec<(String, u32)> = Vec::new();
        let mut extract_failed = 0u32;
        for e in &entries {
            let crate::catalog::LaunchTarget::Win32 { exe, .. } = &e.target else {
                continue;
            };
            let Some(icon) = cue_util_win::icon::extract_file_icon(exe) else {
                extract_failed += 1;
                continue;
            };
            let total = icon.rgba.len() / 4;
            let transparent = icon.rgba.chunks_exact(4).filter(|px| px[3] == 0).count();
            if transparent == total {
                zero_alpha.push(format!("{} -> {}", e.name, exe.display()));
            } else if transparent * 100 / total > 95 {
                partial.push((
                    format!("{} -> {}", e.name, exe.display()),
                    transparent as u32,
                ));
            } else {
                ok += 1;
            }
        }
        println!("== icon audit: {ok} ok, {} all-alpha-zero, {} >95% transparent, {extract_failed} extract failed ==",
            zero_alpha.len(), partial.len());
        for s in &zero_alpha {
            println!("ZERO-ALPHA: {s}");
        }
        for (s, n) in &partial {
            println!("MOSTLY-TRANSPARENT({n}): {s}");
        }
        // 把 mostly-transparent 样本落 PNG 便于肉眼确认(target/ 下,免进 git)
        let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/icon-audit");
        std::fs::create_dir_all(&out_dir).unwrap();
        for (i, (s, _)) in partial.iter().take(9).enumerate() {
            let exe = s.rsplit("-> ").next().unwrap();
            let icon = cue_util_win::icon::extract_file_icon(Path::new(exe)).unwrap();
            let rgba: Vec<u8> = icon.rgba.to_vec();
            let img = image::RgbaImage::from_raw(icon.width, icon.height, rgba).unwrap();
            img.save(out_dir.join(format!("tiny-{i}.png"))).unwrap();
        }
    }
}
