//! cue-module-system —— SystemModule(§126)。
//!
//! `>` 触发的系统动作:锁屏/睡眠/休眠/注销/重启/关机/清空回收站。
//! 固定枚举动作(不是 shell runner——不接受任意命令);中文名按
//! 拼音全拼/首字母匹配(7 个静态项,键是手校表,不动 pinyin 引擎),
//! 英文名同义匹配;usage 提升常用动作。空查询列出全部动作。
//!
//! 破坏性分级:重启/关机走 30 秒宽限(InitiateSystemShutdownEx,
//! 原生倒计时,`shutdown /a` 可中止,应用可拒绝);其余立即执行
//! (锁屏/睡眠/休眠/注销都可逆或经正常会话结束流程)。
//! 关机特权在 load 时启用一次(SE_SHUTDOWN_NAME,交互用户默认可得)。

use cue_protocol::*;
use std::sync::Arc;

/// 动作身份(usage key 与 item id 的稳定来源)。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ActionKind {
    Lock,
    Sleep,
    Hibernate,
    Logoff,
    Restart,
    Shutdown,
    RecycleBin,
}

/// 静态动作表的一项。匹配键全部手校:7 个固定中文名,
/// 拼音/首字母/英文别名是已知答案,不需要运行时引擎。
struct ActionSpec {
    /// 稳定身份:usage 的 item_key。
    id: &'static str,
    /// 中文名(标题,也是匹配键)。
    name: &'static str,
    /// 拼音全拼(无调,小写连写)。
    pinyin: &'static str,
    /// 拼音首字母。
    initials: &'static str,
    /// 英文关键词。
    english: &'static str,
    /// 备用匹配键(多音字、常用别名)。
    extras: &'static [&'static str],
    /// 副标题:说明行为与破坏性。
    subtitle: &'static str,
    icon: SystemIconId,
    kind: ActionKind,
}

/// 全部系统动作(表顺序 = 无 usage 时的默认顺序)。
const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        id: "lock",
        name: "锁屏",
        pinyin: "suoping",
        initials: "sp",
        english: "lock",
        extras: &["suojie", "锁定"],
        subtitle: "立即锁定当前会话",
        icon: SystemIconId::Lock,
        kind: ActionKind::Lock,
    },
    ActionSpec {
        id: "sleep",
        name: "睡眠",
        pinyin: "shuimian",
        initials: "sm",
        english: "sleep",
        extras: &[],
        subtitle: "进入睡眠(内存保持供电,按电源键唤醒)",
        icon: SystemIconId::Sleep,
        kind: ActionKind::Sleep,
    },
    ActionSpec {
        id: "hibernate",
        name: "休眠",
        pinyin: "xiumian",
        initials: "xm",
        english: "hibernate",
        extras: &[],
        subtitle: "内存写入磁盘后断电;仅在本机启用休眠时出现",
        icon: SystemIconId::Hibernate,
        kind: ActionKind::Hibernate,
    },
    ActionSpec {
        id: "logoff",
        name: "注销",
        pinyin: "zhuxiao",
        initials: "zx",
        english: "logoff",
        extras: &["logout", "signout"],
        subtitle: "退出当前账户(程序有机会正常退出)",
        icon: SystemIconId::Logoff,
        kind: ActionKind::Logoff,
    },
    ActionSpec {
        id: "restart",
        name: "重启",
        // 电脑重启读 chóngqǐ;zhòngqǐ 是常见误读,放 extras。
        pinyin: "chongqi",
        initials: "cq",
        english: "restart",
        extras: &["zhongqi", "zq", "reboot"],
        subtitle: "30 秒后重启(原生倒计时;shutdown /a 可取消)",
        icon: SystemIconId::Restart,
        kind: ActionKind::Restart,
    },
    ActionSpec {
        id: "shutdown",
        name: "关机",
        pinyin: "guanji",
        initials: "gj",
        english: "shutdown",
        extras: &["poweroff"],
        subtitle: "30 秒后关机(原生倒计时;shutdown /a 可取消)",
        icon: SystemIconId::Shutdown,
        kind: ActionKind::Shutdown,
    },
    ActionSpec {
        id: "recyclebin",
        name: "清空回收站",
        pinyin: "qingkonghuishouzhan",
        initials: "qkhsz",
        english: "empty recycle bin",
        extras: &[
            "recycle",
            "recyclebin",
            "trash",
            "huishouzhan",
            "hsz",
            "回收站",
        ],
        subtitle: "永久删除回收站全部内容",
        icon: SystemIconId::RecycleBin,
        kind: ActionKind::RecycleBin,
    },
];

