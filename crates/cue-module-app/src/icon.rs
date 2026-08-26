//! 图标管线:异步提取,Arc 复用,完成后推
//! `PresentationInvalidated` 让 Core 重跑可见行的 present()。
//! Win32 提取由 cue-util-win::icon 提供;Packaged logo 走 WinRT
//! `AppDisplayInfo.GetLogo`(§134)——发现线程把 AUMID → AppListEntry
//! 索引在发布 catalog 前填入(`set_packaged_index`):query 只能看到
//! 已发布 catalog 的条目,Packaged 图标请求到达时索引必然就绪,
//! 枚举不重复、worker 不等待。
//!
//! 线程模型:module 自有 worker 线程串行提取,负缓存防重试风暴。

use cue_protocol::{IconImage, ItemId, ModuleEvent, ModuleEventSink, ResultIcon};
use cue_util_win::com::ComGuard;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use windows::ApplicationModel::Core::AppListEntry;
use windows::Foundation::Size;
use windows::Storage::Streams::{
    Buffer, DataReader, IRandomAccessStreamWithContentType, InputStreamOptions,
};

enum Slot {
    /// 提取在途 + 等它的行。同一 key 可被多行共享(dedup 偏好
    /// 保留重复项,同 exe 的行很常见):每行都登记进等待名单,
    /// 完成时整单失效重画——只记首行的话兄弟行永远停在空槽。
    Pending(Vec<ItemId>),
    Failed,
    Ready(Arc<IconImage>),
}

