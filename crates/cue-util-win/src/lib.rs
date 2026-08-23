//! cue-util-win —— 模块间共享的 Win32 小工具(§72–73 Rule of Three:
//! 同一助手第三次被复制时下沉到 util crate;只下沉,永不上浮进 Core)。
//!
//! 当前住户(均为 app → bookmark → file 第三次复制的收编):
//! - [`com`]:COM 初始化 guard;
//! - [`icon`]:SHGetFileInfoW 系统图标提取 + HICON → §14 RGBA;
//! - [`shell`]:ShellExecuteExW 启动/打开。
//!
//! 本 crate 不是协议的一部分:依赖 cue-protocol 仅为复用 `IconImage` /
//! `ModuleError` 类型,不引入任何业务语义。

pub mod com;
pub mod icon;
pub mod shell;
