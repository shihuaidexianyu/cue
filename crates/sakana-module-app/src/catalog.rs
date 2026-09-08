//! App catalog:发现、规范化、去重后的应用入口。

use crate::pinyin_index;
use std::path::PathBuf;
use std::sync::Arc;

/// 可启动的应用入口。稳定身份 = item_key。
#[derive(Clone, Debug)]
pub struct AppEntry {
    pub name: Arc<str>,
    pub name_lower: Arc<str>,
    /// 全拼键:"永劫无间" → "yongjiewujian"。
    pub pinyin_full: Arc<str>,
    /// 首字母键:"yjwj"。
    pub pinyin_initials: Arc<str>,
    pub target: LaunchTarget,
    /// Packaged = AUMID;Win32 = exe + 原始参数 + 工作目录。
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
    /// 构造时完成 normalize:小写键、拼音键、item_key。
    pub fn new(name: &str, target: LaunchTarget) -> Self {
        let (full, initials) = pinyin_index::keys(name);
        let item_key: Arc<str> = match &target {
            LaunchTarget::Packaged { aumid } => aumid.clone(),
            LaunchTarget::Win32 {
                exe,
                args,
                working_dir,
            } => {
                let mut key = format!("{}\u{1f}{}", exe.to_string_lossy().to_lowercase(), args);
                // 无工作目录且参数未被旧规则改变的入口保留原 usage key。
                if let Some(dir) = working_dir {
                    key.push('\u{1f}');
                    key.push_str(&dir.to_string_lossy().to_lowercase());
                }
                key.into()
            }
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

    /// 行标识:PresentationInvalidated 寻址与 ResultState 行 id。
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

/// FNV-1a 64。不需要加密强度,需要稳定(同输入同输出)。
/// item_id 与图标缓存文件名(§137)共用。
pub(crate) fn fnv1a(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 只按 launch semantics(item_key)去重;宁可重复,不要 aggressive。
pub fn dedup(entries: &mut Vec<AppEntry>) {
    let mut seen = std::collections::HashSet::new();
    entries.retain(|e| seen.insert(e.item_key.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_preserves_argument_and_working_directory_semantics() {
        let make = |args: &str, dir: &str| {
            AppEntry::new(
                "app",
                LaunchTarget::Win32 {
                    exe: r"C:\Apps\app.exe".into(),
                    args: args.into(),
                    working_dir: Some(dir.into()),
                },
            )
        };
        let mut entries = vec![
            make("https://example.com/A", "C:\\one"),
            make("https://example.com/a", "C:\\one"),
            make("\"a  b\"", "C:\\one"),
            make("\"a b\"", "C:\\one"),
            make("\"a b\"", "C:\\two"),
            make("\"a b\"", "C:\\two"),
        ];
        dedup(&mut entries);
        assert_eq!(entries.len(), 5);
    }
}
