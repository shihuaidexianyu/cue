use futures::future::BoxFuture;

/// Future 的轮询者,由外部注入。
///
/// Core 定义 trait,不提供实现:生产环境是 GPUI executor,
/// 测试是手动 pump 的实现。Core 因此不依赖任何具体 runtime。
///
/// 注意:spawn 返回 `()`,Future 的 ownership 即移交 executor——
/// Core 不做物理取消,stale 结果由 ticket 判定丢弃。
pub trait TaskSpawner: Send + Sync {
    fn spawn(&self, fut: BoxFuture<'static, ()>);
}
