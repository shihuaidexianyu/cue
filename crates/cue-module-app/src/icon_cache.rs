//! 应用图标磁盘缓存(§137):最终渲染好的 96×96 RGBA 落盘,
//! 重启后预载直接进内存,不再走提取/WinRT。
//!
//! 每枚图标一个文件:`<fnv64(icon_key)>.bin`,内容为定长头、key
//! 校验串与原始 RGBA。写走 tmp+rename(崩溃不留半个文件);任何
//! 校验失败都按未命中处理并删除文件(调用方随后走正常提取重缓存)。
//!
//! 失效判定:
//! - Win32 = exe 的 mtime + size(应用更新必然重写 exe);
//! - Packaged = 包版本号(发现时顺手取,预载比对零 WinRT);
//! - 拿不到版本号的 packaged 条目不缓存——宁可不缓存也不错缓存。
//!
//! 渲染 epoch:缓存存的是 bbox 归一化**之后**的像素,调整
//! fill_bbox / 解码策略等渲染逻辑时手动 +1,旧缓存整体失效重建。

use cue_protocol::IconImage;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAGIC: u32 = 0x4945_5543; // "CUEI"
const PARSE_VERSION: u32 = 1;
/// 渲染管线 epoch:fill_bbox / 解码 / 归一化策略变更时 +1。
const RENDER_EPOCH: u32 = 1;

const STAMP_WIN32: u32 = 1;
const STAMP_PACKAGED: u32 = 2;

/// 定长头:magic/parse/epoch/kind + stamp[3] + key_len/width/height/保留。
const HEADER_LEN: usize = 4 * 4 + 8 * 3 + 4 * 4;

/// 缓存失效指纹。Win32 用文件 mtime+size;Packaged 用包版本号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp {
    Win32 {
        mtime_secs: u64,
        mtime_nanos: u64,
        size: u64,
    },
    Packaged(u64),
}

impl Stamp {
    /// 从 exe 当前状态取指纹;metadata 失败(None)表示该条目不缓存。
    pub fn of_exe(path: &Path) -> Option<Stamp> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime = meta.modified().ok()?;
        let d = mtime.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(Stamp::Win32 {
            mtime_secs: d.as_secs(),
            mtime_nanos: d.subsec_nanos() as u64,
            size: meta.len(),
        })
    }

    fn kind(self) -> u32 {
        match self {
            Stamp::Win32 { .. } => STAMP_WIN32,
            Stamp::Packaged(_) => STAMP_PACKAGED,
        }
    }

    fn words(self) -> [u64; 3] {
        match self {
            Stamp::Win32 {
                mtime_secs,
                mtime_nanos,
                size,
            } => [mtime_secs, mtime_nanos, size],
            Stamp::Packaged(v) => [v, 0, 0],
        }
    }
}

/// 缓存文件路径:`<fnv64(key):016x>.bin`。key 本身存进文件头做
/// 碰撞校验(64 位哈希撞概率极低,但撞了会发错图标,校验串是
/// 廉价保险)。
pub fn cache_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{:016x}.bin", crate::catalog::fnv1a(key)))
}

/// 写入缓存(tmp+rename)。任何失败向上抛 io::Error,调用方记日志。
pub fn write(dir: &Path, key: &str, stamp: Stamp, icon: &IconImage) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{:016x}.tmp", crate::catalog::fnv1a(key)));
    let path = cache_path(dir, key);
    let mut buf = Vec::with_capacity(HEADER_LEN + key.len() + icon.rgba.len());
    buf.extend_from_slice(&MAGIC.to_le_bytes());
    buf.extend_from_slice(&PARSE_VERSION.to_le_bytes());
    buf.extend_from_slice(&RENDER_EPOCH.to_le_bytes());
    buf.extend_from_slice(&stamp.kind().to_le_bytes());
    for w in stamp.words() {
        buf.extend_from_slice(&w.to_le_bytes());
    }
    buf.extend_from_slice(&(key.len() as u32).to_le_bytes());
    buf.extend_from_slice(&icon.width.to_le_bytes());
    buf.extend_from_slice(&icon.height.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes()); // 保留位
    debug_assert_eq!(buf.len(), HEADER_LEN);
    buf.extend_from_slice(key.as_bytes());
    buf.extend_from_slice(&icon.rgba);
    // 写临时文件后 rename:崩溃只会留 .tmp 垃圾,不会留半个 .bin。
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&buf)?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// 读取并校验:全部校验(magic/解析版本/渲染 epoch/stamp/key/像素
/// 长度)过才返回图标。任何失败返回 None——由调用方删除文件并走
/// 正常提取重缓存。
pub fn read(dir: &Path, key: &str, stamp: Stamp) -> Option<IconImage> {
    let path = cache_path(dir, key);
    let buf = std::fs::read(&path).ok()?;
    parse(&buf, key, stamp)
}

