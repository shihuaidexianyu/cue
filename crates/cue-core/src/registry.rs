use cue_protocol::{LauncherDescriptor, LauncherModule, ModuleError, ModuleId};
use std::collections::HashMap;
use std::fmt;

/// Module Registry。V1 直接存 `Box<dyn LauncherModule>`——
/// stable Rust 无法在 `dyn Module` 上运行时查询"是否同时是
/// LauncherModule"(无 trait upcasting),而 V1 所有 Module 都是
/// LauncherModule。不解决不存在的问题。
pub struct ModuleRegistry {
    modules: HashMap<ModuleId, ModuleSlot>,
    /// 保持注册顺序,路由按序匹配 trigger。
    order: Vec<ModuleId>,
    default_module: Option<ModuleId>,
    next_epoch: u64,
}

struct ModuleSlot {
    module: Box<dyn LauncherModule>,
    /// 每次 load 分配,单调递增。unload / reload 后旧实例的
    /// 在途 query 与自发事件全部失效。
    epoch: u64,
}

#[derive(Debug)]
pub enum RegistryError {
    DuplicateModule(ModuleId),
    UnknownModule(ModuleId),
    DuplicateTrigger(String),
    /// 触发词必填(§128):空触发词会按标点分支匹配一切输入,
    /// 吞掉默认路由——声明层直接拒绝。
    EmptyTrigger(ModuleId),
    MultipleDefaults(ModuleId, ModuleId),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateModule(id) => write!(f, "duplicate module: {id}"),
            Self::UnknownModule(id) => write!(f, "unknown module: {id}"),
            Self::DuplicateTrigger(t) => write!(f, "duplicate trigger: {t:?}"),
            Self::EmptyTrigger(id) => write!(f, "empty trigger: {id}"),
            Self::MultipleDefaults(a, b) => write!(f, "multiple default modules: {a}, {b}"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl ModuleRegistry {
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
            order: Vec::new(),
            default_module: None,
            next_epoch: 1,
        }
    }

    pub fn register(&mut self, module: Box<dyn LauncherModule>) -> Result<(), RegistryError> {
        let id = module.descriptor().id.clone();
        if self.modules.contains_key(&id) {
            return Err(RegistryError::DuplicateModule(id));
        }
        let launcher = module.launcher_descriptor();
        if let Some(trigger) = &launcher.trigger {
            if trigger.is_empty() {
                return Err(RegistryError::EmptyTrigger(id));
            }
            for other in self.modules.values() {
                if other.module.launcher_descriptor().trigger.as_ref() == Some(trigger) {
                    return Err(RegistryError::DuplicateTrigger(trigger.clone()));
                }
            }
        }
        if launcher.is_default {
            if let Some(existing) = &self.default_module {
                return Err(RegistryError::MultipleDefaults(existing.clone(), id));
            }
            self.default_module = Some(id.clone());
        }
        let epoch = self.alloc_epoch();
        self.modules
            .insert(id.clone(), ModuleSlot { module, epoch });
        self.order.push(id);
        Ok(())
    }

    /// 替换模块实例(未 load)。epoch 递增,旧实例的 ticket 全部失效。
    pub fn replace(&mut self, module: Box<dyn LauncherModule>) -> Result<(), RegistryError> {
        let id = module.descriptor().id.clone();
        let epoch = self.alloc_epoch();
        let slot = self
            .modules
            .get_mut(&id)
            .ok_or_else(|| RegistryError::UnknownModule(id.clone()))?;
        slot.epoch = epoch;
        slot.module = module;
        Ok(())
    }

    fn alloc_epoch(&mut self) -> u64 {
        let epoch = self.next_epoch;
        self.next_epoch += 1;
        epoch
    }

    pub fn ids(&self) -> Vec<ModuleId> {
        self.order.clone()
    }

    pub fn epoch(&self, id: &ModuleId) -> Option<u64> {
        self.modules.get(id).map(|s| s.epoch)
    }

    pub fn module(&self, id: &ModuleId) -> Option<&dyn LauncherModule> {
        self.modules.get(id).map(|s| &*s.module)
    }

    pub fn module_mut(&mut self, id: &ModuleId) -> Option<&mut Box<dyn LauncherModule>> {
        self.modules.get_mut(id).map(|s| &mut s.module)
    }

    pub fn default_module(&self) -> Option<&ModuleId> {
        self.default_module.as_ref()
    }

    /// 路由按注册顺序给出 (ModuleId, LauncherDescriptor) 供 Core 匹配。
    pub fn launcher_descriptors(&self) -> impl Iterator<Item = (&ModuleId, LauncherDescriptor)> {
        self.order.iter().map(move |id| {
            let slot = &self.modules[id];
            (id, slot.module.launcher_descriptor())
        })
    }
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl From<RegistryError> for ModuleError {
    fn from(e: RegistryError) -> Self {
        ModuleError::InvalidState(e.to_string())
    }
}
