use crate::action::ActionId;
use crate::item::ItemId;
use crate::module::ModuleId;
use crate::settings::SettingValue;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

/// §49 ModuleContext。Core 加载 Module 时提供。
///
/// `events` sink 在 load 时绑定 `(ModuleId, ModuleEpoch)`(§49、§109):
/// unload / reload 之后,旧 sink 发出的事件一律丢弃。
pub struct ModuleContext {
    pub module_id: ModuleId,
    pub storage: ModuleStorage,
    pub settings: ModuleSettings,
    pub usage: UsageReader,
    pub logger: ModuleLogger,
    pub events: ModuleEventSink,
}

/// §43–47 Module 存储根(`%LOCALAPPDATA%\CUE\modules\<id>\` 下)。
#[derive(Clone, Debug)]
pub struct ModuleStorage {
    /// 持久用户数据(aliases、internal database)。
    pub data: PathBuf,
    /// 可恢复状态(last index time、cursor)。
    pub state: PathBuf,
    /// 可自由删除重建(icons、favicons)。
    pub cache: PathBuf,
}

/// §44
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageScope {
    Data,
    State,
    Cache,
}

/// Module 自己命名空间下的当前设置值快照。
/// 设置的唯一所有者永远是 Settings Host(§48)。
#[derive(Clone, Debug, Default)]
pub struct ModuleSettings {
    values: Arc<HashMap<String, SettingValue>>,
}

impl ModuleSettings {
    pub fn new(values: HashMap<String, SettingValue>) -> Self {
        Self {
            values: Arc::new(values),
        }
    }

    pub fn get(&self, key: &str) -> Option<&SettingValue> {
        self.values.get(key)
    }
}

/// §50 聚合 usage 统计。V1 不存 event log。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsageStat {
    pub count: u64,
    pub last_used: SystemTime,
}

/// Module 读取自己 usage 的接口(绑定 module id)。
pub trait UsageRead: Send + Sync {
    fn stat(&self, item_key: &str, action: ActionId) -> Option<UsageStat>;
}

pub type UsageReader = Arc<dyn UsageRead>;

/// §64 统一 logger。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

pub trait ModuleLog: Send + Sync {
    fn log(&self, level: LogLevel, message: &str);
}

pub type ModuleLogger = Arc<dyn ModuleLog>;

/// §109 Module 自发事件。V1 只有 PresentationInvalidated 一种。
#[derive(Clone, Debug)]
pub enum ModuleEvent {
    PresentationInvalidated { items: Vec<ItemId> },
}

pub trait ModuleEventSend: Send + Sync {
    fn send(&self, event: ModuleEvent);
}

pub type ModuleEventSink = Arc<dyn ModuleEventSend>;
