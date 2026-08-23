//! 行图标(§117):每行显示来源浏览器的图标(Edge/Chrome exe 提取,
//! 提取走 cue-util-win::icon),不做网站 favicon(Flow 默认关;要读
//! "Favicons" sqlite + 拷贝解锁,复杂度与依赖不值)。
//! 提取失败/浏览器未装 → SystemIcon::Generic。
//!
//! 与 app 侧的差异只在调用形态:浏览器图标仅 ≤2 张,load 时在模块
//! 后台线程同步提取一次,不需要 worker 管线。

use crate::chromium::Browser;
use cue_protocol::IconImage;
use std::collections::HashMap;
use std::sync::Arc;

/// 每个已发现浏览器提取一张 exe 图标。调用方负责在 COM 线程上调用
/// (ComGuard;SHGetImageList 需要)。找不到 exe / 提取失败的浏览器
/// 不进 map(present 走 SystemIcon 兜底)。
pub fn load_browser_icons() -> HashMap<Browser, Arc<IconImage>> {
    let mut out = HashMap::new();
    for browser in [Browser::Edge, Browser::Chrome] {
        if let Some(icon) = browser
            .exe_path()
            .and_then(|exe| cue_util_win::icon::extract_file_icon(&exe))
        {
            out.insert(browser, Arc::new(icon));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本机 Edge 必装(§117 目标平台现实),其 exe 图标必须提得出。
    #[test]
    fn edge_icon_extracts() {
        let _com = cue_util_win::com::ComGuard::new();
        let Some(exe) = Browser::Edge.exe_path() else {
            eprintln!("edge not installed, skipping");
            return;
        };
        let icon = cue_util_win::icon::extract_file_icon(&exe).expect("edge icon extraction");
        assert_eq!(
            (icon.width, icon.height),
            (cue_util_win::icon::ICON_SIZE, cue_util_win::icon::ICON_SIZE)
        );
        assert!(
            icon.rgba.chunks_exact(4).any(|px| px[3] > 0),
            "图标不应全透明"
        );
    }
}