/// 单键得分:完全相等 > 前缀 > 子串。
fn score_key(key: &str, q: &str) -> Option<i32> {
    if q.is_empty() {
        return None;
    }
    if key == q {
        Some(120)
    } else if key.starts_with(q) {
        Some(100)
    } else if key.contains(q) {
        Some(60)
    } else {
        None
    }
}

/// 一个动作对查询的最佳得分(所有匹配键取最大)。
fn spec_score(spec: &ActionSpec, q: &str) -> Option<i32> {
    [spec.name, spec.pinyin, spec.initials, spec.english]
        .into_iter()
        .chain(spec.extras.iter().copied())
        .filter_map(|k| score_key(k, q))
        .max()
}

/// usage 加分:次数封顶 25,7 天内用过再 +15(上限 40 = 匹配
/// 等级差 40,最差也只是追平前缀命中、由表序决胜——usage 重排
/// 同级匹配,但压不过更强的匹配)。
fn usage_bonus(usage: Option<&UsageReader>, item_key: &str) -> i32 {
    let Some(stat) = usage.and_then(|u| u.stat(item_key, ActionId::PRIMARY)) else {
        return 0;
    };
    let recency = stat
        .last_used
        .elapsed()
        .map(|d| d.as_secs() < 7 * 24 * 3600)
        .unwrap_or(false);
    stat.count.min(25) as i32 + if recency { 15 } else { 0 }
}

/// 纯查询逻辑(可测):空查询列全部(usage 在前),非空按匹配分
/// + usage 加分排序。休眠按能力探测过滤。
fn search(
    usage: Option<&UsageReader>,
    hibernate_available: bool,
    query: &str,
    limit: usize,
) -> Vec<ModuleItem> {
    let catalog = ACTIONS
        .iter()
        .filter(|s| s.kind != ActionKind::Hibernate || hibernate_available);
    let q = query.trim().to_lowercase();
    let mut scored: Vec<(&ActionSpec, i32)> = if q.is_empty() {
        // 空查询:全部列出;usage 加分排序,没用过的(0 分)
        // 保持表序(stable sort)。
        let mut v: Vec<(&ActionSpec, i32)> =
            catalog.map(|s| (s, usage_bonus(usage, s.id))).collect();
        v.sort_by_key(|(_, s)| std::cmp::Reverse(*s));
        v
    } else {
        let mut v: Vec<(&ActionSpec, i32)> = catalog
            .filter_map(|s| spec_score(s, &q).map(|sc| (s, sc + usage_bonus(usage, s.id))))
            .collect();
        v.sort_by_key(|(_, s)| std::cmp::Reverse(*s));
        v
    };
    scored.truncate(limit);
    scored
        .into_iter()
        .map(|(s, _)| {
            // ItemId 稳定:表内序号 + 1。
            let idx = ACTIONS.iter().position(|a| a.id == s.id).unwrap() as u64;
            ModuleItem::new(ItemId(idx + 1), s)
        })
        .collect()
}

/// 系统动作模块。无设置、无存储;usage 只读(load 注入)。
pub struct SystemModule {
    desc: ModuleDescriptor,
    usage: Option<UsageReader>,
    /// 本机是否可休眠(load 探测;不可休眠则不显示"休眠"动作)。
    hibernate_available: bool,
}

impl SystemModule {
    pub fn new() -> Self {
        Self {
            desc: ModuleDescriptor {
                id: ModuleId::from_static("system"),
                name: "系统动作",
                version: "0.1.0",
            },
            usage: None,
            hibernate_available: true,
        }
    }
}

impl Default for SystemModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for SystemModule {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.desc
    }

    fn load(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.usage = Some(ctx.usage.clone());
        self.hibernate_available = exec::hibernate_available();
        if !exec::enable_shutdown_privilege() {
            ctx.logger.log(
                LogLevel::Warn,
                "system: 启用关机特权失败,睡眠/注销/重启/关机可能不可用",
            );
        }
        Ok(())
    }

    fn unload(&mut self) {
        self.usage = None;
    }

    fn settings_schema(&self) -> SettingsSchema {
        Vec::new()
    }

    fn try_apply_settings(&mut self, _changes: SettingsChangeSet) -> Result<(), ModuleError> {
        Ok(())
    }
}

