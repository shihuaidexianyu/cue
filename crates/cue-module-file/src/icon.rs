//! 行图标(§118):文件夹 / 通用文件各一张,load 时后台线程一次性
//! 提取(cue-util-win::icon::extract_virtual_icon,不触盘)。
//! 不做按扩展名的真实类型图标(V1.x 再议——要嘛逐行提取上 UI 线程
//! 不行,要嘛按扩展名缓存,属于增量优化不是必须)。

use cue_protocol::IconImage;
use std::sync::Arc;
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};

pub struct FileIcons {
    pub folder: Arc<IconImage>,
    pub file: Arc<IconImage>,
}

/// 提取两张系统图标。调用方负责在 COM 线程上调用(ComGuard;
/// SHGetImageList 需要)。任一失败返回 None(present 走 SystemIcon 兜底)。
pub fn load_file_icons() -> Option<FileIcons> {
    let folder = cue_util_win::icon::extract_virtual_icon("folder", FILE_ATTRIBUTE_DIRECTORY)?;
    let file = cue_util_win::icon::extract_virtual_icon("file", FILE_ATTRIBUTE_NORMAL)?;
    Some(FileIcons {
        folder: Arc::new(folder),
        file: Arc::new(file),
    })
}
