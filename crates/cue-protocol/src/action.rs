use crate::hotkey::{Key, Modifiers};
use std::sync::Arc;

/// Action ID。每个结果至少有一个 Primary Action,通常绑定 Enter。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActionId(pub u32);

impl ActionId {
    pub const PRIMARY: ActionId = ActionId(0);
}

#[derive(Clone, Debug)]
pub struct ActionDescriptor {
    pub id: ActionId,
    pub label: Arc<str>,
    pub shortcut: Option<Shortcut>,
}

/// Action 快捷键描述。形状与 Hotkey 相同,但不共用一个类型——
/// 两者演化方向不同(热键可配置、Shortcut 由 Module 静态给出)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: Modifiers,
    pub key: Key,
}
