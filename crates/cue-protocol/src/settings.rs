use crate::hotkey::Hotkey;
use std::path::PathBuf;
use std::sync::Arc;

/// 设置 key。带完整命名空间(`core.*` / `module.<module-id>.*`,§40)。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SettingKey(pub Arc<str>);

/// §39。V1 不建立复杂表单 framework。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKind {
    Bool,
    Integer,
    String,
    Enum,
    Path,
    Hotkey,
}

/// 设置值。注意 `core.*` 的设置值必须是 OS-neutral 数据描述(§111)。
#[derive(Clone, Debug, PartialEq)]
pub enum SettingValue {
    Bool(bool),
    Integer(i64),
    String(String),
    Enum(String),
    Path(PathBuf),
    Hotkey(Hotkey),
}

/// §38。`apply_policy` 是每个设置的事务挂载点(§42),不可省略。
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

/// §42
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
