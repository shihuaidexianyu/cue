//! cue-util-win —— 模块间共享的 Win32 小工具(Rule of Three:
//! 同一助手第三次被复制时下沉到 util crate;只下沉,永不上浮进 Core)。
//!
//! 住户:
//! - [`com`]:COM 初始化 guard(app → bookmark → file 第三次复用时下沉);
//! - [`icon`]:SHGetFileInfoW 系统图标提取 + HICON → RGBA(同下沉);
//! - [`shell`]:ShellExecuteExW 打开/启动 + 次级动作原语(runas
//!   提权、explorer /select 定位)+ UTF-16 转换助手(to_wide /
//!   os_str_to_wide,第三个使用处出现时下沉的模块间共享件);
//! - [`clipboard`]:一次性写剪贴板(Copy path/link;不是
//!   clipboard manager)。
//!
//! 本 crate 不是协议的一部分:依赖 cue-protocol 仅为复用 `IconImage` /
//! `ModuleError` 类型,不引入任何业务语义。

pub mod clipboard;
pub mod com;
pub mod icon;
pub mod shell;
