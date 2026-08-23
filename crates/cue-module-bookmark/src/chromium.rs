//! Chromium 系书签数据源:
//! `<User Data>/<profile>/{Bookmarks,AccountBookmarks}` JSON。
//!
//! 对齐 Flow Launcher BrowserBookmark 插件的发现方式:枚举 User Data
//! 下所有 profile 目录(Default / Profile N ……)的 `Bookmarks`(本地
//! 书签)与 `AccountBookmarks`(账号书签——登录 Google 账号后 Chrome
//! 改写它而不再维护 `Bookmarks`,两者 JSON 同构、可同时存在),递归
//! 遍历 `roots`(`folder`/`workspace` 容器、Opera `custom_root`)。
//! JSON 无锁,浏览器运行中可直接读;Firefox(places.sqlite)不在
//! V1.x 范围。

use std::path::PathBuf;

/// V1.x 支持的 Chromium 系浏览器。display 进 accessory;exe 用于行图标。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Browser {
    Edge,
    Chrome,
}

impl Browser {
    pub fn display(self) -> &'static str {
        match self {
            Browser::Edge => "Edge",
            Browser::Chrome => "Chrome",
        }
    }

    /// usage 身份前缀:打开动作按来源浏览器区分(从哪来回哪开),
    /// item_key = `{key}:{url}`。
    pub fn key(self) -> &'static str {
        match self {
            Browser::Edge => "edge",
            Browser::Chrome => "chrome",
        }
    }

    /// User Data 目录(%LOCALAPPDATA% 下)。
    fn user_data(self) -> Option<PathBuf> {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
        let rel = match self {
            Browser::Edge => r"Microsoft\Edge\User Data",
            Browser::Chrome => r"Google\Chrome\User Data",
        };
        Some(local.join(rel))
    }

    /// 浏览器 exe 候选路径(行图标提取用)。
    pub fn exe_path(self) -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        let push_env = |candidates: &mut Vec<PathBuf>, var: &str, rel: &str| {
            if let Some(base) = std::env::var_os(var) {
                candidates.push(PathBuf::from(base).join(rel));
            }
        };
        match self {
            Browser::Edge => {
                let rel = r"Microsoft\Edge\Application\msedge.exe";
                push_env(&mut candidates, "ProgramFiles(x86)", rel);
                push_env(&mut candidates, "ProgramFiles", rel);
            }
            Browser::Chrome => {
                let rel = r"Google\Chrome\Application\chrome.exe";
                push_env(&mut candidates, "ProgramFiles", rel);
                push_env(&mut candidates, "ProgramFiles(x86)", rel);
                push_env(&mut candidates, "LOCALAPPDATA", rel);
            }
        }
        candidates.into_iter().find(|p| p.is_file())
    }
}

const BROWSERS: [Browser; 2] = [Browser::Edge, Browser::Chrome];

/// 一个 profile 的一个书签文件。(browser, profile 显示名, 文件路径);
/// "Default" profile 的显示名为空(Flow 同款:默认 profile 不标注)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookmarkFile {
    pub browser: Browser,
    pub profile: String,
    pub path: PathBuf,
}

/// 每个 profile 目录下的书签文件名:本地书签 + 账号书签。同一 URL
/// 若两边都有,按 "宁可重复"原则各出一行(item_key 相同,usage
/// 自然合并)。
const BOOKMARK_FILE_NAMES: [&str; 2] = ["Bookmarks", "AccountBookmarks"];

/// 枚举所有已安装浏览器的所有 profile 的书签文件。
/// 纯 readdir/stat,亚毫秒级;每次查询都可安全重跑(刷新策略)。
pub fn discover_files() -> Vec<BookmarkFile> {
    let mut out = Vec::new();
    for browser in BROWSERS {
        if let Some(user_data) = browser.user_data() {
            scan_user_data(browser, &user_data, &mut out);
        }
    }
    out.sort_by(|a, b| {
        a.browser
            .cmp(&b.browser)
            .then(a.profile.cmp(&b.profile))
            .then(a.path.cmp(&b.path))
    });
    out
}

/// 扫描一个 User Data 目录:每个 profile 的每个书签文件产出一条
/// ("System Profile" 等目录没有这些文件,自然跳过)。测试可直接喂
/// 临时目录。
fn scan_user_data(browser: Browser, user_data: &std::path::Path, out: &mut Vec<BookmarkFile>) {
    let Ok(entries) = std::fs::read_dir(user_data) else {
        return; // 浏览器未安装
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let profile = if name == "Default" {
            String::new()
        } else {
            name
        };
        for file_name in BOOKMARK_FILE_NAMES {
            let path = dir.join(file_name);
            if path.is_file() {
                out.push(BookmarkFile {
                    browser,
                    profile: profile.clone(),
                    path,
                });
            }
        }
    }
}