/// 图标来源:Win32 按 exe 路径提系统图标;Packaged 按 AUMID 查
/// AppListEntry 取 logo(§134)。
pub enum IconSource<'a> {
    Exe(&'a Path),
    Packaged(&'a str),
}

enum OwnedSource {
    Exe(PathBuf),
    Packaged(String),
}

struct Request {
    key: String,
    source: OwnedSource,
}

pub struct IconPipeline {
    cache: Arc<Mutex<HashMap<String, Slot>>>,
    packaged_index: Arc<Mutex<HashMap<String, AppListEntry>>>,
    tx: Option<Sender<Request>>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl IconPipeline {
    pub fn new(sink: ModuleEventSink) -> Self {
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let packaged_index = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = channel::<Request>();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker = {
            let cache = Arc::clone(&cache);
            let packaged_index = Arc::clone(&packaged_index);
            let shutdown = Arc::clone(&shutdown);
            std::thread::spawn(move || worker_loop(rx, cache, packaged_index, sink, shutdown))
        };
        Self {
            cache,
            packaged_index,
            tx: Some(tx),
            shutdown,
            worker: Some(worker),
        }
    }

    /// 发现线程在发布 catalog 前调用(§134)。只在启动时写一次,
    /// 之后 worker 只读;锁在请求粒度上持有,无热路径竞争。
    pub fn set_packaged_index(&self, index: HashMap<String, AppListEntry>) {
        *self.packaged_index.lock().unwrap() = index;
    }

    /// present() 热路径(< 1 ms,无 IO):命中返回缓存图标;
    /// 未命中登记 Pending 并投递提取请求,本帧返回 None(留空槽位)。
    pub fn get_or_queue(
        &self,
        item_id: ItemId,
        key: &str,
        source: IconSource<'_>,
    ) -> Option<ResultIcon> {
        let mut cache = self.cache.lock().unwrap();
        match cache.get_mut(key) {
            // IconImage 内 rgba 是 Arc<[u8]>,clone 保持指针不变——
            // UI 按该指针缓存纹理。
            Some(Slot::Ready(icon)) => Some(ResultIcon::Raster((**icon).clone())),
            Some(Slot::Pending(waiting)) => {
                waiting.push(item_id);
                None
            }
            Some(Slot::Failed) => None,
            None => {
                cache.insert(key.to_string(), Slot::Pending(vec![item_id]));
                if let Some(tx) = &self.tx {
                    let owned = match source {
                        IconSource::Exe(exe) => OwnedSource::Exe(exe.to_path_buf()),
                        IconSource::Packaged(aumid) => OwnedSource::Packaged(aumid.to_string()),
                    };
                    let _ = tx.send(Request {
                        key: key.to_string(),
                        source: owned,
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
    packaged_index: Arc<Mutex<HashMap<String, AppListEntry>>>,
    sink: ModuleEventSink,
    shutdown: Arc<AtomicBool>,
) {
    let _com = ComGuard::new();
    while let Ok(req) = rx.recv() {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let icon = match &req.source {
            OwnedSource::Exe(exe) => cue_util_win::icon::extract_file_icon(exe),
            OwnedSource::Packaged(aumid) => packaged_logo(&packaged_index.lock().unwrap(), aumid),
        };
        let mut ready = Vec::new();
        {
            let mut cache = cache.lock().unwrap();
            match icon {
                Some(icon) => {
                    // insert 返回旧槽:等待名单整单取出,广播失效。
                    if let Some(Slot::Pending(waiting)) =
                        cache.insert(req.key, Slot::Ready(Arc::new(icon)))
                    {
                        ready = waiting;
                    }
                }
                // 负缓存:失败不重试(图标缺失不是致命问题)
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

/// Packaged 图标:AUMID → AppListEntry → DisplayInfo.GetLogo(96)
/// → 流 → 解码 → 96×96 RGBA(straight alpha 契约)。
fn packaged_logo(index: &HashMap<String, AppListEntry>, aumid: &str) -> Option<IconImage> {
    let entry = index.get(aumid)?;
    let logo = entry
        .DisplayInfo()
        .and_then(|d| {
            d.GetLogo(Size {
                Width: 96.0,
                Height: 96.0,
            })
        })
        .ok()?;
    let stream = logo.OpenReadAsync().ok()?.join().ok()?;
    let bytes = read_stream(&stream).ok()?;
    decode_to_icon(&bytes)
}

fn read_stream(stream: &IRandomAccessStreamWithContentType) -> windows::core::Result<Vec<u8>> {
    let size = stream.Size()?;
    let mut out = Vec::with_capacity(size as usize);
    let mut offset = 0u64;
    while offset < size {
        let want = (size - offset).min(1 << 20) as u32;
        let buffer = Buffer::Create(want)?;
        let filled = stream
            .ReadAsync(&buffer, want, InputStreamOptions::None)?
            .join()?;
        let n = filled.Length()? as usize;
        if n == 0 {
            break;
        }
        let reader = DataReader::FromBuffer(&filled)?;
        let mut chunk = vec![0u8; n];
        reader.ReadBytes(&mut chunk)?;
        out.extend_from_slice(&chunk);
        offset += n as u64;
    }
    Ok(out)
}

fn decode_to_icon(bytes: &[u8]) -> Option<IconImage> {
    const SIZE: u32 = cue_util_win::icon::ICON_SIZE;
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = if img.width() == SIZE && img.height() == SIZE {
        img.to_rgba8()
    } else {
        image::imageops::resize(&img, SIZE, SIZE, image::imageops::FilterType::Lanczos3)
    };
    let mut bytes = rgba.into_vec();
    fill_bbox(&mut bytes, SIZE);
    Some(IconImage::new(Arc::from(bytes), SIZE, SIZE))
}

/// UWP small logo 为开始菜单磁贴设计,自带一圈透明留边——直接进
/// 96px 画布,行内视觉只有 ~55%,比 Win32 exe 图标小一圈。把内容
/// 包围盒放大到画布 80%(封顶 2×,防极小内容被拉爆);满幅 logo
/// (plate 类)不动。与 util-win 的 normalize_bbox 策略不同(那边
/// 只救 <50% 的角落图标、填满 100%),按 §72 暂存为第二份实现。
fn fill_bbox(rgba: &mut Vec<u8>, size: u32) {
    const TARGET_FILL: f32 = 0.80;
    const MAX_SCALE: f32 = 2.0;
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
    let fill = (bw.max(bh) as f32) / s as f32;
    if fill >= TARGET_FILL {
        return; // 已经够满
    }
    let scale = (TARGET_FILL / fill).min(MAX_SCALE);
    let (dw, dh) = (
        ((bw as f32 * scale) as usize).min(s),
        ((bh as f32 * scale) as usize).min(s),
    );
    let (ox, oy) = ((s - dw) / 2, (s - dh) / 2);
    let sx = bw as f32 / dw as f32;
    let sy = bh as f32 / dh as f32;
    let src = rgba.clone();
    let mut dst = vec![0u8; s * s * 4];
    for dy in 0..dh {
        for dx in 0..dw {
            // 目标像素中心映射回源内容坐标(双线性)
            let fx = (dx as f32 + 0.5) * sx - 0.5;
            let fy = (dy as f32 + 0.5) * sy - 0.5;
            let (x0, y0) = (fx.floor().max(0.0) as usize, fy.floor().max(0.0) as usize);
            let (x1, y1) = ((x0 + 1).min(bw - 1), (y0 + 1).min(bh - 1));
            let (tx, ty) = (fx.fract().max(0.0), fy.fract().max(0.0));
            let mut px = [0f32; 4];
            for (ry, wy) in [(y0, 1.0 - ty), (y1, ty)] {
                for (rx, wx) in [(x0, 1.0 - tx), (x1, tx)] {
                    let i = ((min_y + ry) * s + (min_x + rx)) * 4;
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
    use crate::{app_paths, catalog, packaged, start_menu};
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
        let d = packaged::discover(&logger);
        entries.extend(d.entries);
        entries.extend(app_paths::discover(&logger));
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
            let transparent = icon
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|px| px[3] == 0)
                .count();
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
        println!(
            "== icon audit: {ok} ok, {} all-alpha-zero, {} >95% transparent, {extract_failed} extract failed ==",
            zero_alpha.len(),
            partial.len()
        );
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

    /// §134 审计:对全量 packaged 条目跑 GetLogo 提取,统计成功率
    /// 并落前 24 枚样本 PNG 供肉眼验收。
    /// 运行:cargo test -p cue-module-app packaged_logo_audit -- --ignored --nocapture
    #[test]
    #[ignore]
    fn packaged_logo_audit() {
        // GetLogo/OpenReadAsync 需要 COM 套间(生产侧在 worker 的 ComGuard 内)。
        let _com = cue_util_win::com::ComGuard::new();
        let logger: cue_protocol::ModuleLogger = std::sync::Arc::new(TestLog);
        let d = packaged::discover(&logger);
        let mut ok = 0u32;
        let mut failed: Vec<String> = Vec::new();
        let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/icon-audit");
        std::fs::create_dir_all(&out_dir).unwrap();
        for (aumid, entry) in &d.logo_index {
            let icon = entry
                .DisplayInfo()
                .and_then(|di| {
                    di.GetLogo(windows::Foundation::Size {
                        Width: 96.0,
                        Height: 96.0,
                    })
                })
                .ok()
                .and_then(|logo| logo.OpenReadAsync().ok()?.join().ok())
                .and_then(|stream| super::read_stream(&stream).ok())
                .and_then(|bytes| super::decode_to_icon(&bytes));
            match icon {
                Some(icon) => {
                    ok += 1;
                    if ok <= 24 {
                        let img =
                            image::RgbaImage::from_raw(icon.width, icon.height, icon.rgba.to_vec())
                                .unwrap();
                        img.save(out_dir.join(format!("packaged-{ok:02}.png")))
                            .unwrap();
                    }
                }
                None => failed.push(aumid.clone()),
            }
        }
        println!(
            "== packaged logo audit: {ok} ok, {} failed of {} ==",
            failed.len(),
            d.logo_index.len()
        );
        for f in failed.iter().take(20) {
            println!("LOGO-FAILED: {f}");
        }
    }
}