impl LauncherModule for SystemModule {
    fn launcher_descriptor(&self) -> LauncherDescriptor {
        LauncherDescriptor {
            trigger: Some(">".into()),
            is_default: false,
        }
    }

    fn query(&mut self, ctx: QueryContext) -> QueryFuture {
        let usage = self.usage.clone();
        let hibernate = self.hibernate_available;
        Box::pin(async move {
            let items = search(usage.as_ref(), hibernate, &ctx.query, ctx.result_limit);
            Ok(QueryResponse { items })
        })
    }

    fn present(&self, item: &ModuleItem) -> ResultPresentation {
        let Some(spec) = item.downcast_ref::<&'static ActionSpec>() else {
            return ResultPresentation::new("?");
        };
        let mut p = ResultPresentation::new(spec.name);
        p.subtitle = Some(Arc::from(spec.subtitle));
        p.icon = Some(ResultIcon::SystemIcon(spec.icon));
        p
    }

    fn actions(&self, _item: &ModuleItem) -> Vec<ActionDescriptor> {
        vec![ActionDescriptor {
            id: ActionId::PRIMARY,
            label: Arc::from("执行"),
            shortcut: None,
        }]
    }

    fn activate(&mut self, item: &ModuleItem, action: ActionId) -> ActivationFuture {
        let spec = item.downcast_ref::<&'static ActionSpec>().copied();
        Box::pin(async move {
            let Some(spec) = spec else {
                return ModuleOutcome::failed(ModuleError::ActivationFailed("system: 未知条目".into()));
            };
            if action != ActionId::PRIMARY {
                return ModuleOutcome::failed(ModuleError::ActivationFailed("system: 未知动作".into()));
            }
            match exec::run(spec.kind) {
                Ok(()) => ModuleOutcome::success(
                    SessionDisposition::Close,
                    Some(UsageRecordRequest {
                        item_key: spec.id.to_string(),
                        action_id: ActionId::PRIMARY,
                    }),
                ),
                Err(e) => ModuleOutcome::failed(e),
            }
        })
    }
}

// ------------------------------------------------------------------
// 执行层:全部 Win32 调用,薄封装,失败归 ModuleError 不 panic。
// 测试不触碰这层(查询逻辑是纯函数)。
// ------------------------------------------------------------------

