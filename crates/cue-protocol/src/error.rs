use std::fmt;

/// §61 Module 错误模型。V1 不需要复杂的 error hierarchy。
///
/// Module 与 Core 同进程,外部数据失败必须返回本类型而不是 panic(§63)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleError {
    Unavailable(String),
    QueryFailed(String),
    ActivationFailed(String),
    InvalidState(String),
    Internal(String),
}

impl fmt::Display for ModuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "module unavailable: {m}"),
            Self::QueryFailed(m) => write!(f, "query failed: {m}"),
            Self::ActivationFailed(m) => write!(f, "activation failed: {m}"),
            Self::InvalidState(m) => write!(f, "invalid state: {m}"),
            Self::Internal(m) => write!(f, "internal error: {m}"),
        }
    }
}

impl std::error::Error for ModuleError {}
