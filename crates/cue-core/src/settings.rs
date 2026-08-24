//! Settings Host:设置的唯一所有者。
//!
//! 职责边界:本模块只管"规格表 + 当前值 + 持久化 + core.* 的
//! try-apply 回调";事务编排(校验 → try-apply → commit → persist)
//! 由 Core::apply_setting 驱动,module.* 的 try-apply 由 Core
//! 经 registry 调 `Module::try_apply_settings`。
//!
//! 持久化:`<storage_root>/settings.tsv`,版本头 + 整体重写 + tmp
//! rename(与 usage 同一规则):坏行跳过、头不符整个忽略、IO 失败
//! 仅告警——设置文件损坏永远不构成启动失败。

use cue_protocol::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// 同步例外:core.hotkey 的 try-apply 由 Core 直接调用
/// Host 注入的这个函数(先注册新的,成功再注销旧的)。是函数,不是
/// HostPlatform trait。Core 是 UI 线程单线程状态机,
/// 回调只在 UI 线程被调用,不要求 Send。
pub type ApplyHotkey = Box<dyn FnMut(&Hotkey) -> Result<(), String>>;

/// core.start_on_boot 的 try-apply 回调(写登录启动项),与 ApplyHotkey
/// 同一模式:Host 注入、UI 线程调用、失败不 commit。
pub type ApplyStartOnBoot = Box<dyn FnMut(bool) -> Result<(), String>>;

/// Path 类设置行的"打开"动作:Host 注入的同步回调(用系统默认程序
/// 打开该路径——名单、日志这类给人看/给人改的文件)。与 try-apply
/// 无关:不是值变更,不 commit;同一模式,函数不是 trait,只在
/// UI 线程调用。
pub type OpenPath = Box<dyn FnMut(&std::path::Path) -> Result<(), String>>;

/// core.dnd_mode 的 commit 后通知(托盘状态图标,§127):Host 注入、
/// UI 线程调用、无返回值——通知不会失败,不参与事务(与 try-apply
/// 不同)。Core::new 以初始值调一次,此后每次成功 commit 调一次;
/// 重复同值 commit 也会通知(host 侧换图标幂等,无需 Core 去重)。
pub type NotifyDndMode = Box<dyn FnMut(bool)>;

const HEADER: &str = "cue-settings-v1";

pub const KEY_HOTKEY: &str = "core.hotkey";
pub const KEY_HIDE_ON_FOCUS_LOSS: &str = "core.hide_on_focus_loss";
pub const KEY_START_ON_BOOT: &str = "core.start_on_boot";
pub const KEY_DND_MODE: &str = "core.dnd_mode";
/// 旧 key(§127,2026-08-24 更名免打扰模式前):read_persisted 读入时
/// 映射到新 key,下次整体重写时旧 key 自愈消失。
const KEY_GAME_MODE_LEGACY: &str = "core.game_mode";

/// 给 UI 的渲染模型:Core 出模型,cue-ui 只渲染——
/// Module 永远不画 GPUI(禁止 `render_settings_gpui`)。
#[derive(Clone, Debug)]
pub struct SettingsModel {
    pub rows: Vec<SettingsRow>,
    pub selected: usize,
    /// try-apply 失败信息(旧值保留)。
    pub error: Option<Arc<str>>,
    /// RestartApplication:有改动待重启生效。
    pub restart_required: bool,
}

#[derive(Clone, Debug)]
pub struct SettingsRow {
    pub key: Arc<str>,
    pub label: Arc<str>,
    pub description: Option<Arc<str>>,
    pub kind: SettingKind,
    pub value: SettingValue,
}

/// 设置视图的 Core 侧状态(选中项、上次 apply 错误)。
#[derive(Clone, Debug, Default)]
pub struct SettingsViewState {
    pub selected: usize,
    pub error: Option<Arc<str>>,
}

