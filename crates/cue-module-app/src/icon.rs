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
use std::path::{Path, PathBuf};
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
fn extract_icon(exe: &Path) -> Option<IconImage> {
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
        // 失败路径必须先 DeleteDC——CreateCompatibleDC 已成功,
        // 直接 `?` 返回会泄漏这个 GDI DC。
        let dib = match CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(dib) => dib,
            Err(_) => {
                let _ = DeleteDC(hdc);
                return None;
            }
        };
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
            normalize_bbox(&mut out, size);
            rgba = Some(IconImage::new(Arc::from(out), size, size));
        }
        SelectObject(hdc, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(hdc);
        rgba
    }
}

/// 老式应用的 JUMBO image list 单元是"原尺寸贴在左上角的 256px
/// 大图"(shell 不上采样),直接缩放会得到缩在角落的小图标。
/// 用 alpha 包围盒检测这种异常:内容宽或高不足画布一半时,
/// 裁出内容双线性放大并居中。正常图标(包围盒 ≈ 全画布)不动,
/// 避免把自带留边的图标放大得大小不一。
fn normalize_bbox(rgba: &mut Vec<u8>, size: u32) {
    let s = size as usize;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (s, s, 0usize, 0usize);
    for y in 0..s {
        for x in 0..s {
            if rgba[(y * s + x) * 4 + 3] > 16 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if max_x < min_x || max_y < min_y {
        return; // 全透明:无可归一化
    }
    let (bw, bh) = (max_x - min_x + 1, max_y - min_y + 1);
    if bw * 2 >= s && bh * 2 >= s {
        return; // 内容 ≥ 半画布:正常图标
    }
    // 等比放大到尽量填满画布,居中
    let scale = (s as f32 / bw as f32).min(s as f32 / bh as f32);
    let (dw, dh) = ((bw as f32 * scale) as usize, (bh as f32 * scale) as usize);
    let (ox, oy) = ((s - dw) / 2, (s - dh) / 2);
    let src = rgba.clone();
    let mut dst = vec![0u8; s * s * 4];
    for dy in 0..dh {
        for dx in 0..dw {
            // 目标像素中心映射回源内容坐标(双线性)
            let fx = (dx as f32 + 0.5) / scale - 0.5;
            let fy = (dy as f32 + 0.5) / scale - 0.5;
            let (x0, y0) = (fx.floor().max(0.0) as usize, fy.floor().max(0.0) as usize);
            let (x1, y1) = ((x0 + 1).min(bw - 1), (y0 + 1).min(bh - 1));
            let (tx, ty) = (fx.fract().max(0.0), fy.fract().max(0.0));
            let mut px = [0f32; 4];
            for (sy, wy) in [(y0, 1.0 - ty), (y1, ty)] {
                for (sx, wx) in [(x0, 1.0 - tx), (x1, tx)] {
                    let i = ((min_y + sy) * s + (min_x + sx)) * 4;
                    let w = wx * wy;
                    for c in 0..4 {
                        px[c] += src[i + c] as f32 * w;
                    }
                }
            }
            let d = ((oy + dy) * s + (ox + dx)) * 4;
            for c in 0..4 {
                dst[d + c] = px[c].round() as u8;
            }
        }
    }
    *rgba = dst;
}

#[cfg(test)]
mod tests {
    use super::*;
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
            let Some(icon) = extract_icon(exe) else {
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
            let icon = extract_icon(Path::new(exe)).unwrap();
            let rgba: Vec<u8> = icon.rgba.to_vec();
            let img = image::RgbaImage::from_raw(icon.width, icon.height, rgba).unwrap();
            img.save(out_dir.join(format!("tiny-{i}.png"))).unwrap();
        }
    }
}
