//! 图标管线(§14、§109):异步提取,Arc 复用,完成后推
//! `PresentationInvalidated` 让 Core 重跑可见行的 present()。
//!
//! 提取路径:`SHGetFileInfoW(SHGFI_SYSICONINDEX)` → `SHGetImageList(SHIL_JUMBO)`
//! (256px)→ HICON → `DrawIconEx` 进 32bpp top-down DIB → BGRA→RGBA,
//! 单尺寸 96px(§14;UI 降采样,纹理按 Arc 指针缓存,故缓存只存 Arc)。
//! Packaged app 的 logo 在 WinRT 资源里,V1 用 SystemIcon 兜底,不提取。
//!
//! 线程模型(§99):module 自有 worker 线程串行提取,负缓存防重试风暴。

use crate::com::ComGuard;
use cue_protocol::{IconImage, ItemId, ModuleEvent, ModuleEventSink, ResultIcon};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::{
    SHGetFileInfoW, SHGetImageList, SHFILEINFOW, SHGFI_SYSICONINDEX, SHIL_JUMBO,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// §14:单尺寸 96px。
const ICON_SIZE: u32 = 96;

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
    pub fn get_or_queue(&self, item_id: ItemId, key: &str, exe: &PathBuf) -> Option<ResultIcon> {
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
                        exe: exe.clone(),
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
        let icon = extract_icon(&req.exe);
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

/// 提取 exe 的 256px 系统图标并降采样到 96px RGBA。
fn extract_icon(exe: &PathBuf) -> Option<IconImage> {
    unsafe {
        let wide: Vec<u16> = exe
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut shfi = SHFILEINFOW::default();
        let got = SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut shfi),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX,
        );
        if got == 0 {
            return None;
        }
        let list: IImageList = SHGetImageList(SHIL_JUMBO as i32).ok()?;
        let hicon = list.GetIcon(shfi.iIcon, ILD_TRANSPARENT.0).ok()?;
        let icon = hicon_to_rgba(hicon, ICON_SIZE);
        let _ = DestroyIcon(hicon);
        icon
    }
}

/// HICON → 96×96 RGBA(straight alpha,§14 像素契约)。
/// GDI 32bpp DIB 为 BGRA,逐像素交换 R/B。
fn hicon_to_rgba(hicon: HICON, size: u32) -> Option<IconImage> {
    unsafe {
        let hdc = CreateCompatibleDC(None);
        if hdc.is_invalid() {
            return None;
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32), // top-down,读像素不用倒序
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() {
            let _ = DeleteObject(dib.into());
            let _ = DeleteDC(hdc);
            return None;
        }
        std::ptr::write_bytes(bits as *mut u8, 0, (size * size * 4) as usize);
        let old = SelectObject(hdc, dib.into());
        let drawn = DrawIconEx(
            hdc,
            0,
            0,
            hicon,
            size as i32,
            size as i32,
            0,
            None,
            DI_NORMAL,
        );
        let mut rgba = None;
        if drawn.is_ok() {
            let raw =
                std::slice::from_raw_parts(bits as *const u8, (size * size * 4) as usize).to_vec();
            let mut out = raw;
            for px in out.chunks_exact_mut(4) {
                px.swap(0, 2); // BGRA → RGBA
            }
            rgba = Some(IconImage::new(Arc::from(out), size, size));
        }
        SelectObject(hdc, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(hdc);
        rgba
    }
}