pub struct SettingsHost {
    specs: Vec<SettingSpec>,
    /// key → 当前已 commit 的值。一定有值(spec 注册时先填 default,
    /// 持久化 overlay,apply 时替换)。
    values: HashMap<String, SettingValue>,
    file: Option<PathBuf>,
    /// 启动时读一次的持久化 overlay,register_specs 复用——
    /// 不为每次注册重读 settings.tsv(§77 冷启动路径)。
    persisted: HashMap<String, String>,
    /// Core 自有的合成行(§128 触发词):不进模块的设置快照。
    core_owned_keys: std::collections::HashSet<String>,
    apply_hotkey: Option<ApplyHotkey>,
    apply_start_on_boot: Option<ApplyStartOnBoot>,
    open_path: Option<OpenPath>,
    restart_required: bool,
}

impl SettingsHost {
    pub fn new(
        file: Option<PathBuf>,
        apply_hotkey: Option<ApplyHotkey>,
        apply_start_on_boot: Option<ApplyStartOnBoot>,
        open_path: Option<OpenPath>,
    ) -> Self {
        let persisted = Self::read_persisted(file.as_deref());
        let mut host = Self {
            specs: Vec::new(),
            values: HashMap::new(),
            file,
            persisted,
            core_owned_keys: std::collections::HashSet::new(),
            apply_hotkey,
            apply_start_on_boot,
            open_path,
            restart_required: false,
        };
        // core.*:V1 四项,都是 Immediate。
        host.register_specs(core_specs());
        host
    }

    /// 模块 load 之后注册其 schema。key 必须带
    /// `module.<id>.` 前缀;不符的条目跳过(不 panic)。
    pub fn register_module_specs(&mut self, module_id: &ModuleId, schema: SettingsSchema) {
        let prefix = format!("module.{}.", module_id.as_str());
        self.register_specs(
            schema
                .into_iter()
                .filter(|s| s.key.0.starts_with(&prefix))
                .collect(),
        );
    }

    /// Core 合成的触发词设置行(§128)。触发词是 Core 路由状态,
    /// 不属于模块 schema,但放在模块命名空间下展示;
    /// default = 模块声明值。返回完整 key 供 Core 登记所有权。
    pub fn register_trigger_spec(
        &mut self,
        module_id: &ModuleId,
        module_name: &str,
        default_trigger: &str,
    ) -> String {
        let key = format!("module.{}.trigger", module_id.as_str());
        self.core_owned_keys.insert(key.clone());
        self.register_specs(vec![SettingSpec {
            key: SettingKey(Arc::from(key.as_str())),
            label: format!("{module_name} · 触发词").into(),
            description: Some(
                "在输入开头键入它进入该模块;以字母/数字结尾时后面需跟空格(如 `b github`),标点类直接前缀匹配(如 `/路径`)"
                    .into(),
            ),
            kind: SettingKind::String,
            default: SettingValue::String(default_trigger.to_string()),
            apply_policy: ApplyPolicy::Immediate,
        }]);
        key
    }

    fn register_specs(&mut self, specs: Vec<SettingSpec>) {
        for spec in specs {
            let key = spec.key.0.to_string();
            // first-wins:同 key 重复注册(模块 schema 撞上 Core 合成行,
            // 或模块自身重复)只留第一份——设置页不出双行,spec() 查找
            // 结果确定。
            if self.specs.iter().any(|s| s.key.0.as_ref() == key) {
                eprintln!("[warn] duplicate setting key skipped: {key}");
                continue;
            }
            let value = self
                .persisted
                .get(&key)
                .and_then(|raw| decode_value(spec.kind, raw))
                .unwrap_or_else(|| spec.default.clone());
            self.values.insert(key, value);
            self.specs.push(spec);
        }
    }

    pub fn spec(&self, key: &str) -> Option<&SettingSpec> {
        self.specs.iter().find(|s| s.key.0.as_ref() == key)
    }

