//! 书签 catalog(§117):条目模型 + mtime 指纹刷新。
//!
//! 刷新纪律:无 watcher(§56 精神)——每次查询在模块后台 future 里
//! 重跑发现 + stat,指纹(路径, mtime, 长度)变了才重解析。46 KB
//! 量级 JSON 解析亚毫秒,热路径(UI/唤醒)零 IO。

use crate::chromium::{self, Browser};
use crate::pinyin_index;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct BookmarkEntry {
    pub title: Arc<str>,
    /// 排序键(§27 同款:name 小写)。
    pub title_lower: Arc<str>,
    pub pinyin_full: String,
    pub pinyin_initials: String,
    pub url: Arc<str>,
    pub domain: Arc<str>,
    pub browser: Browser,
    /// 非 Default profile 的目录名;Default 为空串(不标注)。
    pub profile: Arc<str>,
    /// §51 usage 身份:`{browser}:{url}`——从哪来回哪开之后,不同来源
    /// 浏览器是不同启动动作,usage 分开计。
    pub item_key: Arc<str>,
    id: u64,
}

impl BookmarkEntry {
    pub fn item_id(&self) -> u64 {
        self.id
    }
}

/// (路径, mtime, 长度) 指纹:集合不同即重读。Chrome/Edge 原子写
/// (tmp+rename),mtime 必然变化。
type Fingerprints = Vec<(PathBuf, Option<SystemTime>, u64)>;

pub struct CatalogCache {
    state: Mutex<State>,
}

struct State {
    entries: Vec<Arc<BookmarkEntry>>,
    fingerprints: Fingerprints,
}

impl CatalogCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                entries: Vec::new(),
                fingerprints: Vec::new(),
            }),
        })
    }

    /// 发现 + stat;指纹不同则重解析全部(解析极廉价,不做增量)。
    /// 单个文件损坏跳过该文件(§63),其余照常。
    pub fn refresh_if_changed(&self) {
        let files = chromium::discover_files();
        let fingerprints: Fingerprints = files
            .iter()
            .map(|f| {
                let meta = std::fs::metadata(&f.path).ok();
                (
                    f.path.clone(),
                    meta.as_ref().and_then(|m| m.modified().ok()),
                    meta.map(|m| m.len()).unwrap_or(0),
                )
            })
            .collect();
        {
            let state = self.state.lock().unwrap();
            if state.fingerprints == fingerprints && !state.entries.is_empty() {
                return;
            }
        }
        let mut entries = Vec::new();
        let mut seen_ids = HashSet::new();
        for file in &files {
            let Ok(json) = std::fs::read_to_string(&file.path) else {
                continue; // 读不到就跳过(浏览器正在写/权限),下轮再试
            };
            for (title, url) in chromium::parse_bookmarks(&json) {
                entries.push(make_entry(file, title, url, &mut seen_ids));
            }
        }
        entries.sort_by(|a, b| a.title_lower.cmp(&b.title_lower));
        let mut state = self.state.lock().unwrap();
        state.entries = entries;
        state.fingerprints = fingerprints;
    }

    pub fn entries(&self) -> Vec<Arc<BookmarkEntry>> {
        self.state.lock().unwrap().entries.clone()
    }
}

fn make_entry(
    file: &chromium::BookmarkFile,
    title: String,
    url: String,
    seen_ids: &mut HashSet<u64>,
) -> Arc<BookmarkEntry> {
    let (pinyin_full, pinyin_initials) = pinyin_index::keys(&title);
    // ItemId:hash(browser, profile, url);同 browser+profile 下重复收藏
    // 同一 URL(不同文件夹)时盐值递进去,保证目录内唯一。
    let mut salt = 0u64;
    let id = loop {
        let id = hash_id(file.browser, &file.profile, &url, salt);
        if seen_ids.insert(id) {
            break id;
        }
        salt += 1;
    };
    let domain = chromium::domain_of(&url);
    Arc::new(BookmarkEntry {
        title_lower: title.to_lowercase().into(),
        title: title.into(),
        pinyin_full,
        pinyin_initials,
        domain: domain.into(),
        item_key: format!("{}:{url}", file.browser.key()).into(),
        url: url.into(),
        browser: file.browser,
        profile: file.profile.as_str().into(),
        id,
    })
}

fn hash_id(browser: Browser, profile: &str, url: &str, salt: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (browser, profile, url, salt).hash(&mut h);
    h.finish()
}

/// 测试用构造器(lib.rs 的搜索/排序测试需要直接造条目)。
#[cfg(test)]
pub(crate) fn test_entry(title: &str, url: &str, browser: Browser) -> Arc<BookmarkEntry> {
    let file = chromium::BookmarkFile {
        browser,
        profile: String::new(),
        path: PathBuf::new(),
    };
    make_entry(
        &file,
        title.to_string(),
        url.to_string(),
        &mut HashSet::new(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_bookmarks(dir: &std::path::Path, name: &str, url: &str) {
        let profile = dir.join("Default");
        std::fs::create_dir_all(&profile).unwrap();
        let json = format!(
            r#"{{"roots":{{"bookmark_bar":{{"type":"folder","children":[{{"type":"url","name":"{name}","url":"{url}"}}]}}}}}}"#
        );
        std::fs::File::create(profile.join("Bookmarks"))
            .unwrap()
            .write_all(json.as_bytes())
            .unwrap();
    }

    /// 指纹变化驱动重读:同一文件改写内容(保证 mtime 前进)后,
    /// 新条目必须出现。
    #[test]
    fn refresh_picks_up_changes() {
        // discover_files 走 %LOCALAPPDATA%,这里用指纹比较的等价物:
        // 直接构造 cache 状态机不可行(发现是环境耦合的),所以测试
        // 落在 parse + 指纹语义上:重写文件 → metadata 变化 → 重解析。
        let tmp = std::env::temp_dir().join(format!("cue-bm-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        write_bookmarks(&tmp, "甲", "https://a.example/");
        let path = tmp.join("Default").join("Bookmarks");
        let fp1 = std::fs::metadata(&path)
            .map(|m| (m.modified().ok(), m.len()))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_bookmarks(&tmp, "乙", "https://b.example/");
        let fp2 = std::fs::metadata(&path)
            .map(|m| (m.modified().ok(), m.len()))
            .unwrap();
        assert_ne!(fp1, fp2, "重写后指纹必须变化");
        let items = chromium::parse_bookmarks(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(
            items,
            [("乙".to_string(), "https://b.example/".to_string())]
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn duplicate_urls_get_unique_ids() {
        let file = chromium::BookmarkFile {
            browser: Browser::Edge,
            profile: String::new(),
            path: PathBuf::new(),
        };
        let mut seen = HashSet::new();
        let a = make_entry(&file, "t".into(), "https://x.example/".into(), &mut seen);
        let b = make_entry(&file, "t".into(), "https://x.example/".into(), &mut seen);
        assert_ne!(a.item_id(), b.item_id());
    }
}
