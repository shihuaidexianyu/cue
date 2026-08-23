//! cue-util-win —— 模块间共享的 Win32 小工具(§72–73 Rule of Three:
//! 同一助手第三次被复制时下沉到 util crate;只下沉,永不上浮进 Core)。
//!
//! 住户:
//! - [`com`]:COM 初始化 guard(app → bookmark → file 第三次复制的收编);
//! - [`icon`]:SHGetFileInfoW 系统图标提取 + HICON → §14 RGBA(同收编);
//! - [`shell`]:ShellExecuteExW 打开/启动 + §18 次级动作原语(runas
//!   提权、explorer /select 定位);
//! - [`clipboard`]:一次性写剪贴板(§18 Copy path/link;不是 §76 的
//!   clipboard manager)。
//!
//! 本 crate 不是协议的一部分:依赖 cue-protocol 仅为复用 `IconImage` /
//! `ModuleError` 类型,不引入任何业务语义。

pub mod clipboard;
pub mod com;
pub mod icon;
pub mod shell;
