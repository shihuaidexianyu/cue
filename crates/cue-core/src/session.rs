use cue_protocol::{ActionDescriptor, ItemId, ModuleError, ModuleId, ModuleItem};

/// Session 标识。跨 session 的旧结果由它保证必死——
/// generation 在每个 session 内从 0 递增即可。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

/// 次级动作菜单(Tab 打开):打开时对选中项快照
/// `Module::actions`(同步、廉价);输入变化或新结果提交即关。
pub struct ActionMenuState {
    pub actions: Vec<ActionDescriptor>,
    pub selected: usize,
    /// 菜单归属的选中项标题(打开时 present 快照,UI 头部展示)。
    pub item_title: std::sync::Arc<str>,
}

/// SessionState。
pub struct SessionState {
    pub id: SessionId,
    pub raw_input: String,
    pub active_module: ModuleId,
    /// session 内单调递增;输入每次变化 +1。
    pub generation: u64,
    pub results: Vec<ModuleItem>,
    pub selected: Option<usize>,
    pub error: Option<ModuleError>,
    /// Enter 后 activation 在途,期间忽略重复 Enter。
    pub activation_in_flight: bool,
    /// Some 时 UI 渲染动作菜单,↑↓/Enter/Esc 路由给菜单。
    pub action_menu: Option<ActionMenuState>,
}

impl SessionState {
    pub fn new(id: SessionId, active_module: ModuleId) -> Self {
        Self {
            id,
            raw_input: String::new(),
            active_module,
            generation: 0,
            results: Vec::new(),
            selected: None,
            error: None,
            activation_in_flight: false,
            action_menu: None,
        }
    }

    pub fn selected_item(&self) -> Option<&ModuleItem> {
        self.selected.and_then(|i| self.results.get(i))
    }

    pub fn contains_item(&self, id: ItemId) -> bool {
        self.results.iter().any(|r| r.id() == id)
    }
}
