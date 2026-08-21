use cue_protocol::{ItemId, ModuleError, ModuleId, ModuleItem};

/// Session 标识。跨 session 的旧结果由它保证必死(§96)——
/// generation 在每个 session 内从 0 递增即可。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

/// §5.1 SessionState。
pub struct SessionState {
    pub id: SessionId,
    pub raw_input: String,
    pub active_module: ModuleId,
    /// session 内单调递增;输入每次变化 +1(§102)。
    pub generation: u64,
    pub results: Vec<ModuleItem>,
    pub selected: Option<usize>,
    pub error: Option<ModuleError>,
    /// Enter 后 activation 在途,期间忽略重复 Enter。
    pub activation_in_flight: bool,
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
        }
    }

    pub fn selected_item(&self) -> Option<&ModuleItem> {
        self.selected.and_then(|i| self.results.get(i))
    }

    pub fn contains_item(&self, id: ItemId) -> bool {
        self.results.iter().any(|r| r.id() == id)
    }
}
