//! OS-neutral 热键描述。
//!
//! 禁止把 Win32 常量(`MOD_*`、`VK_*`)作为设置值存进 Core;
//! 翻译到平台 API 属于 Host。

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
/// 字母统一按小写规范化存储。
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

/// 全局热键设置值。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl Default for Hotkey {
    /// 默认 Alt+Space。
    fn default() -> Self {
        Self {
            modifiers: Modifiers::ALT,
            key: Key::Space,
        }
    }
}

impl Key {
    fn name(self) -> String {
        match self {
            Key::Space => "space".into(),
            Key::Tab => "tab".into(),
            Key::Enter => "enter".into(),
            Key::Escape => "esc".into(),
            Key::Backspace => "backspace".into(),
            Key::Delete => "delete".into(),
            Key::Insert => "insert".into(),
            Key::Up => "up".into(),
            Key::Down => "down".into(),
            Key::Left => "left".into(),
            Key::Right => "right".into(),
            Key::Home => "home".into(),
            Key::End => "end".into(),
            Key::PageUp => "pageup".into(),
            Key::PageDown => "pagedown".into(),
            Key::F1 => "f1".into(),
            Key::F2 => "f2".into(),
            Key::F3 => "f3".into(),
            Key::F4 => "f4".into(),
            Key::F5 => "f5".into(),
            Key::F6 => "f6".into(),
            Key::F7 => "f7".into(),
            Key::F8 => "f8".into(),
            Key::F9 => "f9".into(),
            Key::F10 => "f10".into(),
            Key::F11 => "f11".into(),
            Key::F12 => "f12".into(),
            Key::Char(c) => c.to_ascii_lowercase().to_string(),
        }
    }

    /// 解析键名(小写规范形式;UI 热键捕获与设置反序列化共用)。
    /// 无法识别返回 None(如纯修饰键名)。
    pub fn parse(token: &str) -> Option<Key> {
        Some(match token {
            "space" => Key::Space,
            "tab" => Key::Tab,
            "enter" => Key::Enter,
            "esc" => Key::Escape,
            "backspace" => Key::Backspace,
            "delete" => Key::Delete,
            "insert" => Key::Insert,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            t if t.len() == 1 => Key::Char(t.chars().next()?.to_ascii_lowercase()),
            _ => {
                let n: u32 = token.strip_prefix('f')?.parse().ok()?;
                match n {
                    1 => Key::F1,
                    2 => Key::F2,
                    3 => Key::F3,
                    4 => Key::F4,
                    5 => Key::F5,
                    6 => Key::F6,
                    7 => Key::F7,
                    8 => Key::F8,
                    9 => Key::F9,
                    10 => Key::F10,
                    11 => Key::F11,
                    12 => Key::F12,
                    _ => return None,
                }
            }
        })
    }
}

/// 规范形式:"ctrl+alt+space"(修饰键固定 ctrl/alt/shift/win 序,
/// 键名小写)。设置文件与 `CUE_HOTKEY` 环境变量共用此格式。
impl std::fmt::Display for Hotkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let m = &self.modifiers;
        let mut parts: Vec<&str> = Vec::new();
        if m.ctrl {
            parts.push("ctrl");
        }
        if m.alt {
            parts.push("alt");
        }
        if m.shift {
            parts.push("shift");
        }
        if m.super_key {
            parts.push("win");
        }
        write!(
            f,
            "{}{}",
            parts.join("+") + if parts.is_empty() { "" } else { "+" },
            self.key.name()
        )
    }
}

/// 解析失败(空修饰键、未知键名、多余键位都拒绝——设置值必须
/// 可注册,"取值校验"在解析层完成大半)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyParseError(pub String);

impl std::fmt::Display for HotkeyParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for Hotkey {
    type Err = HotkeyParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let mut modifiers = Modifiers::NONE;
        let mut key = None;
        for token in raw.split('+').map(|t| t.trim().to_ascii_lowercase()) {
            match token.as_str() {
                "alt" => modifiers.alt = true,
                "ctrl" => modifiers.ctrl = true,
                "shift" => modifiers.shift = true,
                "win" | "super" => modifiers.super_key = true,
                t => {
                    if key.is_some() {
                        return Err(HotkeyParseError(format!("multiple keys in hotkey: {raw}")));
                    }
                    key = Some(
                        Key::parse(t)
                            .ok_or_else(|| HotkeyParseError(format!("unknown key: {t}")))?,
                    );
                }
            }
        }
        let key = key.ok_or_else(|| HotkeyParseError("hotkey has no key".into()))?;
        if modifiers.is_empty() {
            return Err(HotkeyParseError("hotkey requires a modifier".into()));
        }
        Ok(Self { modifiers, key })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn display_parse_roundtrip() {
        let hk = Hotkey::default();
        assert_eq!(hk.to_string(), "alt+space");
        assert_eq!(Hotkey::from_str("alt+space"), Ok(hk));

        let hk2 = Hotkey {
            modifiers: Modifiers {
                ctrl: true,
                alt: true,
                shift: false,
                super_key: false,
            },
            key: Key::Char('k'),
        };
        assert_eq!(hk2.to_string(), "ctrl+alt+k");
        assert_eq!(Hotkey::from_str("Ctrl+Alt+K"), Ok(hk2));
        assert_eq!(Hotkey::from_str("win+f5").unwrap().key, Key::F5);
    }

    #[test]
    fn parse_rejects_invalid() {
        assert!(Hotkey::from_str("space").is_err()); // 无修饰键
        assert!(Hotkey::from_str("alt+").is_err()); // 无键
        assert!(Hotkey::from_str("alt+space+tab").is_err()); // 多键
        assert!(Hotkey::from_str("alt+nope").is_err()); // 未知键
        assert!(Hotkey::from_str("alt+f13").is_err());
    }
}
