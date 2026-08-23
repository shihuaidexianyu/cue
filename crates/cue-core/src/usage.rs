use cue_protocol::{ActionId, ModuleId, UsageRead, UsageReader, UsageRecordRequest, UsageStat};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

type UsageKey = (ModuleId, String, ActionId);

/// 聚合 Usage store(Phase 4 起带持久化)。
///
/// V1 只存 `count + last_used`,不存 event log——排序只需要
/// frequency + recency,这是刻意取舍。
///
/// 这是 Core 中唯一共享加锁的状态:Module 的后台 Future 也会
/// 读取它做 ranking,因此内部用 RwLock;Core 其余状态一律不加锁。
///
/// 持久化:整体重写 + tmp rename。记录频率 = 用户启动应用的节奏,
/// 聚合后文件有界(每行一条统计),写穿比 flush 调度更简单且抗崩溃。
/// 格式(制表符分隔,item_key 转义后永远最后一个字段):
///
/// ```text
/// cue-usage-v1
/// <module_id>\t<action>\t<count>\t<unix_secs>\t<item_key>
/// ```
///
/// 文件是外部数据:坏行跳过、头不匹配整个忽略、IO 失败
/// 只告警——usage 丢失永远不构成启动/运行失败。
#[derive(Clone, Default)]
pub struct UsageStore {
    inner: Arc<RwLock<HashMap<UsageKey, UsageStat>>>,
    /// None = 纯内存(测试)。
    file: Option<PathBuf>,
}

const HEADER: &str = "cue-usage-v1";

impl UsageStore {
    pub fn new(file: Option<PathBuf>) -> Self {
        let map = file.as_deref().and_then(parse_file).unwrap_or_default();
        Self {
            inner: Arc::new(RwLock::new(map)),
            file,
        }
    }

    /// activation 完成时调用(usage 总是记录)。
    pub fn record(&self, module: &ModuleId, req: &UsageRecordRequest) {
        {
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
        self.persist();
    }

    pub fn stat(&self, module: &ModuleId, item_key: &str, action: ActionId) -> Option<UsageStat> {
        self.inner
            .read()
            .expect("usage store poisoned")
            .get(&(module.clone(), item_key.to_string(), action))
            .copied()
    }

    /// 绑定 module id 的 reader,随 ModuleContext 发给 Module。
    pub fn reader_for(&self, module: &ModuleId) -> UsageReader {
        Arc::new(ModuleUsageReader {
            module: module.clone(),
            inner: Arc::clone(&self.inner),
        })
    }

    /// 整体重写(文件有界:聚合统计一行一条)。tmp + rename 避免
    /// 半截文件;失败只告警,下次 record 再试。
    fn persist(&self) {
        let Some(path) = &self.file else { return };
        let text = render(&self.inner.read().expect("usage store poisoned"));
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_err() || std::fs::rename(&tmp, path).is_err() {
            eprintln!("[warn] usage persist failed: {}", path.display());
        }
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('\\') => out.push('\\'),
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                _ => return None, // 非法转义:坏行
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

fn render(map: &HashMap<UsageKey, UsageStat>) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for ((module, key, action), stat) in map {
        let secs = stat
            .last_used
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push_str(&format!(
            "{}\t{}\t{}\t{secs}\t{}\n",
            escape(module.as_str()),
            action.0,
            stat.count,
            escape(key),
        ));
    }
    out
}

fn parse_file(path: &Path) -> Option<HashMap<UsageKey, UsageStat>> {
    let text = std::fs::read_to_string(path).ok()?;
    parse(&text)
}

fn parse(text: &str) -> Option<HashMap<UsageKey, UsageStat>> {
    let mut lines = text.lines();
    if lines.next()? != HEADER {
        return None; // 版本不符:整个忽略,不猜格式
    }
    let mut map = HashMap::new();
    for line in lines {
        let mut f = line.split('\t');
        let (Some(m), Some(a), Some(c), Some(t), Some(k), None) =
            (f.next(), f.next(), f.next(), f.next(), f.next(), f.next())
        else {
            continue; // 字段数不对:坏行跳过
        };
        let (Ok(action), Ok(count), Ok(secs)) =
            (a.parse::<u32>(), c.parse::<u64>(), t.parse::<u64>())
        else {
            continue;
        };
        let (Some(module), Some(key)) = (unescape(m), unescape(k)) else {
            continue;
        };
        map.insert(
            (ModuleId::new(module), key, ActionId(action)),
            UsageStat {
                count,
                last_used: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
            },
        );
    }
    Some(map)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(count: u64, secs: u64) -> UsageStat {
        UsageStat {
            count,
            last_used: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
        }
    }

    #[test]
    fn render_parse_roundtrip() {
        let mut map = HashMap::new();
        map.insert(
            (
                ModuleId::from_static("app"),
                "C:\\Apps\\x.exe\u{1f}".to_string(),
                ActionId::PRIMARY,
            ),
            stat(7, 1_700_000_000),
        );
        map.insert(
            // 需要转义的 key:制表符、换行、反斜杠
            (
                ModuleId::from_static("file"),
                "a\tb\nc\\d".to_string(),
                ActionId(3),
            ),
            stat(1, 42),
        );
        let parsed = parse(&render(&map)).unwrap();
        assert_eq!(parsed, map);
    }

    #[test]
    fn parse_skips_bad_lines() {
        let text =
            "cue-usage-v1\napp\t0\t5\t100\tok\napp\t0\tNaN\t100\tbad\napp\t0\t1\nbad-escape\t\\x\n";
        let map = parse(text).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get(&(
                ModuleId::from_static("app"),
                "ok".to_string(),
                ActionId::PRIMARY
            )),
            Some(&stat(5, 100))
        );
    }

    #[test]
    fn parse_rejects_unknown_header() {
        assert!(parse("something-else\napp\t0\t1\t1\tk\n").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn file_roundtrip_and_record_persists() {
        let dir = std::env::temp_dir().join(format!("cue-usage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("usage.tsv");

        let store = UsageStore::new(Some(file.clone()));
        store.record(
            &ModuleId::from_static("app"),
            &UsageRecordRequest {
                item_key: "k1".to_string(),
                action_id: ActionId::PRIMARY,
            },
        );
        store.record(
            &ModuleId::from_static("app"),
            &UsageRecordRequest {
                item_key: "k1".to_string(),
                action_id: ActionId::PRIMARY,
            },
        );
        assert!(file.exists());

        let reloaded = UsageStore::new(Some(file));
        let s = reloaded
            .stat(&ModuleId::from_static("app"), "k1", ActionId::PRIMARY)
            .unwrap();
        assert_eq!(s.count, 2);
        assert!(s.last_used.elapsed().unwrap().as_secs() < 60);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
