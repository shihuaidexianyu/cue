use std::sync::Arc;

/// Result Presentation。Module 决定展示什么,Core/GPUI 决定怎么画。
///
/// v0.2:文本统一 `Arc<str>`——`SharedString` 是 GPUI re-export 类型,
/// 会违反依赖方向;到 GPUI 类型的转换留在 ui crate。
#[derive(Clone, Debug)]
pub struct ResultPresentation {
    pub title: Arc<str>,
    pub subtitle: Option<Arc<str>>,
    pub icon: Option<ResultIcon>,
    pub badges: Vec<ResultBadge>,
    pub accessory: Option<ResultAccessory>,
}

impl ResultPresentation {
    pub fn new(title: impl Into<Arc<str>>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            icon: None,
            badges: Vec::new(),
            accessory: None,
        }
    }
}

/// ResultIcon —— 协议自有位图,不是 GPUI 类型。
#[derive(Clone, Debug)]
pub enum ResultIcon {
    Raster(IconImage),
    /// 没有专属图标时的通用字形逃生口。
    SystemIcon(SystemIconId),
}

/// 像素契约:RGBA8、row-major、sRGB、straight(非预乘)alpha,
/// `rgba.len() == width * height * 4`。
/// 若 GPUI 纹理需要预乘 alpha 或其他通道序,由 ui 在上传时转换一次。
///
/// 单尺寸 96px,UI 永远降采样到行内槽位;UI 按 Arc 指针缓存纹理,
/// 因此 Module 对同一张缓存图标必须复用同一个 `Arc<IconImage>` 实例。
#[derive(Clone, Debug)]
pub struct IconImage {
    pub rgba: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}

impl IconImage {
    pub fn new(rgba: Arc<[u8]>, width: u32, height: u32) -> Self {
        debug_assert_eq!(
            rgba.len() as u64,
            width as u64 * height as u64 * 4,
            "rgba.len() == width * height * 4"
        );
        Self {
            rgba,
            width,
            height,
        }
    }
}

/// SystemIcon 逃生口。前四个是 V1 最小集合;`Lock` 起为系统动作
/// 模块(§126)的动作字形——协议自有语义,UI 决定具体字形(emoji)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemIconId {
    App,
    File,
    Folder,
    Generic,
    Lock,
    Sleep,
    Hibernate,
    Logoff,
    Restart,
    Shutdown,
    RecycleBin,
}

#[derive(Clone, Debug)]
pub struct ResultBadge {
    pub text: Arc<str>,
}

/// 右侧辅助信息。
#[derive(Clone, Debug)]
pub enum ResultAccessory {
    Text(Arc<str>),
    Shortcut(Arc<str>),
}
