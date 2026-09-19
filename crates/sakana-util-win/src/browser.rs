//! 浏览器家族(§143 Rule of Three 下沉:bookmark 私有的
//! `chromium::Browser` 出现第三、四使用处——web 搜索组合、
//! app「打开链接」行的用 Edge/Chrome 打开次级动作)。
//!
//! - [`Browser`]:Edge/Chrome 身份(display/key/exe 候选路径探测,
//!   env 目录拼候选 + `is_file` 实测,不引注册表);
//! - [`open_url`]:指定浏览器 exe 带 URL 启动;exe 缺失(浏览器已
//!   卸载等)退回系统默认浏览器——「宁可降级,不让激活失败」
//!   (§117 bookmark 哲学,web/app 沿用);
//! - [`load_icons`]:每浏览器 exe 提取一张图标(§117 bookmark 同法,
//!   调用方负责 COM 线程)。

use sakana_protocol::{IconImage, ModuleError};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// V1.x 支持的 Chromium 系浏览器。display 进 accessory;exe 用于
/// 行图标与按浏览器启动。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Browser {
    Edge,
    Chrome,
}

/// 全部浏览器(表顺序 = 无 usage 时的默认顺序)。
pub const ALL: [Browser; 2] = [Browser::Edge, Browser::Chrome];

impl Browser {
    /// 行内 accessory 显示名。
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

    /// 浏览器 exe 候选路径(env 目录拼候选 + is_file 实测)。
    /// 微秒级,调用方按查询现探即可(中途安装也能及时可见)。
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

/// 指定浏览器打开 URL:来源浏览器 exe 带 URL 参数启动;exe 找不到
/// 退回系统默认浏览器(ShellExecute 直接把 URL 路由给注册 handler)。
pub fn open_url(browser: Browser, url: &str) -> Result<(), ModuleError> {
    match browser.exe_path() {
        Some(exe) => crate::shell::shell_execute(&exe.to_string_lossy(), Some(url), None),
        None => crate::shell::shell_execute(url, None, None),
    }
}

/// 每个已发现浏览器提取一张 exe 图标。调用方负责在 COM 线程上调用
/// (ComGuard;SHGetImageList 需要)。找不到 exe / 提取失败的浏览器
/// 不进 map(调用方决定兜底字形,§135)。
pub fn load_icons() -> HashMap<Browser, Arc<IconImage>> {
    let mut out = HashMap::new();
    for browser in ALL {
        if let Some(icon) = browser
            .exe_path()
            .and_then(|exe| crate::icon::extract_file_icon(&exe))
        {
            out.insert(browser, Arc::new(icon));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本机 Edge 必装(目标平台现实),其 exe 图标必须提得出。
    #[test]
    fn edge_icon_extracts() {
        let _com = crate::com::ComGuard::new();
        let Some(exe) = Browser::Edge.exe_path() else {
            eprintln!("edge not installed, skipping");
            return;
        };
        let icon = crate::icon::extract_file_icon(&exe).expect("edge icon extraction");
        assert_eq!(
            (icon.width, icon.height),
            (crate::icon::ICON_SIZE, crate::icon::ICON_SIZE)
        );
        assert!(
            icon.rgba.as_chunks::<4>().0.iter().any(|px| px[3] > 0),
            "图标不应全透明"
        );
    }
}