#[cfg(windows)]
mod exec {
    use super::ActionKind;
    use cue_protocol::ModuleError;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
        SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
    };
    use windows::Win32::System::Power::{
        GetPwrCapabilities, SYSTEM_POWER_CAPABILITIES, SetSuspendState,
    };
    use windows::Win32::System::Shutdown::{
        EWX_LOGOFF, ExitWindowsEx, InitiateSystemShutdownExW, LockWorkStation,
        SHTDN_REASON_MINOR_OTHER,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::Win32::UI::Shell::{
        SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHEmptyRecycleBinW,
    };
    use windows::core::{PCWSTR, w};

    fn failed(api: &str, e: windows::core::Error) -> ModuleError {
        ModuleError::ActivationFailed(format!("{api} 失败:{e}"))
    }

    pub fn run(kind: ActionKind) -> Result<(), ModuleError> {
        match kind {
            ActionKind::Lock => lock(),
            ActionKind::Sleep => suspend(false),
            ActionKind::Hibernate => suspend(true),
            ActionKind::Logoff => logoff(),
            ActionKind::Restart => shutdown(true),
            ActionKind::Shutdown => shutdown(false),
            ActionKind::RecycleBin => empty_recycle_bin(),
        }
    }

    /// 交互用户令牌自带 SeShutdownPrivilege,但默认禁用;启用一次,
    /// 进程内长期有效。返回是否成功(失败仅告警,执行时报具体错)。
    pub fn enable_shutdown_privilege() -> bool {
        unsafe {
            let mut token = std::mem::zeroed();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token).is_err() {
                return false;
            }
            let ok: windows::core::Result<()> = (|| {
                let mut luid = std::mem::zeroed();
                LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid)?;
                let tp = TOKEN_PRIVILEGES {
                    PrivilegeCount: 1,
                    Privileges: [LUID_AND_ATTRIBUTES {
                        Luid: luid,
                        Attributes: SE_PRIVILEGE_ENABLED,
                    }],
                };
                AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None)
            })();
            let _ = CloseHandle(token);
            ok.is_ok()
        }
    }

    /// 本机能否休眠(S4 + 休眠文件在);决定"休眠"动作是否出现。
    pub fn hibernate_available() -> bool {
        unsafe {
            let mut caps = SYSTEM_POWER_CAPABILITIES::default();
            if !GetPwrCapabilities(&mut caps) {
                return false;
            }
            caps.SystemS4 && caps.HiberFilePresent
        }
    }

    fn lock() -> Result<(), ModuleError> {
        unsafe { LockWorkStation() }.map_err(|e| failed("LockWorkStation", e))
    }

    fn suspend(hibernate: bool) -> Result<(), ModuleError> {
        if unsafe { SetSuspendState(hibernate, false, false) } {
            Ok(())
        } else {
            Err(ModuleError::ActivationFailed(
                "SetSuspendState 失败(可能被电源策略拒绝)".into(),
            ))
        }
    }

    fn logoff() -> Result<(), ModuleError> {
        unsafe { ExitWindowsEx(EWX_LOGOFF, SHTDN_REASON_MINOR_OTHER) }
            .map_err(|e| failed("ExitWindowsEx", e))
    }

    /// 30 秒宽限:原生倒计时对话框,期间 `shutdown /a` 可中止;
    /// 不强制关应用(应用可提示保存/拒绝,等于第二道保险)。
    fn shutdown(reboot: bool) -> Result<(), ModuleError> {
        let msg = if reboot {
            w!("CUE:30 秒后重启。取消请运行 shutdown /a")
        } else {
            w!("CUE:30 秒后关机。取消请运行 shutdown /a")
        };
        unsafe {
            InitiateSystemShutdownExW(
                PCWSTR::null(),
                msg,
                30,
                false,
                reboot,
                SHTDN_REASON_MINOR_OTHER,
            )
        }
        .map_err(|e| failed("InitiateSystemShutdownEx", e))
    }

    fn empty_recycle_bin() -> Result<(), ModuleError> {
        unsafe {
            SHEmptyRecycleBinW(
                None,
                PCWSTR::null(),
                SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND,
            )
        }
        .map_err(|e| failed("SHEmptyRecycleBin", e))
    }
}

#[cfg(not(windows))]
mod exec {
    use super::ActionKind;
    use cue_protocol::ModuleError;

    pub fn run(_kind: ActionKind) -> Result<(), ModuleError> {
        Err(ModuleError::Unavailable("system: 仅支持 Windows".into()))
    }
    pub fn enable_shutdown_privilege() -> bool {
        false
    }
    pub fn hibernate_available() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::SystemTime;

    struct FakeUsage(HashMap<String, UsageStat>);
    impl UsageRead for FakeUsage {
        fn stat(&self, item_key: &str, _action: ActionId) -> Option<UsageStat> {
            self.0.get(item_key).copied()
        }
    }

    fn names(items: &[ModuleItem]) -> Vec<&'static str> {
        items
            .iter()
            .map(|i| i.downcast_ref::<&'static ActionSpec>().unwrap().name)
            .collect()
    }

    fn stat(count: u64) -> UsageStat {
        UsageStat {
            count,
            last_used: SystemTime::now(),
        }
    }

    /// 拼音首字母 / 全拼 / 英文 / 别名都能命中对应动作。
    #[test]
    fn match_by_pinyin_initials_english_and_alias() {
        let no_usage: Option<&UsageReader> = None;
        let first = |q: &str| names(&search(no_usage, true, q, 10)).first().copied();
        assert_eq!(first("gj"), Some("关机"));
        assert_eq!(first("guanji"), Some("关机"));
        assert_eq!(first("shutdown"), Some("关机"));
        assert_eq!(first("sp"), Some("锁屏"));
        assert_eq!(first("lock"), Some("锁屏"));
        // 多音字两条路都通
        assert_eq!(first("cq"), Some("重启"));
        assert_eq!(first("zq"), Some("重启"));
        assert_eq!(first("reboot"), Some("重启"));
        assert_eq!(first("hsz"), Some("清空回收站"));
        assert_eq!(first("回收站"), Some("清空回收站"));
    }

