//! §5.6 Settings Host:设置的唯一所有者(§48)。
//!
//! 职责边界:本模块只管"规格表 + 当前值 + 持久化 + core.* 的
//! try-apply 回调";事务编排(校验 → try-apply → commit → persist,
//! §42)由 Core::apply_setting 驱动,module.* 的 try-apply 由 Core
//! 经 registry 调 `Module::try_apply_settings`。
//!
//! 持久化:`<storage_root>/settings.tsv`,版本头 + 整体重写 + tmp
//! rename(与 usage 同一纪律):坏行跳过、头不符整个忽略、IO 失败
//! 仅告警——设置文件损坏永远不构成启动失败(§63)。

use cue_protocol::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// §53 / §112 同步例外:core.hotkey 的 try-apply 由 Core 直接调用
/// Host 注入的这个函数(先注册新的,成功再注销旧的)。是函数,不是
/// HostPlatform trait(§110)。Core 是 UI 线程单线程状态机(§91),
/// 回调只在 UI 线程被调用,不要求 Send。
pub type ApplyHotkey = Box<dyn FnMut(&Hotkey) -> Result<(), String>>;

/// core.start_on_boot 的 try-apply 回调(写登录启动项),与 ApplyHotkey
/// 同一模式:Host 注入、UI 线程调用、失败不 commit。
pub type ApplyStartOnBoot = Box<dyn FnMut(bool) -> Result<(), String>>;

const HEADER: &str = "cue-settings-v1";

pub const KEY_HOTKEY: &str = "core.hotkey";
pub const KEY_HIDE_ON_FOCUS_LOSS: &str = "core.hide_on_focus_loss";
pub const KEY_START_ON_BOOT: &str = "core.start_on_boot";

/// §41 给 UI 的渲染模型:Core 出模型,cue-ui 只渲染——
/// Module 永远不画 GPUI(禁止 `render_settings_gpui`)。
#[derive(Clone, Debug)]
pub struct SettingsModel {
    pub rows: Vec<SettingsRow>,
    pub selected: usize,
    /// try-apply 失败信息(旧值保留,§42)。
    pub error: Option<Arc<str>>,
    /// §42 RestartApplication:有改动待重启生效。
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
    apply_hotkey: Option<ApplyHotkey>,
    apply_start_on_boot: Option<ApplyStartOnBoot>,
    restart_required: bool,
}

impl SettingsHost {
    pub fn new(
        file: Option<PathBuf>,
        apply_hotkey: Option<ApplyHotkey>,
        apply_start_on_boot: Option<ApplyStartOnBoot>,
    ) -> Self {
        let mut host = Self {
            specs: Vec::new(),
            values: HashMap::new(),
            file,
            apply_hotkey,
            apply_start_on_boot,
            restart_required: false,
        };
        // §36 core.*:V1 三项,都是 Immediate。
        host.register_specs(core_specs());
        host
    }

    /// 模块 load 之后收编其 schema(§37/§40)。key 必须带
    /// `module.<id>.` 前缀;不符的条目跳过(不 panic,§63)。
    pub fn register_module_specs(&mut self, module_id: &ModuleId, schema: SettingsSchema) {
        let prefix = format!("module.{}.", module_id.as_str());
        self.register_specs(
            schema
                .into_iter()
                .filter(|s| s.key.0.starts_with(&prefix))
                .collect(),
        );
    }

    fn register_specs(&mut self, specs: Vec<SettingSpec>) {
        let persisted = self.load_persisted();
        for spec in specs {
            let key = spec.key.0.to_string();
            let value = persisted
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

    /// 模块设置快照(§49 ModuleContext.settings):短 key(去掉
    /// `module.<id>.` 前缀)——模块知道自己的 id,不需要全限定名。
    pub fn values_for_module(&self, module_id: &ModuleId) -> ModuleSettings {
        let prefix = format!("module.{}.", module_id.as_str());
        let map = self
            .values
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix(&prefix)
                    .map(|short| (short.to_string(), v.clone()))
            })
            .collect();
        ModuleSettings::new(map)
    }

    /// §42 Immediate 的 core.hotkey try-apply:同步回调,失败不 commit。
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

    /// commit + persist(§42 顺序的最后两步,永远一起)。
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
    // 持久化(版本头 + 整体重写 + tmp rename,与 usage 同一纪律)
    // ------------------------------------------------------------------

    fn load_persisted(&self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        let Some(path) = &self.file else { return out };
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
            out.insert(k.to_string(), v.to_string());
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

/// §36 Core settings(V1)。
fn core_specs() -> Vec<SettingSpec> {
    vec![
        SettingSpec {
            key: SettingKey(Arc::from(KEY_HOTKEY)),
            label: "全局热键".into(),
            description: Some("唤起 / 隐藏 Launcher(toggle 语义固定,§53)".into()),
            kind: SettingKind::Hotkey,
            default: SettingValue::Hotkey(Hotkey::default()),
            apply_policy: ApplyPolicy::Immediate,
        },
        SettingSpec {
            key: SettingKey(Arc::from(KEY_HIDE_ON_FOCUS_LOSS)),
            label: "失焦隐藏".into(),
            description: Some("前台焦点离开时自动隐藏窗口(§54);关闭后失焦仅退出聚焦态".into()),
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

        let host = SettingsHost::new(Some(file.clone()), None, None);
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

        let host = SettingsHost::new(Some(file.clone()), None, None);
        assert_eq!(host.hotkey(), Hotkey::default());
        assert!(host.hide_on_focus_loss());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!("cue-settings-test3-{}", std::process::id()));
        let file = dir.join("settings.tsv");

        let mut host = SettingsHost::new(Some(file.clone()), None, None);
        host.commit(KEY_HIDE_ON_FOCUS_LOSS, SettingValue::Bool(false));
        drop(host);

        let host = SettingsHost::new(Some(file.clone()), None, None);
        assert!(!host.hide_on_focus_loss());
        assert_eq!(host.hotkey(), Hotkey::default()); // 未动的保持默认
        let _ = std::fs::remove_dir_all(&dir);
    }
}