    pub fn value(&self, key: &str) -> Option<&SettingValue> {
        self.values.get(key)
    }

    pub fn hotkey(&self) -> Hotkey {
        match self.values.get(KEY_HOTKEY) {
            Some(SettingValue::Hotkey(h)) => *h,
            _ => Hotkey::default(),
        }
    }

    pub fn hide_on_focus_loss(&self) -> bool {
        match self.values.get(KEY_HIDE_ON_FOCUS_LOSS) {
            Some(SettingValue::Bool(b)) => *b,
            _ => true,
        }
    }

    pub fn start_on_boot(&self) -> bool {
        match self.values.get(KEY_START_ON_BOOT) {
            Some(SettingValue::Bool(b)) => *b,
            _ => false,
        }
    }

    pub fn dnd_mode(&self) -> bool {
        match self.values.get(KEY_DND_MODE) {
            Some(SettingValue::Bool(b)) => *b,
            _ => true,
        }
    }

    /// 模块设置快照(ModuleContext.settings):短 key(去掉
    /// `module.<id>.` 前缀)——模块知道自己的 id,不需要全限定名。
    /// Core 自有的合成行(§128 触发词)不进快照:它不是模块 schema。
    pub fn values_for_module(&self, module_id: &ModuleId) -> ModuleSettings {
        let prefix = format!("module.{}.", module_id.as_str());
        let map = self
            .values
            .iter()
            .filter(|(k, _)| !self.core_owned_keys.contains(*k))
            .filter_map(|(k, v)| {
                k.strip_prefix(&prefix)
                    .map(|short| (short.to_string(), v.clone()))
            })
            .collect();
        ModuleSettings::new(map)
    }

    /// Immediate 的 core.hotkey try-apply:同步回调,失败不 commit。
    pub fn try_apply_hotkey(&mut self, hotkey: &Hotkey) -> Result<(), String> {
        match self.apply_hotkey.as_mut() {
            Some(f) => f(hotkey),
            // 无回调(测试环境):try-apply 视为通过。
            None => Ok(()),
        }
    }

    /// core.start_on_boot try-apply:写登录启动项,失败不 commit。
    pub fn try_apply_start_on_boot(&mut self, on: bool) -> Result<(), String> {
        match self.apply_start_on_boot.as_mut() {
            Some(f) => f(on),
            None => Ok(()),
        }
    }

    /// Path 行激活 = 用系统默认程序打开当前路径值。只读值、校验
    /// kind,然后交给 Host 回调——不是设置变更,不 commit、不 persist。
    pub fn open_path(&mut self, key: &str) -> Result<(), String> {
        let spec = self
            .spec(key)
            .ok_or_else(|| format!("unknown setting: {key}"))?;
        if spec.kind != SettingKind::Path {
            return Err(format!("{key} 不是 Path 类设置"));
        }
        let Some(SettingValue::Path(path)) = self.values.get(key) else {
            return Err(format!("{key} 没有路径值"));
        };
        let path = path.clone();
        match self.open_path.as_mut() {
            Some(f) => f(&path),
            // 无回调(测试环境):视为成功。
            None => Ok(()),
        }
    }

    /// 仅改内存中的已 commit 值,不写盘:Core 对自愈类值(手工
    /// 改坏的空触发词)的归一用——随下一次事务的整体重写落盘。
    pub fn set_value_no_persist(&mut self, key: &str, value: SettingValue) {
        self.values.insert(key.to_string(), value);
    }

    /// commit + persist(顺序的最后两步,永远一起)。
    pub fn commit(&mut self, key: &str, value: SettingValue) {
        self.values.insert(key.to_string(), value);
        self.persist();
    }

    pub fn mark_restart_required(&mut self) {
        self.restart_required = true;
    }

    pub fn restart_required(&self) -> bool {
        self.restart_required
    }

