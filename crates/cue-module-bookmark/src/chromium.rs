//! Chromium 系书签数据源(§117):`<User Data>/<profile>/Bookmarks` JSON。
//!
//! 对齐 Flow Launcher BrowserBookmark 插件的发现方式:枚举 User Data
//! 下所有含 `Bookmarks` 文件的 profile 目录(Default / Profile N ……),
//! 递归遍历 `roots`(`folder`/`workspace` 容器、Opera `custom_root`)。
//! JSON 无锁,浏览器运行中可直接读;Firefox(places.sqlite)不在
//! V1.x 范围(§117 依赖决策)。

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

    /// usage 身份前缀(§51):打开动作按来源浏览器区分(从哪来回哪开),
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

    /// 浏览器 exe 候选路径(行图标提取用,§117)。
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

/// 一个 profile 的书签文件。(browser, profile 显示名, Bookmarks 路径);
/// "Default" profile 的显示名为空(Flow 同款:默认 profile 不标注)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookmarkFile {
    pub browser: Browser,
    pub profile: String,
    pub path: PathBuf,
}

/// 枚举所有已安装浏览器的所有 profile 的 Bookmarks 文件。
/// 纯 readdir/stat,亚毫秒级;每次查询都可安全重跑(§117 刷新策略)。
pub fn discover_files() -> Vec<BookmarkFile> {
    let mut out = Vec::new();
    for browser in BROWSERS {
        let Some(user_data) = browser.user_data() else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&user_data) else {
            continue; // 浏览器未安装
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let bookmarks = dir.join("Bookmarks");
            if !bookmarks.is_file() {
                continue; // "System Profile" 等目录无此文件
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let profile = if name == "Default" {
                String::new()
            } else {
                name
            };
            out.push(BookmarkFile {
                browser,
                profile,
                path: bookmarks,
            });
        }
    }
    out.sort_by(|a, b| a.browser.cmp(&b.browser).then(a.profile.cmp(&b.profile)));
    out
}

/// 从 URL 取域名(展示/搜索键):去 scheme,截到第一个 `/ : ?`。
/// 手写而不引 url crate——§72,这里只需要域名。
pub fn domain_of(url: &str) -> &str {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let end = after_scheme
        .find(['/', ':', '?'])
        .unwrap_or(after_scheme.len());
    &after_scheme[..end]
}

/// 解析一个 Bookmarks 文件,产出 (title, url) 对。坏 JSON / 缺字段
/// 一律跳过而非报错(§63:外部数据永不 panic)。
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
