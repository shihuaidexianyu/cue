//! 一次性就绪门(冷启动 spike 结论)。
//!
//! 实测(debug):首次发现 6.7 s(WinRT 冷初始化),热路径 0.55 s,
//! 都远超冷启动 < 500 ms 的预算(会阻塞热键注册)。因此 catalog
//! 构建移出 `load()` 热路径,由 module 自有线程一次性完成(module
//! 自行约束资源);查询 future 在门内等待就绪——不阻塞 UI 线程,
//! 过期由 Core 的 ticket 判定丢弃,无需取消机制。

use crate::catalog::AppEntry;
use futures::future::poll_fn;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::task::Waker;

/// 进程生命周期内只 set 一次(无 watcher,启动时唯一一次构建)。
pub struct CatalogCell {
    state: Mutex<State>,
}

struct State {
    entries: Option<Arc<Vec<AppEntry>>>,
    wakers: Vec<Waker>,
}

impl CatalogCell {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                entries: None,
                wakers: Vec::new(),
            }),
        })
    }

    /// 构建线程写入并唤醒所有等待中的 query future。
    /// set 与 wait 共用同一把锁:waker 先于 entries 不可见而入队,
    /// 或反之——不存在丢失唤醒。
    pub fn set(&self, entries: Vec<AppEntry>) {
        let wakers = {
            let mut st = self.state.lock().unwrap();
            st.entries = Some(Arc::new(entries));
            std::mem::take(&mut st.wakers)
        };
        for w in wakers {
            w.wake();
        }
    }

    /// 等待 catalog 就绪。克隆代价 = 一个 Arc。
    pub async fn wait(&self) -> Arc<Vec<AppEntry>> {
        poll_fn(|cx| {
            let mut st = self.state.lock().unwrap();
            match &st.entries {
                Some(entries) => Poll::Ready(Arc::clone(entries)),
                None => {
                    st.wakers.push(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{AppEntry, LaunchTarget};
    use std::path::PathBuf;

    fn entry(name: &str) -> AppEntry {
        AppEntry::new(
            name,
            LaunchTarget::Win32 {
                exe: PathBuf::from(r"C:\apps\x.exe"),
                args: "".into(),
                working_dir: None,
            },
        )
    }

    #[test]
    fn wait_resolves_after_set() {
        let cell = CatalogCell::new();
        let cell2 = Arc::clone(&cell);
        std::thread::spawn(move || cell2.set(vec![entry("a")]));
        let entries = futures::executor::block_on(cell.wait());
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn set_before_wait_is_immediate() {
        let cell = CatalogCell::new();
        cell.set(vec![entry("a"), entry("b")]);
        let entries = futures::executor::block_on(cell.wait());
        assert_eq!(entries.len(), 2);
    }
}
