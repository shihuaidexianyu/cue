//! 图标管线:异步提取,Arc 复用,完成后推
//! `PresentationInvalidated` 让 Core 重跑可见行的 present()。
//! Win32 提取由 sakana-util-win::icon 提供;Packaged logo 走 WinRT
//! `AppDisplayInfo.GetLogo`(§134)——发现线程把 AUMID → AppListEntry
//! 索引在发布 catalog 前填入(`set_packaged_index`):query 只能看到
//! 已发布 catalog 的条目,Packaged 图标请求到达时索引必然就绪,
//! 枚举不重复、worker 不等待。
//!
//! §137:最终像素落盘(`icon_cache`),catalog 发布前预载——重启后
//! 首屏图标直接命中内存,不经历"占位字形 → 逐个蹦出"。worker 提取
//! 成功后写穿到磁盘。失效:Win32 = exe mtime+size;Packaged = 包
//! 版本号。读失败的坏/旧文件即删(随后正常提取重缓存);不在当前
//! catalog 的缓存文件一律保留(packaged 发现可能整批失败,误删
//! 会让缓存比没有更脆弱)。
//!
//! 线程模型:module 自有 worker 线程串行提取,负缓存防重试风暴。

use crate::catalog::{AppEntry, LaunchTarget};
use crate::icon_cache::{self, Stamp};
use sakana_protocol::{
    IconImage, ItemId, LogLevel, ModuleEvent, ModuleEventSink, ModuleLogger, ResultIcon,
};
use sakana_util_win::com::ComGuard;
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
    /// AUMID → 打包包版本号(§137 磁盘缓存失效指纹)。
    packaged_versions: Arc<Mutex<HashMap<String, u64>>>,
    /// 磁盘缓存目录(modules/app/cache/icons);None = 纯内存(测试)。
    cache_dir: Option<PathBuf>,
    logger: ModuleLogger,
    tx: Option<Sender<Request>>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

/// worker 的共享上下文(打包成结构,避免参数串过长)。
struct WorkerCtx {
    cache: Arc<Mutex<HashMap<String, Slot>>>,
    packaged_index: Arc<Mutex<HashMap<String, AppListEntry>>>,
    packaged_versions: Arc<Mutex<HashMap<String, u64>>>,
    cache_dir: Option<PathBuf>,
    sink: ModuleEventSink,
    logger: ModuleLogger,
    shutdown: Arc<AtomicBool>,
}

impl IconPipeline {
    pub fn new(sink: ModuleEventSink, cache_dir: Option<PathBuf>, logger: ModuleLogger) -> Self {
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let packaged_index = Arc::new(Mutex::new(HashMap::new()));
        let packaged_versions = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = channel::<Request>();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker = {
            let ctx = WorkerCtx {
                cache: Arc::clone(&cache),
                packaged_index: Arc::clone(&packaged_index),
                packaged_versions: Arc::clone(&packaged_versions),
                cache_dir: cache_dir.clone(),
                sink,
                logger: logger.clone(),
                shutdown: Arc::clone(&shutdown),
            };
            std::thread::spawn(move || worker_loop(rx, ctx))
        };
        Self {
            cache,
            packaged_index,
            packaged_versions,
            cache_dir,
            logger,
            tx: Some(tx),
            shutdown,
            worker: Some(worker),
        }
    }

    /// 发现线程在发布 catalog 前调用(§134)。只在启动时写一次,
    /// 之后 worker 只读;锁在请求粒度上持有,无热路径竞争。
    pub fn set_packaged_index(
        &self,
        index: HashMap<String, AppListEntry>,
        versions: HashMap<String, u64>,
    ) {
        *self.packaged_index.lock().unwrap() = index;
        *self.packaged_versions.lock().unwrap() = versions;
    }

