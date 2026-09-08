use crate::hotkey::Hotkey;
use std::path::PathBuf;
use std::sync::Arc;

/// 设置 key。带完整命名空间(`core.*` / `module.<module-id>.*`)。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SettingKey(pub Arc<str>);

/// V1 不建立复杂表单 framework。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKind {
    Bool,
    Integer { min: i64, max: i64 },
    String,
    Enum(&'static [SettingOption]),
    Path,
    Hotkey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingOption {
    pub value: &'static str,
    pub label: &'static str,
}

impl SettingKind {
    pub fn validate(self, value: &SettingValue) -> Result<(), String> {
        match (self, value) {
            (Self::Integer { min, max }, SettingValue::Integer(n)) if (min..=max).contains(n) => {
                Ok(())
            }
            (Self::Integer { min, max }, SettingValue::Integer(_)) => {
                Err(format!("请输入 {min}–{max} 之间的整数"))
            }
            (Self::Enum(options), SettingValue::Enum(v))
                if options.iter().any(|o| o.value == v) =>
            {
                Ok(())
            }
            (Self::Enum(options), SettingValue::Enum(_)) => Err(format!(
                "可选值:{}",
                options
                    .iter()
                    .map(|o| o.label)
                    .collect::<Vec<_>>()
                    .join(" / ")
            )),
            (Self::Bool, SettingValue::Bool(_))
            | (Self::String, SettingValue::String(_))
            | (Self::Path, SettingValue::Path(_))
            | (Self::Hotkey, SettingValue::Hotkey(_)) => Ok(()),
            _ => Err(format!("type mismatch: expected {self:?}")),
        }
    }
}

/// 设置值。注意 `core.*` 的设置值必须是 OS-neutral 数据描述。
#[derive(Clone, Debug, PartialEq)]
pub enum SettingValue {
    Bool(bool),
    Integer(i64),
    String(String),
    Enum(String),
    Path(PathBuf),
    Hotkey(Hotkey),
}

/// `apply_policy` 是每个设置的事务挂载点,不可省略。
#[derive(Clone, Debug)]
pub struct SettingSpec {
    pub key: SettingKey,
    pub label: Arc<str>,
    pub description: Option<Arc<str>>,
    pub kind: SettingKind,
    pub default: SettingValue,
    pub apply_policy: ApplyPolicy,
}

pub type SettingsSchema = Vec<SettingSpec>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyPolicy {
    Immediate,
    ReloadModule,
    RestartApplication,
}

/// 一次设置变更的全部改动(同一 Module 维度)。
#[derive(Clone, Debug, Default)]
pub struct SettingsChangeSet {
    pub changes: Vec<(SettingKey, SettingValue)>,
}