/// 读取失败后的清理:坏文件留在盘上会每次启动都白读一遍。
pub fn remove(dir: &Path, key: &str) {
    let _ = std::fs::remove_file(cache_path(dir, key));
}

fn parse(buf: &[u8], key: &str, stamp: Stamp) -> Option<IconImage> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    let u32_at = |i: usize| -> Option<u32> {
        let off = i.checked_mul(4)?;
        Some(u32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?))
    };
    if u32_at(0)? != MAGIC || u32_at(1)? != PARSE_VERSION || u32_at(2)? != RENDER_EPOCH {
        return None;
    }
    if u32_at(3)? != stamp.kind() {
        return None;
    }
    let u64_at = |i: usize| -> Option<u64> {
        let off = 16 + i.checked_mul(8)?;
        Some(u64::from_le_bytes(buf.get(off..off + 8)?.try_into().ok()?))
    };
    for (i, w) in stamp.words().iter().enumerate() {
        if u64_at(i)? != *w {
            return None;
        }
    }
    let key_len = u32_at(10)? as usize;
    let (width, height) = (u32_at(11)?, u32_at(12)?);
    let pixel_len = width as usize * height as usize * 4;
    if width == 0 || height == 0 || pixel_len > 512 * 512 * 4 {
        return None; // 尺寸异常:不分配,直接视为坏文件
    }
    let key_off = HEADER_LEN;
    let rgba_off = key_off.checked_add(key_len)?;
    let end = rgba_off.checked_add(pixel_len)?;
    if buf.len() != end {
        return None;
    }
    if buf.get(key_off..rgba_off)? != key.as_bytes() {
        return None; // 哈希碰撞:文件属于别的 key
    }
    Some(IconImage::new(
        Arc::from(&buf[rgba_off..end]),
        width,
        height,
    ))
}

/// 包版本号打包:Major.Minor.Build.Revision(各 u16)→ u64。
pub fn pack_version(major: u16, minor: u16, build: u16, revision: u16) -> u64 {
    ((major as u64) << 48) | ((minor as u64) << 32) | ((build as u64) << 16) | revision as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(seed: u8) -> IconImage {
        IconImage::new(Arc::from(vec![seed; 96 * 96 * 4]), 96, 96)
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cue-icon-cache-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const STAMP: Stamp = Stamp::Win32 {
        mtime_secs: 123,
        mtime_nanos: 456,
        size: 789,
    };

    /// 写入→读回:同 key 同 stamp 命中,像素逐字节一致。
    #[test]
    fn roundtrip() {
        let dir = temp_dir("roundtrip");
        write(&dir, "c:\\apps\\a.exe", STAMP, &icon(7)).unwrap();
        let got = read(&dir, "c:\\apps\\a.exe", STAMP).expect("hit");
        assert_eq!((got.width, got.height), (96, 96));
        assert!(got.rgba.iter().all(|&b| b == 7));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 各类未命中:stamp 漂移 / 错 key(碰撞)/ 坏 magic / 旧 epoch /
    /// 缺文件,一律 None。
    #[test]
    fn misses() {
        let dir = temp_dir("miss");
        write(&dir, "k", STAMP, &icon(1)).unwrap();
        let stale = Stamp::Win32 {
            mtime_secs: 124, // exe 被更新过
            mtime_nanos: 456,
            size: 789,
        };
        assert!(read(&dir, "k", stale).is_none());
        assert!(read(&dir, "k", Stamp::Packaged(1)).is_none()); // kind 不符
        assert!(read(&dir, "other-key", STAMP).is_none()); // 无此文件
        // 手动伪造同哈希文件不可行,改测 parse 层的 key 校验:
        let buf = std::fs::read(cache_path(&dir, "k")).unwrap();
        assert!(parse(&buf, "different-key", STAMP).is_none());
        // 坏 magic / 旧 epoch / 截断
        let mut bad = buf.clone();
        bad[0] ^= 0xff;
        assert!(parse(&bad, "k", STAMP).is_none());
        let mut old_epoch = buf.clone();
        old_epoch[8] = 0; // RENDER_EPOCH 字段(小端首字节)回拨
        assert!(parse(&old_epoch, "k", STAMP).is_none());
        assert!(parse(&buf[..HEADER_LEN + 10], "k", STAMP).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// tmp+rename 中途崩溃只留 .tmp:不影响读取,也不被当作缓存。
    #[test]
    fn leftover_tmp_is_inert() {
        let dir = temp_dir("tmp");
        std::fs::write(dir.join(format!("{:016x}.tmp", crate::catalog::fnv1a("k"))), b"junk")
            .unwrap();
        assert!(read(&dir, "k", STAMP).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn version_packs() {
        let v = pack_version(1, 2, 3, 4);
        assert_eq!(v, 0x0001_0002_0003_0004);
        assert_eq!(pack_version(u16::MAX, 0, 0, 0), 0xffff_0000_0000_0000);
    }
}
