use crate::session::SessionId;
use cue_protocol::{ModuleEvent, ModuleId, ModuleOutcome, QueryResult};

/// QueryTicket —— query 的身份完全是 Core runtime 的关注点,不进协议。
/// 接受一个 query 结果要求四项全部匹配当前状态。
#[derive(Clone, Debug)]
pub struct QueryTicket {
    pub session_id: SessionId,
    pub module_id: ModuleId,
    pub module_epoch: u64,
    pub generation: u64,
}

/// Activation ticket。session 处置只对发起它的 session 生效;
/// usage 记录不受 session 影响。
#[derive(Clone, Debug)]
pub struct ActivationTicket {
    pub session_id: SessionId,
    pub module_id: ModuleId,
    pub module_epoch: u64,
}

/// Host → Core 的输入事件。OS-neutral:由 cue-windows 从 Win32 翻译而来。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostEvent {
    /// 全局热键按下(toggle)。
    HotkeyPressed,
    /// 第二实例请求 show / focus。
    ShowRequested,
    /// 前台焦点离开 Launcher 窗口。
    FocusLost,
    /// 托盘菜单"设置"(入口, 设置页)。
    OpenSettings,
}

/// 单一事件队列:所有异步完成都从这里回流到 UI 线程。
pub enum CoreEvent {
    QueryCompleted {
        ticket: QueryTicket,
        result: QueryResult,
    },
    ActivationCompleted {
        ticket: ActivationTicket,
        outcome: ModuleOutcome,
    },
    /// Module 自发事件,sink 在 load 时绑定 epoch。
    ModuleEvent {
        module_id: ModuleId,
        module_epoch: u64,
        event: ModuleEvent,
    },
    Host(HostEvent),
}