/// 从 URL 取域名(展示/搜索键):去 scheme,截到第一个 `/ : ?`。
/// 手写而不引 url crate:这里只需要域名。
pub fn domain_of(url: &str) -> &str {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let end = after_scheme
        .find(['/', ':', '?'])
        .unwrap_or(after_scheme.len());
    &after_scheme[..end]
}

/// 解析一个 Bookmarks 文件,产出 (title, url) 对。坏 JSON / 缺字段
/// 一律跳过而非报错(外部数据永不 panic)。
pub fn parse_bookmarks(json: &str) -> Vec<(String, String)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(roots) = value.get("roots").and_then(|r| r.as_object()) {
        for node in roots.values() {
            walk(node, &mut out);
        }
    }
    out
}

/// 递归遍历:任何带 `children` 数组的对象都是容器(folder /
/// bookmark_bar / other / synced / workspace / custom_root 一视同仁),
/// `type == "url"` 才是书签。
fn walk(node: &serde_json::Value, out: &mut Vec<(String, String)>) {
    let Some(obj) = node.as_object() else {
        return;
    };
    if obj.get("type").and_then(|t| t.as_str()) == Some("url") {
        if let (Some(name), Some(url)) = (
            obj.get("name").and_then(|v| v.as_str()),
            obj.get("url").and_then(|v| v.as_str()),
        ) {
            out.push((name.to_string(), url.to_string()));
        }
    }
    if let Some(children) = obj.get("children").and_then(|c| c.as_array()) {
        for child in children {
            walk(child, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 发现:Default 只有账号书签(登录 Google 账号的 Chrome 现状)、
    /// Profile 1 两者都有、System Profile 啥也没有。
    #[test]
    fn discovers_local_and_account_bookmarks_per_profile() {
        let tmp = std::env::temp_dir().join(format!("cue-bm-disc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let default = tmp.join("Default");
        std::fs::create_dir_all(&default).unwrap();
        std::fs::write(default.join("AccountBookmarks"), "{}").unwrap();
        let p1 = tmp.join("Profile 1");
        std::fs::create_dir_all(&p1).unwrap();
        std::fs::write(p1.join("Bookmarks"), "{}").unwrap();
        std::fs::write(p1.join("AccountBookmarks"), "{}").unwrap();
        std::fs::create_dir_all(tmp.join("System Profile")).unwrap();

        let mut out = Vec::new();
        scan_user_data(Browser::Chrome, &tmp, &mut out);
        out.sort_by(|a, b| a.profile.cmp(&b.profile).then(a.path.cmp(&b.path)));

        let paths: Vec<(&str, &str)> = out
            .iter()
            .map(|f| {
                (
                    f.profile.as_str(),
                    f.path.file_name().unwrap().to_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            paths,
            [
                ("", "AccountBookmarks"),
                ("Profile 1", "AccountBookmarks"),
                ("Profile 1", "Bookmarks"),
            ]
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    const FIXTURE: &str = r##"{
        "roots": {
            "bookmark_bar": {
                "type": "folder",
                "children": [
                    {"type": "url", "name": "GitHub", "url": "https://github.com/"},
                    {"type": "folder", "name": "开发", "children": [
                        {"type": "url", "name": "文档", "url": "https://docs.rs/"},
                        {"type": "workspace", "name": "WS", "children": [
                            {"type": "url", "name": "Issue", "url": "https://github.com/x/1"}
                        ]}
                    ]},
                    {"type": "url", "name": "缺url字段被跳过"},
                    {"type": "url", "url": "https://缺name被跳过.example/"}
                ]
            },
            "custom_root": {
                "children": [
                    {"type": "url", "name": "Opera风格", "url": "https://opera.example/"}
                ]
            },
            "synced": {"type": "folder", "children": []}
        }
    }"##;

    #[test]
    fn parses_nested_folders_workspaces_and_custom_root() {
        let items = parse_bookmarks(FIXTURE);
        let urls: Vec<&str> = items.iter().map(|(_, u)| u.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://github.com/",
                "https://docs.rs/",
                "https://github.com/x/1",
                "https://opera.example/",
            ]
        );
    }

    #[test]
    fn bad_json_and_missing_fields_never_panic() {
        assert!(parse_bookmarks("not json").is_empty());
        assert!(parse_bookmarks("{}").is_empty());
        assert!(parse_bookmarks(r#"{"roots": 42}"#).is_empty());
    }

    #[test]
    fn domain_extraction() {
        assert_eq!(domain_of("https://github.com/x/y?z=1"), "github.com");
        assert_eq!(domain_of("http://localhost:8080/a"), "localhost");
        assert_eq!(domain_of("chrome://bookmarks/"), "bookmarks");
        assert_eq!(domain_of("about:blank"), "about");
    }
}
