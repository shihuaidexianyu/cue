//! cue-protocol —— Core ↔ Module 协议。
//!
//! 本 crate 只包含数据类型与 trait 定义,不含任何平台代码。
//! Core 与所有 Module 都依赖它;它不依赖任何 crate。

mod action;
mod context;
mod error;
mod hotkey;
mod item;
mod launcher_module;
mod module;
mod outcome;
mod presentation;
mod settings;

pub use action::{ActionDescriptor, ActionId, Shortcut};
pub use context::{
    LogLevel, ModuleContext, ModuleEvent, ModuleEventSend, ModuleEventSink, ModuleLog,
    ModuleLogger, ModuleSettings, ModuleStorage, StorageScope, UsageRead, UsageReader, UsageStat,
};
pub use error::ModuleError;
pub use hotkey::{Hotkey, HotkeyParseError, Key, Modifiers};
pub use item::{ItemId, ModuleItem};
pub use launcher_module::{
    ActivationFuture, LauncherDescriptor, LauncherModule, QueryContext, QueryFuture, QueryResponse,
    QueryResult,
};
pub use module::{Module, ModuleDescriptor, ModuleId};
pub use outcome::{ModuleOutcome, OutcomeStatus, SessionDisposition, UsageRecordRequest};
pub use presentation::{
    IconImage, ResultAccessory, ResultBadge, ResultIcon, ResultPresentation, SystemIconId,
};
pub use settings::{
    ApplyPolicy, SettingKey, SettingKind, SettingSpec, SettingValue, SettingsChangeSet,
    SettingsSchema,
};
