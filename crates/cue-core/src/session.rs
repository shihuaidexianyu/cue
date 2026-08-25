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
    /// 当前 generation 有 query 在途、结果尚未提交。§102 要求输入
    /// 变化立即清空 results(激活安全),但视图层若把"已清空"立刻
    /// 画出来,每次击键都会把结果区闪成空态(bug 3:选中带上边缘
    /// 的逐键频闪被用户看成"分割线抖动")。视图据此位保持绘制
    /// 上一批行,直到提交到达(空结果也算提交,见 §115 增补)。
    pub results_pending: bool,
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
            results_pending: false,
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
