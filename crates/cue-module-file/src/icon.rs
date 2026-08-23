//! 行图标:文件夹与无扩展名文件用通用图标(worker 启动时最先
//! 提取);具体文件的真实图标异步提取——exe 类(图标内嵌于文件)
//! 按全路径缓存,其余按扩展名取类型图标(虚拟名,不触盘)。
//! 完成后推 `PresentationInvalidated` 让 Core 重跑可见行。
//! 提取本身由 cue-util-win::icon 提供。
//!
//! 线程模型:module 自有 worker 线程串行提取(与 AppModule 图标
//! 管线同款);负缓存防重试风暴;缓存超界整体清空(重建很便宜)。

use cue_protocol::{IconImage, ItemId, ModuleEvent, ModuleEventSink, ResultIcon};
use cue_util_win::com::ComGuard;
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};

/// 通用文件夹 / 文件图标(worker 线程最先提取;失败则 present
/// 走 SystemIcon 兜底)。
pub struct FileIcons {
    pub folder: Arc<IconImage>,
    pub file: Arc<IconImage>,
}

enum Slot {
    Pending,
    Failed,
    Ready(Arc<IconImage>),
}

/// 缓存上限:单张 96×96×4 ≈ 37 KB,超界整体清空(提取便宜,
/// 不做 LRU)。
const CACHE_CAP: usize = 512;

/// exe 类:图标内嵌于文件,按全路径缓存;其余按扩展名缓存
/// (同类型文件共享一枚)。文件夹与无扩展名文件返回 None
/// (通用图标,不进提取队列)。
fn icon_key(path: &Path, is_dir: bool) -> Option<String> {
    if is_dir {
        return None;
    }
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    if matches!(
        ext.as_str(),
        "exe" | "lnk" | "msi" | "bat" | "cmd" | "com" | "scr"
    ) {
        Some(format!("path:{}", path.to_string_lossy()))
    } else {
        Some(format!("ext:{ext}"))
    }
}

/// 按 key 提取:全路径走真实文件(exe 内嵌图标);扩展名走
/// 虚拟文件名 "x.<ext>"(SHGFI_USEFILEATTRIBUTES,不触盘)。
fn extract(key: &str) -> Option<IconImage> {
    if let Some(p) = key.strip_prefix("path:") {
        cue_util_win::icon::extract_file_icon(Path::new(p))
    } else if let Some(ext) = key.strip_prefix("ext:") {
        cue_util_win::icon::extract_virtual_icon(&format!("x.{ext}"), FILE_ATTRIBUTE_NORMAL)
    } else {
        None
    }
}

/// 图标 worker:启动时先提取两枚通用图标(填 OnceLock),随后
/// 串行服务提取队列。sender 全部断开即退出(module Drop 时)。
pub struct IconWorker {
    cache: Arc<Mutex<HashMap<String, Slot>>>,
    tx: Option<Sender<String>>,
    worker: Option<JoinHandle<()>>,
}

impl IconWorker {
    pub fn new(
        generic: Arc<OnceLock<FileIcons>>,
        last_items: Arc<Mutex<Vec<ItemId>>>,
        sink: ModuleEventSink,
    ) -> Self {
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = channel::<String>();
        let worker = {
            let cache = Arc::clone(&cache);
            std::thread::spawn(move || worker_loop(rx, cache, generic, last_items, sink))
        };
        Self {
            cache,
            tx: Some(tx),
            worker: Some(worker),
        }
    }

    /// present() 热路径(< 1 ms,无 IO):命中返回缓存图标;
    /// 未命中登记 Pending 并投递提取请求,本帧返回 None(走通用图标)。
    pub fn get_or_queue(&self, path: &Path, is_dir: bool) -> Option<ResultIcon> {
        let key = icon_key(path, is_dir)?;
        let mut cache = self.cache.lock().unwrap();
        match cache.get(&key) {
            // IconImage 内 rgba 是 Arc<[u8]>,clone 保持指针不变——
            // UI 按该指针缓存纹理。
            Some(Slot::Ready(icon)) => Some(ResultIcon::Raster((**icon).clone())),
            Some(Slot::Pending) | Some(Slot::Failed) => None,
            None => {
                if cache.len() >= CACHE_CAP {
                    cache.clear();
                }
                cache.insert(key.clone(), Slot::Pending);
                if let Some(tx) = &self.tx {
                    let _ = tx.send(key);
                }
                None
            }
        }
    }
}

impl Drop for IconWorker {
    fn drop(&mut self) {
        // 断 sender:worker 的 recv 出错后退出。
        self.tx = None;
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

fn worker_loop(
    rx: Receiver<String>,
    cache: Arc<Mutex<HashMap<String, Slot>>>,
    generic: Arc<OnceLock<FileIcons>>,
    last_items: Arc<Mutex<Vec<ItemId>>>,
    sink: ModuleEventSink,
) {
    let _com = ComGuard::new();
    // 先提取两枚通用图标:OnceLock 只填一次;失败不重试
    //(present 的 SystemIcon 兜底一直在)。
    if generic.get().is_none()
        && let Some(icons) = (|| {
            Some(FileIcons {
                folder: Arc::new(cue_util_win::icon::extract_virtual_icon(
                    "folder",
                    FILE_ATTRIBUTE_DIRECTORY,
                )?),
                file: Arc::new(cue_util_win::icon::extract_virtual_icon(
                    "file",
                    FILE_ATTRIBUTE_NORMAL,
                )?),
            })
        })()
        && generic.set(icons).is_ok()
    {
        let items = last_items.lock().unwrap().clone();
        if !items.is_empty() {
            sink.send(ModuleEvent::PresentationInvalidated { items });
        }
    }
    while let Ok(key) = rx.recv() {
        let icon = extract(&key);
        let mut cache = cache.lock().unwrap();
        match icon {
            Some(icon) => {
                cache.insert(key, Slot::Ready(Arc::new(icon)));
                // 失效寻址用当前行快照:Core 只取与当前结果的交集,
                // 同扩展名的兄弟行一并覆盖(请求登记行可能已滚走)。
                let items = last_items.lock().unwrap().clone();
                drop(cache);
                if !items.is_empty() {
                    sink.send(ModuleEvent::PresentationInvalidated { items });
                }
            }
            // 负缓存:失败不重试(图标缺失不是致命问题)
            None => {
                cache.insert(key, Slot::Failed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// key 规则:文件夹/无扩展名 None;exe 类按全路径;其余按
    /// 小写扩展名(同类文件共享一枚图标)。
    #[test]
    fn icon_key_routing() {
        assert!(icon_key(Path::new(r"C:\Alpha"), true).is_none());
        assert!(icon_key(Path::new(r"C:\Alpha\README"), false).is_none());
        assert_eq!(
            icon_key(Path::new(r"C:\app\geek.EXE"), false).as_deref(),
            Some(r"path:C:\app\geek.EXE")
        );
        assert_eq!(
            icon_key(Path::new(r"C:\app\setup.lnk"), false).as_deref(),
            Some(r"path:C:\app\setup.lnk")
        );
        assert_eq!(
            icon_key(Path::new(r"C:\docs\Report.PDF"), false).as_deref(),
            Some("ext:pdf")
        );
        assert_eq!(
            icon_key(Path::new(r"C:\docs\note.txt"), false).as_deref(),
            Some("ext:txt")
        );
    }

    /// extract 的 key 分发:未知前缀 None;ext 走虚拟名(真实
    /// Win32 冒烟在 cue-util-win 已覆盖)。
    #[test]
    fn extract_rejects_unknown_prefix() {
        assert!(extract("weird:key").is_none());
    }
}
