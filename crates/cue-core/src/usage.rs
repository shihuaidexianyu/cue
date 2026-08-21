use cue_protocol::{ActionId, ModuleId, UsageRead, UsageReader, UsageRecordRequest, UsageStat};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

type UsageKey = (ModuleId, String, ActionId);

/// §50 聚合 Usage store(内存版;持久化随 Phase 4 落地)。
///
/// V1 只存 `count + last_used`,不存 event log——排序只需要
/// frequency + recency(§52),这是刻意取舍。
///
/// 这是 Core 中唯一共享加锁的状态:Module 的后台 Future 也会
/// 读取它做 ranking,因此内部用 RwLock;Core 其余状态一律不加锁(§91)。
#[derive(Clone, Default)]
pub struct UsageStore {
    inner: Arc<RwLock<HashMap<UsageKey, UsageStat>>>,
}

impl UsageStore {
    /// §103:activation 完成时调用(usage 总是记录)。
    pub fn record(&self, module: &ModuleId, req: &UsageRecordRequest) {
        let mut map = self.inner.write().expect("usage store poisoned");
        let stat = map
            .entry((module.clone(), req.item_key.clone(), req.action_id))
            .or_insert(UsageStat {
                count: 0,
                last_used: SystemTime::UNIX_EPOCH,
            });
        stat.count += 1;
        stat.last_used = SystemTime::now();
    }

    pub fn stat(&self, module: &ModuleId, item_key: &str, action: ActionId) -> Option<UsageStat> {
        self.inner
            .read()
            .expect("usage store poisoned")
            .get(&(module.clone(), item_key.to_string(), action))
            .copied()
    }

    /// 绑定 module id 的 reader,随 ModuleContext 发给 Module(§49)。
    pub fn reader_for(&self, module: &ModuleId) -> UsageReader {
        Arc::new(ModuleUsageReader {
            module: module.clone(),
            inner: Arc::clone(&self.inner),
        })
    }
}

struct ModuleUsageReader {
    module: ModuleId,
    inner: Arc<RwLock<HashMap<UsageKey, UsageStat>>>,
}

impl UsageRead for ModuleUsageReader {
    fn stat(&self, item_key: &str, action: ActionId) -> Option<UsageStat> {
        self.inner
            .read()
            .expect("usage store poisoned")
            .get(&(self.module.clone(), item_key.to_string(), action))
            .copied()
    }
}
