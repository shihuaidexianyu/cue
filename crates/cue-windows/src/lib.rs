//! cue-windows —— Windows Host(平台代码隔离区)。
//!
//! 所有 Win32 调用只允许存在于本 crate 与各 Module 内部。
//! 本 crate 不依赖 cue-core:Host 事件通过 [`host::HostMsg`] 上报,
//! 由编排层(cue binary)翻译成 Core 的 HostEvent。

pub mod autostart;
pub mod host;
pub mod hotkey;
pub mod icon;
pub mod ime;
pub mod monitor;
pub mod single_instance;
pub mod tray;
pub mod window;
