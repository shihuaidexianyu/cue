use crate::action::{ActionDescriptor, ActionId};
use crate::error::ModuleError;
use crate::item::ModuleItem;
use crate::module::Module;
use crate::outcome::ModuleOutcome;
use crate::presentation::ResultPresentation;
use std::future::Future;
use std::pin::Pin;

/// §9 LauncherModule —— 能参与 Launcher 输入交互的 Module。
///
/// 它必须回答四个问题:输入给我之后返回什么?这些结果展示什么?
/// 这个结果有哪些行为?执行行为后发生什么?
pub trait LauncherModule: Module {
    fn launcher_descriptor(&self) -> LauncherDescriptor;

    fn query(&mut self, ctx: QueryContext) -> QueryFuture;

    fn present(&self, item: &ModuleItem) -> ResultPresentation;

    fn actions(&self, item: &ModuleItem) -> Vec<ActionDescriptor>;

    fn activate(&mut self, item: &ModuleItem, action: ActionId) -> ActivationFuture;
}

/// §10
#[derive(Clone, Debug)]
pub struct LauncherDescriptor {
    /// `None` 表示无前缀模态;一个 Launcher 只能有一个 default Module(§82)。
    pub trigger: Option<String>,
    pub is_default: bool,
}

/// §94。
///
/// v0.2:`generation` 已删除——staleness 是 Core 的 bookkeeping(§96),
/// 不由 Module 回显。`result_limit` 是 Core/UI 的请求预算(V1 为 Core
/// 内固定值),不来自任何 `module.*` 设置。
#[derive(Clone, Debug)]
pub struct QueryContext {
    pub query: String,
    pub result_limit: usize,
}

/// §95。Module 只回答"结果是什么";有效性判定全部在 Core 侧(§96)。
#[derive(Debug)]
pub struct QueryResponse {
    pub items: Vec<ModuleItem>,
}

/// §93。必须 `Send + 'static`;创建 Future 本身 < 1 ms,创建时不得触碰
/// IO / IPC / 磁盘。`&mut self` 只用于启动工作,Future 内部持有的是
/// Module 事先准备好的 `Arc` 状态或 channel,不借用 self。
pub type QueryFuture = Pin<Box<dyn Future<Output = QueryResult> + Send>>;

pub type QueryResult = Result<QueryResponse, ModuleError>;

/// Activation 的错误在 `ModuleOutcome` 内表达(§22),不单独设 `Err`。
pub type ActivationFuture = Pin<Box<dyn Future<Output = ModuleOutcome> + Send>>;
