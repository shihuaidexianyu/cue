//! 锁键服务(§140)的配置:纯数据、OS-neutral(§110),
//! Core 负责校验与持久化,host(sakana-windows)负责翻译执行。
//!
//! 手势语义(移植自 WinCaps,MIT):中文输入法前台时,轻点触发键
//! = 发送输入法切换快捷键,长按 = 切换大写;其余前台原样透传。
//! NumLock 无切输入法语义,提供防误触(长按才切换)与常开守护。

use std::fmt;

/// 手势触发键。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockRemapKey {
    CapsLock,
    ScrollLock,
}

/// 状态上报用的锁键标识(纯数据;host 消息与 OSD 文案都按它区分)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockKey {
    Caps,
    Num,
}

/// 轻点触发键时注入的输入法切换快捷键。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockTapAction {
    /// Ctrl+Space(微软拼音等多数 IME 的中英切换)。
    CtrlSpace,
    /// 单发 Shift(另一种常见 IME 中英切换)。
    Shift,
}

/// NumLock 键的处理模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumLockMode {
    /// 不干预,系统原生行为。
    Native,
    /// 轻点吞掉,按住 ≥ hold_ms 才放行切换(防误触)。
    Hold,
    /// 吞掉全部 NumLock 按下;发现状态为关即注入一次切换打回开。
    AlwaysOn,
}

/// 锁键服务全量配置(每次 commit 后整体推给 host worker)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockKeysConfig {
    /// 总开关:false = 手势/守护全部停用,按键全原生(OSD 也停)。
    pub enabled: bool,
    pub remap_key: LockRemapKey,
    pub tap_action: LockTapAction,
    /// 轻点/长按分界(毫秒)。
    pub hold_ms: u32,
    pub numlock_mode: NumLockMode,
    /// 状态变化时是否弹 OSD 卡片。
    pub osd: bool,
}

impl Default for LockKeysConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            remap_key: LockRemapKey::CapsLock,
            tap_action: LockTapAction::CtrlSpace,
            hold_ms: 350,
            numlock_mode: NumLockMode::Hold,
            osd: true,
        }
    }
}

impl LockKeysConfig {
    pub const REMAP_KEYS: &'static [&'static str] = &["caps_lock", "scroll_lock"];
    pub const TAP_ACTIONS: &'static [&'static str] = &["ctrl_space", "shift"];
    pub const NUMLOCK_MODES: &'static [&'static str] = &["native", "hold", "always_on"];
    pub const HOLD_MS_RANGE: std::ops::RangeInclusive<u32> = 150..=1000;

    /// 设置值(字符串)→ 配置。所有校验集中于此:Core 的事务
    /// 验证与 commit 后组装都走这一个函数,永不分叉。
    #[allow(clippy::too_many_arguments)]
    pub fn from_settings(
        enabled: bool,
        remap_key: &str,
        tap_action: &str,
        hold_ms: &str,
        numlock_mode: &str,
        osd: bool,
    ) -> Result<Self, LockKeysConfigError> {
        let remap_key = match remap_key.trim() {
            "caps_lock" => LockRemapKey::CapsLock,
            "scroll_lock" => LockRemapKey::ScrollLock,
            other => {
                return Err(LockKeysConfigError::BadValue {
                    key: "remap_key",
                    value: other.to_string(),
                    allowed: Self::REMAP_KEYS,
                });
            }
        };
        let tap_action = match tap_action.trim() {
            "ctrl_space" => LockTapAction::CtrlSpace,
            "shift" => LockTapAction::Shift,
            other => {
                return Err(LockKeysConfigError::BadValue {
                    key: "tap_action",
                    value: other.to_string(),
                    allowed: Self::TAP_ACTIONS,
                });
            }
        };
        let hold_ms: u32 = hold_ms
            .trim()
            .parse()
            .map_err(|_| LockKeysConfigError::BadHoldMs(hold_ms.trim().to_string()))?;
        if !Self::HOLD_MS_RANGE.contains(&hold_ms) {
            return Err(LockKeysConfigError::BadHoldMs(hold_ms.to_string()));
        }
        let numlock_mode = match numlock_mode.trim() {
            "native" => NumLockMode::Native,
            "hold" => NumLockMode::Hold,
            "always_on" => NumLockMode::AlwaysOn,
            other => {
                return Err(LockKeysConfigError::BadValue {
                    key: "numlock_mode",
                    value: other.to_string(),
                    allowed: Self::NUMLOCK_MODES,
                });
            }
        };
        Ok(Self {
            enabled,
            remap_key,
            tap_action,
            hold_ms,
            numlock_mode,
            osd,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockKeysConfigError {
    BadValue {
        key: &'static str,
        value: String,
        allowed: &'static [&'static str],
    },
    BadHoldMs(String),
}

impl fmt::Display for LockKeysConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadValue {
                key,
                value,
                allowed,
            } => write!(f, "{key} 的值 `{value}` 无效,可选:{}", allowed.join(" / ")),
            Self::BadHoldMs(v) => write!(
                f,
                "hold_ms 的值 `{v}` 无效,需为 {}–{} 毫秒",
                LockKeysConfig::HOLD_MS_RANGE.start(),
                LockKeysConfig::HOLD_MS_RANGE.end()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrips_through_strings() {
        let d = LockKeysConfig::default();
        let parsed = LockKeysConfig::from_settings(
            d.enabled,
            "caps_lock",
            "ctrl_space",
            "350",
            "hold",
            d.osd,
        )
        .unwrap();
        assert_eq!(parsed, d);
    }

    #[test]
    fn all_variants_parse() {
        for (key, tap, mode) in [
            ("caps_lock", "ctrl_space", "native"),
            ("scroll_lock", "shift", "always_on"),
        ] {
            assert!(LockKeysConfig::from_settings(true, key, tap, "500", mode, false).is_ok());
        }
    }

    #[test]
    fn rejects_bad_values_with_readable_errors() {
        let err =
            LockKeysConfig::from_settings(true, "num_lock", "ctrl_space", "350", "hold", true)
                .unwrap_err();
        assert!(err.to_string().contains("caps_lock / scroll_lock"));
        let err =
            LockKeysConfig::from_settings(true, "caps_lock", "ctrl_space", "99", "hold", true)
                .unwrap_err();
        assert!(err.to_string().contains("150"));
        let err =
            LockKeysConfig::from_settings(true, "caps_lock", "ctrl_space", "abc", "hold", true)
                .unwrap_err();
        assert!(matches!(err, LockKeysConfigError::BadHoldMs(_)));
        let err =
            LockKeysConfig::from_settings(true, "caps_lock", "ctrl_space", "350", "locked", true)
                .unwrap_err();
        assert!(err.to_string().contains("native / hold / always_on"));
    }

    #[test]
    fn trims_whitespace() {
        assert!(
            LockKeysConfig::from_settings(true, " caps_lock ", "ctrl_space", " 350 ", "hold", true)
                .is_ok()
        );
    }
}
