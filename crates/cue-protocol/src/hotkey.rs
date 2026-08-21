/// §53 OS-neutral 热键描述。
///
/// 禁止把 Win32 常量(`MOD_*`、`VK_*`)作为设置值存进 Core(§111);
/// 翻译到平台 API 属于 Host。

/// 修饰键。位掩码语义,用 bool 字段表达,便于序列化与 UI 渲染。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub super_key: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        alt: false,
        ctrl: false,
        shift: false,
        super_key: false,
    };

    pub const ALT: Self = Self {
        alt: true,
        ..Self::NONE
    };

    pub fn is_empty(&self) -> bool {
        !self.alt && !self.ctrl && !self.shift && !self.super_key
    }
}

/// 常见可注册键位(V1 覆盖范围)。`Char` 为 ASCII 可打印字符,
/// 字母统一按大写规范化存储。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Space,
    Tab,
    Enter,
    Escape,
    Backspace,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Char(char),
}

/// §53 全局热键设置值。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl Default for Hotkey {
    /// §53 默认 Alt+Space。
    fn default() -> Self {
        Self {
            modifiers: Modifiers::ALT,
            key: Key::Space,
        }
    }
}