    pub fn model(&self, view: &SettingsViewState) -> SettingsModel {
        SettingsModel {
            rows: self
                .specs
                .iter()
                .map(|spec| SettingsRow {
                    key: spec.key.0.clone(),
                    label: spec.label.clone(),
                    description: spec.description.clone(),
                    kind: spec.kind,
                    value: self
                        .values
                        .get(spec.key.0.as_ref())
                        .cloned()
                        .unwrap_or_else(|| spec.default.clone()),
                })
                .collect(),
            selected: view.selected.min(self.specs.len().saturating_sub(1)),
            error: view.error.clone(),
            restart_required: self.restart_required,
        }
    }

    pub fn row_count(&self) -> usize {
        self.specs.len()
    }

    // ------------------------------------------------------------------
    // 持久化(版本头 + 整体重写 + tmp rename,与 usage 同一规则)
    // ------------------------------------------------------------------

    fn read_persisted(path: Option<&std::path::Path>) -> HashMap<String, String> {
        let mut out = HashMap::new();
        let Some(path) = path else { return out };
        let Ok(text) = std::fs::read_to_string(path) else {
            return out;
        };
        let mut lines = text.lines();
        if lines.next() != Some(HEADER) {
            return out; // 版本不符:整个忽略
        }
        for line in lines {
            let Some((k, v)) = line.split_once('\t') else {
                continue; // 坏行跳过
            };
            // 更名迁移:core.game_mode → core.dnd_mode。新旧并存时
            // 新 key 赢(与行序无关);旧 key 不写回,自愈。
            if k == KEY_GAME_MODE_LEGACY {
                if !out.contains_key(KEY_DND_MODE) {
                    out.insert(KEY_DND_MODE.to_string(), v.to_string());
                }
            } else {
                out.insert(k.to_string(), v.to_string());
            }
        }
        out
    }

    fn persist(&self) {
        let Some(path) = &self.file else { return };
        let mut text = String::from(HEADER);
        text.push('\n');
        for (k, v) in &self.values {
            // key 由代码构造(命名空间标识符),不含制表符;value 编码后亦然。
            text.push_str(&format!("{k}\t{}\n", encode_value(v)));
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_err() || std::fs::rename(&tmp, path).is_err() {
            eprintln!("[warn] settings persist failed: {}", path.display());
        }
    }
}

/// Core settings(V1)。
fn core_specs() -> Vec<SettingSpec> {
    vec![
        SettingSpec {
            key: SettingKey(Arc::from(KEY_HOTKEY)),
            label: "全局热键".into(),
            description: Some("唤起 / 隐藏 Launcher(toggle 语义固定)".into()),
            kind: SettingKind::Hotkey,
            default: SettingValue::Hotkey(Hotkey::default()),
            apply_policy: ApplyPolicy::Immediate,
        },
        SettingSpec {
            key: SettingKey(Arc::from(KEY_HIDE_ON_FOCUS_LOSS)),
            label: "失焦隐藏".into(),
            description: Some("前台焦点离开时自动隐藏窗口;关闭后失焦仅退出聚焦态".into()),
            kind: SettingKind::Bool,
            default: SettingValue::Bool(true),
            apply_policy: ApplyPolicy::Immediate,
        },
        SettingSpec {
            key: SettingKey(Arc::from(KEY_START_ON_BOOT)),
            label: "开机自启".into(),
            description: Some("登录 Windows 时自动启动 CUE(写入当前用户的 Run 注册表项)".into()),
            kind: SettingKind::Bool,
            default: SettingValue::Bool(false),
            apply_policy: ApplyPolicy::Immediate,
        },
        SettingSpec {
            key: SettingKey(Arc::from(KEY_DND_MODE)),
            label: "免打扰模式:全屏时不唤起".into(),
            description: Some(
                "前台是全屏应用(游戏、全屏视频)时热键静默失效;只拦截热键唤起,托盘/第二实例照常"
                    .into(),
            ),
            kind: SettingKind::Bool,
            default: SettingValue::Bool(true),
            apply_policy: ApplyPolicy::Immediate,
        },
    ]
}

fn encode_value(v: &SettingValue) -> String {
    match v {
        SettingValue::Bool(b) => b.to_string(),
        SettingValue::Integer(i) => i.to_string(),
        SettingValue::Hotkey(h) => h.to_string(),
        SettingValue::String(s) | SettingValue::Enum(s) => escape(s),
        SettingValue::Path(p) => escape(&p.to_string_lossy()),
    }
}

fn decode_value(kind: SettingKind, raw: &str) -> Option<SettingValue> {
    Some(match kind {
        SettingKind::Bool => SettingValue::Bool(raw == "true"),
        SettingKind::Integer => SettingValue::Integer(raw.parse().ok()?),
        SettingKind::Hotkey => SettingValue::Hotkey(raw.parse().ok()?),
        SettingKind::String => SettingValue::String(unescape(raw)?),
        SettingKind::Enum => SettingValue::Enum(unescape(raw)?),
        SettingKind::Path => SettingValue::Path(PathBuf::from(unescape(raw)?)),
    })
}

/// Bool 的非 "true" 一律解析为 false(宽松);其余类型严格解析失败 → None。
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
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
                _ => return None,
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn persisted_value_overrides_default() {
        let dir = std::env::temp_dir().join(format!("cue-settings-test-{}", std::process::id()));
        let file = dir.join("settings.tsv");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &file,
            "cue-settings-v1\ncore.hotkey\tctrl+alt+k\ncore.hide_on_focus_loss\tfalse\n",
        )
        .unwrap();

