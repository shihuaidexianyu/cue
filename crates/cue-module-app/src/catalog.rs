//! App catalog:发现(§29)、规范化、去重(§30)后的应用入口。

use crate::pinyin_index;
use std::path::PathBuf;
use std::sync::Arc;

/// 可启动的应用入口。稳定身份 = item_key(§51)。
#[derive(Clone, Debug)]
pub struct AppEntry {
    pub name: Arc<str>,
    pub name_lower: Arc<str>,
    /// 全拼键(§27):"永劫无间" → "yongjiewujian"。
    pub pinyin_full: Arc<str>,
    /// 首字母键:"yjwj"。
    pub pinyin_initials: Arc<str>,
    pub target: LaunchTarget,
    /// §51:Packaged = AUMID;Win32 = canonical exe + normalized args。
    pub item_key: Arc<str>,
}

#[derive(Clone, Debug)]
pub enum LaunchTarget {
    Win32 {
        exe: PathBuf,
        args: Arc<str>,
        working_dir: Option<PathBuf>,
    },
    Packaged {
        aumid: Arc<str>,
    },
}

impl AppEntry {
    /// 构造时完成 normalize:小写键、拼音键、item_key(§51)。
    pub fn new(name: &str, target: LaunchTarget) -> Self {
        let (full, initials) = pinyin_index::keys(name);
        let item_key: Arc<str> = match &target {
            LaunchTarget::Packaged { aumid } => aumid.clone(),
            LaunchTarget::Win32 { exe, args, .. } => format!(
                "{}\u{1f}{}",
                exe.to_string_lossy().to_lowercase(),
                normalize_args(args)
            )
            .into(),
        };
        Self {
            name: name.into(),
            name_lower: name.to_lowercase().into(),
            pinyin_full: full.into(),
            pinyin_initials: initials.into(),
            target,
            item_key,
        }
    }

    /// 行标识:PresentationInvalidated 寻址(§109)与 ResultState 行 id。
    /// 由 item_key 派生,跨 query 稳定。
    pub fn item_id(&self) -> u64 {
        fnv1a(&self.item_key)
    }

    /// 图标缓存 key:Win32 = exe 路径;Packaged = AUMID。
    pub fn icon_key(&self) -> Arc<str> {
        match &self.target {
            LaunchTarget::Win32 { exe, .. } => exe.to_string_lossy().into_owned().into(),
            LaunchTarget::Packaged { aumid } => aumid.clone(),
        }
    }
}

/// §30 的 "normalized arguments":折叠空白 + 小写。
fn normalize_args(args: &str) -> String {
    args.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// FNV-1a 64。不需要加密强度,需要稳定(同输入同输出)。
fn fnv1a(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// §30:只按 launch semantics(item_key)去重;宁可重复,不要 aggressive。
pub fn dedup(entries: &mut Vec<AppEntry>) {
    let mut seen = std::collections::HashSet::new();
    entries.retain(|e| seen.insert(e.item_key.clone()));
}
