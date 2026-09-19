//! 模块共享的平台中立助手(Rule of Three 下沉,§145)。
//!
//! 与 `sakana-util-win` 同一纪律:同一份代码被第三次复制时才下沉,
//! 且只下沉、永不上浮进 Core / protocol。本 crate 不含任何平台
//! 代码,依赖 sakana-protocol 仅为复用 `UsageReader` / `ActionId`
//! 类型,不引入业务语义;排名公式属于模块侧的共享实现,不是
//! Core ↔ Module 契约(那是 protocol 的边界)。

use sakana_protocol::{ActionId, UsageReader};

/// 频率 + 新近度加成。app / bookmark / web 三处逐字复制到第三次
/// (web lib.rs 自述"第三次使用")触发 Rule of Three 下沉(§145),
/// 公式与下沉前完全一致:
///
/// ```text
/// UsageBonus = min(count,20) * 2; 24h 内 +10, 7d 内 +5
/// ```
///
/// `last_used` 在未来(`elapsed()` Err)时只剩频率项。system 模块
/// 刻意不是消费方:§126 的封顶设计(上限 40 = 匹配等级差)形状
/// 不同,不强行参数化统一。
pub fn usage_bonus(usage: Option<&UsageReader>, item_key: &str) -> i32 {
    let Some(stat) = usage.and_then(|u| u.stat(item_key, ActionId::PRIMARY)) else {
        return 0;
    };
    let mut bonus = (stat.count as i32).min(20) * 2;
    if let Ok(elapsed) = stat.last_used.elapsed() {
        let hours = elapsed.as_secs() / 3600;
        if hours < 24 {
            bonus += 10;
        } else if hours < 24 * 7 {
            bonus += 5;
        }
    }
    bonus
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakana_protocol::{UsageRead, UsageStat};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    struct FakeUsage(HashMap<&'static str, UsageStat>);

    impl UsageRead for FakeUsage {
        fn stat(&self, item_key: &str, _action: ActionId) -> Option<UsageStat> {
            self.0.get(item_key).copied()
        }
    }

    fn stat(count: u64, age: Option<Duration>) -> UsageStat {
        UsageStat {
            count,
            last_used: SystemTime::now() - age.unwrap_or(Duration::ZERO),
        }
    }

    #[test]
    fn no_usage_is_zero() {
        assert_eq!(usage_bonus(None, "k"), 0);
        let empty: UsageReader = Arc::new(FakeUsage(HashMap::new()));
        assert_eq!(usage_bonus(Some(&empty), "missing"), 0);
    }

    #[test]
    fn formula_tiers() {
        let mut map = HashMap::new();
        map.insert("now", stat(3, None));
        map.insert("cap", stat(100, None));
        map.insert(
            "two_days",
            stat(5, Some(Duration::from_secs(2 * 24 * 3600))),
        );
        map.insert(
            "eight_days",
            stat(5, Some(Duration::from_secs(8 * 24 * 3600))),
        );
        let usage: UsageReader = Arc::new(FakeUsage(map));
        // min(3,20)*2 + 10(24h 内)。
        assert_eq!(usage_bonus(Some(&usage), "now"), 16);
        // 次数封顶 20 → 40,再 + 10。
        assert_eq!(usage_bonus(Some(&usage), "cap"), 50);
        // 超 24h 但 7 天内 → +5。
        assert_eq!(usage_bonus(Some(&usage), "two_days"), 15);
        // 超 7 天,无新近度加成。
        assert_eq!(usage_bonus(Some(&usage), "eight_days"), 10);
    }

    #[test]
    fn future_last_used_drops_recency() {
        // last_used 在未来(elapsed() Err)→ 只剩频率项。
        let mut map = HashMap::new();
        map.insert(
            "future",
            UsageStat {
                count: 4,
                last_used: SystemTime::now() + Duration::from_secs(60),
            },
        );
        let usage: UsageReader = Arc::new(FakeUsage(map));
        assert_eq!(usage_bonus(Some(&usage), "future"), 8);
    }
}