        let host = SettingsHost::new(Some(file.clone()), None, None, None);
        assert_eq!(host.hotkey(), Hotkey::from_str("ctrl+alt+k").unwrap());
        assert!(!host.hide_on_focus_loss());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("cue-settings-test2-{}", std::process::id()));
        let file = dir.join("settings.tsv");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, "garbage\ncore.hotkey\tnot-a-hotkey\nbadline\n").unwrap();

        let host = SettingsHost::new(Some(file.clone()), None, None, None);
        assert_eq!(host.hotkey(), Hotkey::default());
        assert!(host.hide_on_focus_loss());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_game_mode_key_migrates_to_dnd_mode() {
        let dir =
            std::env::temp_dir().join(format!("cue-settings-test4-{}", std::process::id()));
        let file = dir.join("settings.tsv");
        std::fs::create_dir_all(&dir).unwrap();

        // 旧 key 读入即映射为新 key。
        std::fs::write(&file, "cue-settings-v1\ncore.game_mode\tfalse\n").unwrap();
        let host = SettingsHost::new(Some(file.clone()), None, None, None);
        assert!(!host.dnd_mode());

        // 新旧并存:新 key 赢,与行序无关。
        for text in [
            "cue-settings-v1\ncore.game_mode\tfalse\ncore.dnd_mode\ttrue\n",
            "cue-settings-v1\ncore.dnd_mode\ttrue\ncore.game_mode\tfalse\n",
        ] {
            std::fs::write(&file, text).unwrap();
            let host = SettingsHost::new(Some(file.clone()), None, None, None);
            assert!(host.dnd_mode());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!("cue-settings-test3-{}", std::process::id()));
        let file = dir.join("settings.tsv");

        let mut host = SettingsHost::new(Some(file.clone()), None, None, None);
        host.commit(KEY_HIDE_ON_FOCUS_LOSS, SettingValue::Bool(false));
        drop(host);

        let host = SettingsHost::new(Some(file.clone()), None, None, None);
        assert!(!host.hide_on_focus_loss());
        assert_eq!(host.hotkey(), Hotkey::default()); // 未动的保持默认
        let _ = std::fs::remove_dir_all(&dir);
    }
}