    /// 空查询列出全部动作;usage 把常用项提前,未用过的保持表序。
    #[test]
    fn empty_query_lists_all_usage_first() {
        let no_usage: Option<&UsageReader> = None;
        let all = search(no_usage, true, "", 10);
        assert_eq!(all.len(), ACTIONS.len());
        assert_eq!(names(&all)[..2], ["锁屏", "睡眠"]);

        let usage: UsageReader = Arc::new(FakeUsage(HashMap::from([(
            "shutdown".to_string(),
            stat(9),
        )])));
        let all = search(Some(&usage), true, "", 10);
        assert_eq!(names(&all)[0], "关机");
        assert_eq!(names(&all)[1], "锁屏"); // 其余保持表序
    }

    /// usage 加分重排同级匹配,但压不过更强的匹配:满 usage 的
    /// 子串命中最多追平前缀命中,由表序决胜(锁屏在表内更前)。
    #[test]
    fn usage_bonus_boosts_without_distorting() {
        let usage: UsageReader = Arc::new(FakeUsage(HashMap::from([(
            "recyclebin".to_string(),
            stat(50),
        )])));
        let r = search(Some(&usage), true, "s", 10);
        // s 前缀命中 锁屏/睡眠(initials);回收站是子串命中 + 满 usage
        assert_eq!(names(&r)[0], "锁屏");
        // 同级(都前缀)时 usage 决胜:给睡眠加 usage 后睡眠在前
        let usage2: UsageReader =
            Arc::new(FakeUsage(HashMap::from([("sleep".to_string(), stat(2))])));
        let r2 = search(Some(&usage2), true, "s", 10);
        assert_eq!(names(&r2)[0], "睡眠");
    }

    /// 不可休眠的机器上不出现"休眠"。
    #[test]
    fn hibernate_filtered_when_unavailable() {
        let no_usage: Option<&UsageReader> = None;
        assert!(names(&search(no_usage, true, "", 10)).contains(&"休眠"));
        assert!(!names(&search(no_usage, false, "", 10)).contains(&"休眠"));
        assert!(names(&search(no_usage, false, "xm", 10)).is_empty());
    }

    /// item id 稳定(usage 与重排都不影响身份)。
    #[test]
    fn item_ids_are_stable() {
        let no_usage: Option<&UsageReader> = None;
        let a = search(no_usage, true, "gj", 10);
        let usage: UsageReader = Arc::new(FakeUsage(HashMap::from([(
            "shutdown".to_string(),
            stat(3),
        )])));
        let b = search(Some(&usage), true, "", 10);
        let id_a = a[0].id();
        let id_b = b
            .iter()
            .find(|i| i.downcast_ref::<&'static ActionSpec>().unwrap().id == "shutdown")
            .unwrap()
            .id();
        assert_eq!(id_a, id_b);
    }

    /// present 出自 payload:标题中文名、副标题说明、动作图标;
    /// 动作菜单只有 PRIMARY"执行"。
    #[test]
    fn present_and_actions() {
        let no_usage: Option<&UsageReader> = None;
        let items = search(no_usage, true, "gj", 10);
        let m = SystemModule::new();
        let p = m.present(&items[0]);
        assert_eq!(&*p.title, "关机");
        assert!(p.subtitle.as_deref().unwrap().contains("30 秒"));
        assert!(matches!(
            p.icon,
            Some(ResultIcon::SystemIcon(SystemIconId::Shutdown))
        ));
        let acts = m.actions(&items[0]);
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].id, ActionId::PRIMARY);
    }

    /// 触发词是 `>`,非默认模块;无匹配时为空而不是兜底推荐。
    #[test]
    fn descriptor_and_no_match() {
        let m = SystemModule::new();
        let d = m.launcher_descriptor();
        assert_eq!(d.trigger.as_deref(), Some(">"));
        assert!(!d.is_default);
        let no_usage: Option<&UsageReader> = None;
        assert!(search(no_usage, true, "zzz不存在", 10).is_empty());
    }
}
