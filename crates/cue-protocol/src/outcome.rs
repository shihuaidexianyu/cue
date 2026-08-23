use crate::action::ActionId;
use crate::error::ModuleError;

/// ModuleOutcome。Module 不直接控制 Core(`close_launcher()` 是禁止的),
/// 只返回 outcome,由 Core 处置。
#[derive(Clone, Debug)]
pub struct ModuleOutcome {
    pub status: OutcomeStatus,
    pub session: SessionDisposition,
    pub usage: Option<UsageRecordRequest>,
}

impl ModuleOutcome {
    pub fn success(session: SessionDisposition, usage: Option<UsageRecordRequest>) -> Self {
        Self {
            status: OutcomeStatus::Success,
            session,
            usage,
        }
    }

    /// activation 失败默认 KeepOpen——普通启动失败不得关掉 Launcher。
    pub fn failed(error: ModuleError) -> Self {
        Self {
            status: OutcomeStatus::Failed(error),
            session: SessionDisposition::KeepOpen,
            usage: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum OutcomeStatus {
    Success,
    Failed(ModuleError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionDisposition {
    Close,
    KeepOpen,
}

/// Activation 成功后 Module 请求记录的 usage。
/// `item_key` 必须是稳定标识:Packaged = AUMID,
/// Win32 = canonical exe + normalized args。
#[derive(Clone, Debug)]
pub struct UsageRecordRequest {
    pub item_key: String,
    pub action_id: ActionId,
}
