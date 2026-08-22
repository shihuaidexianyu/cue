//! 全局热键(§53)。
//!
//! 事务式替换:先 `RegisterHotKey` 新的,成功后再 `UnregisterHotKey` 旧的——
//! 失败时旧热键仍然有效,Core 保留旧值(§42 的 try-apply 同步回调即调用这里)。
//! 注册一律带 `MOD_NOREPEAT`,避免长按自动重复触发 toggle 造成闪烁。

use cue_protocol::{Hotkey, Key};
use std::fmt;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

#[derive(Debug)]
pub struct HotkeyError(pub String);

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HotkeyError {}

pub struct HotkeyManager {
    /// 接收 WM_HOTKEY 的窗口(host window,见 host.rs)。
    hwnd: HWND,
    active: Option<(Hotkey, i32)>,
    /// §53:用两个(递增的)HOTKEY_ID 做事务式替换,
    /// 避免与旧注册在同一 id 上的语义纠缠。
    next_id: i32,
}

impl HotkeyManager {
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            active: None,
            next_id: 1,
        }
    }

    pub fn current(&self) -> Option<Hotkey> {
        self.active.map(|(hk, _)| hk)
    }

    /// 注册新热键并替换旧热键。失败时旧注册不动。
    pub fn apply(&mut self, hotkey: Hotkey) -> Result<(), HotkeyError> {
        // 与当前注册一致 = 用户确认现值,直接成功——系统会把同一
        // 组合的重复注册(即使是我们自己持有的)判为占用失败。
        if self.active.map(|(h, _)| h) == Some(hotkey) {
            return Ok(());
        }
        let (modifiers, vk) = to_win32(&hotkey)?;
        let id = self.next_id;
        unsafe {
            RegisterHotKey(Some(self.hwnd), id, modifiers | MOD_NOREPEAT, vk)
                .map_err(|e| HotkeyError(format!("RegisterHotKey failed: {e}")))?;
        }
        self.next_id += 1;
        if let Some((_, old_id)) = self.active.take() {
            unsafe {
                let _ = UnregisterHotKey(Some(self.hwnd), old_id);
            }
        }
        self.active = Some((hotkey, id));
        Ok(())
    }
}

/// OS-neutral Hotkey → Win32 常量。翻译只存在于本 crate(§111)。
fn to_win32(hotkey: &Hotkey) -> Result<(HOT_KEY_MODIFIERS, u32), HotkeyError> {
    let m = &hotkey.modifiers;
    if m.is_empty() {
        return Err(HotkeyError("hotkey requires at least one modifier".into()));
    }
    let mut modifiers = HOT_KEY_MODIFIERS(0);
    if m.alt {
        modifiers |= MOD_ALT;
    }
    if m.ctrl {
        modifiers |= MOD_CONTROL;
    }
    if m.shift {
        modifiers |= MOD_SHIFT;
    }
    if m.super_key {
        modifiers |= MOD_WIN;
    }

    let vk = match hotkey.key {
        Key::Space => VK_SPACE.0 as u32,
        Key::Tab => VK_TAB.0 as u32,
        Key::Enter => VK_RETURN.0 as u32,
        Key::Escape => VK_ESCAPE.0 as u32,
        Key::Backspace => VK_BACK.0 as u32,
        Key::Delete => VK_DELETE.0 as u32,
        Key::Insert => VK_INSERT.0 as u32,
        Key::Up => VK_UP.0 as u32,
        Key::Down => VK_DOWN.0 as u32,
        Key::Left => VK_LEFT.0 as u32,
        Key::Right => VK_RIGHT.0 as u32,
        Key::Home => VK_HOME.0 as u32,
        Key::End => VK_END.0 as u32,
        Key::PageUp => VK_PRIOR.0 as u32,
        Key::PageDown => VK_NEXT.0 as u32,
        Key::F1 => VK_F1.0 as u32,
        Key::F2 => VK_F2.0 as u32,
        Key::F3 => VK_F3.0 as u32,
        Key::F4 => VK_F4.0 as u32,
        Key::F5 => VK_F5.0 as u32,
        Key::F6 => VK_F6.0 as u32,
        Key::F7 => VK_F7.0 as u32,
        Key::F8 => VK_F8.0 as u32,
        Key::F9 => VK_F9.0 as u32,
        Key::F10 => VK_F10.0 as u32,
        Key::F11 => VK_F11.0 as u32,
        Key::F12 => VK_F12.0 as u32,
        Key::Char(c) => {
            let c = c.to_ascii_uppercase();
            if c.is_ascii_alphanumeric() {
                // ASCII 字母(大写)与数字的 VK 码等于其码点。
                c as u32
            } else {
                // 标点符号必须映射到 VK_OEM_*;直接用码点会错位
                // (如 ','=44 撞上 VK_SNAPSHOT)。映射名按 US 布局
                // 命名,其他键盘布局上物理键位可能不同——接受的妥协。
                let vk = match c {
                    ';' => VK_OEM_1,
                    '/' => VK_OEM_2,
                    '`' => VK_OEM_3,
                    '[' => VK_OEM_4,
                    '\\' => VK_OEM_5,
                    ']' => VK_OEM_6,
                    '\'' => VK_OEM_7,
                    '-' => VK_OEM_MINUS,
                    '=' => VK_OEM_PLUS,
                    ',' => VK_OEM_COMMA,
                    '.' => VK_OEM_PERIOD,
                    _ => return Err(HotkeyError(format!("unsupported hotkey char: {c:?}"))),
                };
                vk.0 as u32
            }
        }
    };
    Ok((modifiers, vk))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vk_of(c: char) -> Result<u32, HotkeyError> {
        to_win32(&Hotkey {
            modifiers: cue_protocol::Modifiers::ALT,
            key: Key::Char(c),
        })
        .map(|(_, vk)| vk)
    }

    #[test]
    fn punctuation_maps_to_oem_vk() {
        assert_eq!(vk_of(',').unwrap(), VK_OEM_COMMA.0 as u32);
        assert_eq!(vk_of('.').unwrap(), VK_OEM_PERIOD.0 as u32);
        assert_eq!(vk_of(';').unwrap(), VK_OEM_1.0 as u32);
        assert_eq!(vk_of('/').unwrap(), VK_OEM_2.0 as u32);
        assert_eq!(vk_of('-').unwrap(), VK_OEM_MINUS.0 as u32);
        // 码点直接当 VK 会撞上 VK_SNAPSHOT 等键位,回归断言。
        assert_ne!(vk_of(',').unwrap(), ',' as u32);
    }

    #[test]
    fn letters_digits_still_identity() {
        assert_eq!(vk_of('k').unwrap(), 'K' as u32);
        assert_eq!(vk_of('7').unwrap(), '7' as u32);
        // Shift 组合字符无法映射到单键 VK,拒绝。
        assert!(vk_of('!').is_err());
        assert!(vk_of('@').is_err());
    }
}
