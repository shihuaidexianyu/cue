use std::any::Any;
use std::fmt;
use std::sync::Arc;

/// Item ID。保留至今的原因:PresentationInvalidated 事件寻址、
/// Core 在 ResultState 内的行标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u64);

/// ModuleItem —— owned opaque handle。
///
/// v0.2:`payload` 是 `Arc<dyn Any + Send + Sync>`。Core 只保存、传递它,
/// **从不调用 `downcast_ref`**——只有创建它的 Module 知道 `T` 是什么。
/// item 的生命周期由 Rust ownership 表达:Core 还显示这个结果,payload 就活着;
/// 不需要全局 `HashMap<ItemId, Entry>` 查表,没有清理时机问题,没有 UI 线程锁。
#[derive(Clone)]
pub struct ModuleItem {
    id: ItemId,
    payload: Arc<dyn Any + Send + Sync>,
}

impl ModuleItem {
    pub fn new<T>(id: ItemId, payload: T) -> Self
    where
        T: Any + Send + Sync + 'static,
    {
        Self {
            id,
            payload: Arc::new(payload),
        }
    }

    pub fn id(&self) -> ItemId {
        self.id
    }

    /// 仅供创建本 item 的 Module 使用。Core 调用它是架构违规。
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.payload.downcast_ref::<T>()
    }
}

impl fmt::Debug for ModuleItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModuleItem").field("id", &self.id).finish()
    }
}