    /// §137:catalog 发布前预载磁盘缓存——首个查询即可见全部缓存
    /// 图标,不再有占位蹦出。只插缺失键(防御:与 worker 在途的
    /// Pending 不冲突);读失败的坏/旧文件删除(正常提取会重缓存),
    /// 不在 catalog 的文件不碰(发现可能整批失败,误删更糟)。
    /// 在发现线程上跑,几百枚小文件 + 每枚一次 stat,亚秒级。
    pub fn preload_from_cache(&self, entries: &[AppEntry]) {
        let Some(dir) = &self.cache_dir else {
            return;
        };
        let started = std::time::Instant::now();
        let mut hits = 0u32;
        for e in entries {
            let key = e.icon_key();
            if self.cache.lock().unwrap().contains_key(key.as_ref()) {
                continue;
            }
            let stamp = match &e.target {
                LaunchTarget::Win32 { exe, .. } => Stamp::of_exe(exe),
                LaunchTarget::Packaged { aumid } => self
                    .packaged_versions
                    .lock()
                    .unwrap()
                    .get(aumid.as_ref())
                    .copied()
                    .map(Stamp::Packaged),
            };
            // 拿不到指纹(元数据/版本缺失)的条目不缓存,现用现提。
            let Some(stamp) = stamp else { continue };
            match icon_cache::read(dir, &key, stamp) {
                Some(icon) => {
                    self.cache
                        .lock()
                        .unwrap()
                        .insert(key.to_string(), Slot::Ready(Arc::new(icon)));
                    hits += 1;
                }
                None => icon_cache::remove(dir, &key),
            }
        }
        self.logger.log(
            LogLevel::Info,
            &format!(
                "app icon cache preload: {hits}/{} hits in {:?}",
                entries.len(),
                started.elapsed()
            ),
        );
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

#[cfg(test)]
impl IconPipeline {
    fn slot_is_ready(&self, key: &str) -> bool {
        matches!(self.cache.lock().unwrap().get(key), Some(Slot::Ready(_)))
    }

    fn insert_pending_for_test(&self, key: &str) {
        self.cache
            .lock()
            .unwrap()
            .insert(key.to_string(), Slot::Pending(vec![ItemId(1)]));
    }
}

fn worker_loop(rx: Receiver<Request>, ctx: WorkerCtx) {
    let _com = ComGuard::new();
    while let Ok(req) = rx.recv() {
        if ctx.shutdown.load(Ordering::SeqCst) {
            break;
        }
        let icon = match &req.source {
            OwnedSource::Exe(exe) => sakana_util_win::icon::extract_file_icon(exe),
            OwnedSource::Packaged(aumid) => {
                packaged_logo(&ctx.packaged_index.lock().unwrap(), aumid)
            }
        };
        // §137:提取成功即写穿磁盘缓存(重启后预载直接命中);
        // 拿不到失效指纹的不缓存。写失败只记日志——缓存是优化,
        // 不该影响在途展示。
        if let (Some(dir), Some(icon)) = (&ctx.cache_dir, &icon) {
            let stamp = match &req.source {
                OwnedSource::Exe(exe) => Stamp::of_exe(exe),
                OwnedSource::Packaged(aumid) => ctx
                    .packaged_versions
                    .lock()
                    .unwrap()
                    .get(aumid.as_str())
                    .copied()
                    .map(Stamp::Packaged),
            };
            if let Some(stamp) = stamp
                && let Err(e) = icon_cache::write(dir, &req.key, stamp, icon)
            {
                ctx.logger.log(
                    LogLevel::Warn,
                    &format!("app icon cache write failed for {}: {e}", req.key),
                );
            }
        }
        let mut ready = Vec::new();
        {
            let mut cache = ctx.cache.lock().unwrap();
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
            ctx.sink
                .send(ModuleEvent::PresentationInvalidated { items: ready });
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
    const SIZE: u32 = sakana_util_win::icon::ICON_SIZE;
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
    use super::*;
    use crate::{app_paths, catalog, packaged, start_menu};
    use sakana_protocol::{LogLevel, ModuleLog};
    use std::path::Path;

    struct TestLog;
    impl ModuleLog for TestLog {
        fn log(&self, _level: LogLevel, _message: &str) {}
    }

    struct TestSink;
    impl sakana_protocol::ModuleEventSend for TestSink {
        fn send(&self, _event: sakana_protocol::ModuleEvent) {}
    }

    /// §137 预载:指纹一致的缓存文件直接 Ready;已在途的 Pending
    /// 槽位不被覆盖(失效广播留给 worker);stamp 不符的旧文件删除,
    /// 条目留待正常提取重缓存。
    #[test]
    fn preload_from_disk_cache() {
        let dir = std::env::temp_dir().join(format!("sakana-app-preload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pipeline = IconPipeline::new(
            std::sync::Arc::new(TestSink),
            Some(dir.clone()),
            std::sync::Arc::new(TestLog),
        );
        let icon = IconImage::new(std::sync::Arc::from(vec![7u8; 96 * 96 * 4]), 96, 96);
        let mk = |name: &str, content: &[u8]| {
            let exe = dir.join(name);
            std::fs::write(&exe, content).unwrap();
            catalog::AppEntry::new(
                name,
                catalog::LaunchTarget::Win32 {
                    exe,
                    args: "".into(),
                    working_dir: None,
                },
            )
        };
        let stamp_of = |e: &catalog::AppEntry| match &e.target {
            catalog::LaunchTarget::Win32 { exe, .. } => Stamp::of_exe(exe).unwrap(),
            _ => unreachable!(),
        };

        // 1. 命中:缓存文件与 exe 指纹一致 → Ready
        let a = mk("a.exe", b"x");
        let key_a = a.icon_key().to_string();
        icon_cache::write(&dir, &key_a, stamp_of(&a), &icon).unwrap();
        pipeline.preload_from_cache(&[a]);
        assert!(pipeline.slot_is_ready(&key_a));

        // 2. 在途 Pending 不被预载覆盖
        let b = mk("b.exe", b"yy");
        let key_b = b.icon_key().to_string();
        pipeline.insert_pending_for_test(&key_b);
        icon_cache::write(&dir, &key_b, stamp_of(&b), &icon).unwrap();
        pipeline.preload_from_cache(&[b]);
        assert!(!pipeline.slot_is_ready(&key_b));

        // 3. stamp 漂移(exe 更新,size 变)→ 不命中,旧文件被删
        let c = mk("c.exe", b"zzz");
        let key_c = c.icon_key().to_string();
        icon_cache::write(&dir, &key_c, stamp_of(&c), &icon).unwrap();
        std::fs::write(
            match &c.target {
                catalog::LaunchTarget::Win32 { exe, .. } => exe,
                _ => unreachable!(),
            },
            b"zzzz-longer",
        )
        .unwrap();
        pipeline.preload_from_cache(std::slice::from_ref(&c));
        assert!(!pipeline.slot_is_ready(&key_c));
        assert!(!icon_cache::cache_path(&dir, &key_c).exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 离线审计(诊断工具,非常规测试):对全量 catalog 跑提取管线,
    /// 按 alpha 分布分类并落 PNG 样本到 target/icon-audit/。
    /// 运行:cargo test -p sakana-module-app icon_audit -- --ignored --nocapture
    #[test]
    #[ignore]
    fn icon_audit() {
        let logger: sakana_protocol::ModuleLogger = std::sync::Arc::new(TestLog);
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
            let Some(icon) = sakana_util_win::icon::extract_file_icon(exe) else {
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
            let icon = sakana_util_win::icon::extract_file_icon(Path::new(exe)).unwrap();
            let rgba: Vec<u8> = icon.rgba.to_vec();
            let img = image::RgbaImage::from_raw(icon.width, icon.height, rgba).unwrap();
            img.save(out_dir.join(format!("tiny-{i}.png"))).unwrap();
        }
    }

    /// §134 审计:对全量 packaged 条目跑 GetLogo 提取,统计成功率
    /// 并落前 24 枚样本 PNG 供肉眼验收。
    /// 运行:cargo test -p sakana-module-app packaged_logo_audit -- --ignored --nocapture
    #[test]
    #[ignore]
    fn packaged_logo_audit() {
        // GetLogo/OpenReadAsync 需要 COM 套间(生产侧在 worker 的 ComGuard 内)。
        let _com = sakana_util_win::com::ComGuard::new();
        let logger: sakana_protocol::ModuleLogger = std::sync::Arc::new(TestLog);
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
