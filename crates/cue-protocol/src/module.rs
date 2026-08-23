use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::settings::{SettingsChangeSet, SettingsSchema};
use std::fmt;
use std::sync::Arc;

/// Module ID:稳定、唯一、不依赖显示名称。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModuleId(Arc<str>);

impl ModuleId {
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }

    pub fn from_static(id: &'static str) -> Self {
        Self(Arc::from(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ModuleId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

pub struct ModuleDescriptor {
    pub id: ModuleId,
    pub name: &'static str,
    pub version: &'static str,
}

/// 基础 Module trait。
///
/// `try_apply_settings` 命名即语义:先 try-apply,成功才由 Core commit;
/// 失败不 commit。
pub trait Module {
    fn descriptor(&self) -> &ModuleDescriptor;

    fn load(&mut self, ctx: ModuleContext) -> Result<(), ModuleError>;

    fn unload(&mut self);

    fn settings_schema(&self) -> SettingsSchema;

    fn try_apply_settings(&mut self, changes: SettingsChangeSet) -> Result<(), ModuleError>;
}
