//! cue-core —— 薄的 Host Runtime。
//!
//! 单线程状态机,运行在 UI 线程上:
//! 异步工作以 Future 形式离开 Core,以事件形式回到 Core。
//!
//! 北极星:**Core 不取消异步工作;Core 通过 SessionId、ModuleEpoch
//! 和 Generation 判定异步结果是否仍然有效。**

mod effects;
mod event;
mod registry;
mod session;
mod settings;
mod spawner;
mod usage;

pub use effects::CoreEffect;
pub use event::{ActivationTicket, CoreEvent, HostEvent, QueryTicket};
pub use registry::{ModuleRegistry, RegistryError};
pub use session::{ActionMenuState, SessionId, SessionState};
pub use settings::{
    ApplyHotkey, ApplyStartOnBoot, KEY_DND_MODE, KEY_HOTKEY, KEY_START_ON_BOOT, NotifyDndMode,
    OpenPath, SettingsHost, SettingsModel, SettingsRow, SettingsViewState,
};
pub use spawner::TaskSpawner;
pub use usage::UsageStore;

use cue_protocol::logln;
use cue_protocol::*;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use std::path::PathBuf;
use std::sync::Arc;

/// Core 事件队列的生产端。可克隆、可跨线程;
/// host 事件、query 完成、activation 完成、module 自发事件都从这里回流。
pub type CoreEventSender = UnboundedSender<CoreEvent>;

pub struct CoreConfig {
    /// 存储根(由编排层解析,如 `%LOCALAPPDATA%\CUE`)。
    pub storage_root: PathBuf,
    /// usage 持久化文件;None = 纯内存(测试)。
    pub usage_file: Option<PathBuf>,
    /// 设置持久化文件;None = 纯内存(测试)。
    pub settings_file: Option<PathBuf>,
    /// Core/UI 请求预算。V1 为固定值,不来自任何 `module.*` 设置。
    pub result_limit: usize,
    /// core.hotkey 的同步 try-apply 回调。
    /// None(测试)时热键 try-apply 视为通过。
    pub apply_hotkey: Option<ApplyHotkey>,
    /// core.start_on_boot 的同步 try-apply 回调(写登录启动项,同模式)。
    /// None(测试)时 try-apply 视为通过。
    pub apply_start_on_boot: Option<ApplyStartOnBoot>,
    /// Path 类设置行的"打开"动作回调(系统默认程序打开路径,同模式)。
    /// None(测试)时打开视为成功。
    pub open_path: Option<OpenPath>,
    /// 前台全屏探针(免打扰模式,§127):返回 true 时热键唤起被静默忽略。
    /// 同步、只在热键按下瞬间调用;同 host 回调模式(函数,不是 trait)。
    /// None(测试)时视为"非全屏",门控不生效。
    pub fullscreen_probe: Option<Box<dyn FnMut() -> bool>>,
    /// core.dnd_mode 的 commit 后通知(托盘状态图标;同 host 回调模式)。
    /// Core::new 以初始值调一次,之后每次成功 commit 调一次。
    /// None(测试)时不通知。
    pub notify_dnd_mode: Option<NotifyDndMode>,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            storage_root: PathBuf::from("cue-data"),
            usage_file: None,
            settings_file: None,
            result_limit: 20,
            apply_hotkey: None,
            apply_start_on_boot: None,
            open_path: None,
            fullscreen_probe: None,
            notify_dnd_mode: None,
        }
    }
}

pub struct Core {
    config: CoreConfig,
    registry: ModuleRegistry,
    spawner: Arc<dyn TaskSpawner>,
    event_tx: CoreEventSender,
    event_rx: Option<UnboundedReceiver<CoreEvent>>,
    session: Option<SessionState>,
    next_session_id: u64,
    usage: UsageStore,
    /// Settings Host:设置的唯一所有者。
    settings: SettingsHost,
    /// 设置视图状态;Some 时 UI 渲染设置页而非搜索页。
    settings_view: Option<SettingsViewState>,
    /// 窗口可见 / 聚焦状态(toggle 的依据),由 host/UI 事件维护。
    visible: bool,
    focused: bool,
    /// 待执行的 CoreEffect 出站队列。
    effects: Vec<CoreEffect>,
    /// Core 拥有的 `module.<id>.trigger` 设置 key(§128):触发词归
    /// Core 路由管——校验/生效都在 Core,不走模块 try_apply。
    trigger_keys: std::collections::HashSet<String>,
}

impl Core {
    pub fn new(
        mut config: CoreConfig,
        registry: ModuleRegistry,
        spawner: Arc<dyn TaskSpawner>,
    ) -> Result<Self, ModuleError> {
        let (event_tx, event_rx) = mpsc::unbounded();
        let usage = UsageStore::new(config.usage_file.clone());
        // Settings Host 必须先于 load_modules:ModuleContext 的设置快照
        // 依赖它(设置只存在这里)。apply_* 回调的所有权移交 host。
        let settings = SettingsHost::new(
            config.settings_file.clone(),
            config.apply_hotkey.take(),
            config.apply_start_on_boot.take(),
            config.open_path.take(),
        );
        let mut core = Self {
            settings,
            config,
            registry,
            spawner,
            event_tx,
            event_rx: Some(event_rx),
            session: None,
            next_session_id: 1,
            usage,
            settings_view: None,
            visible: false,
            focused: false,
            effects: Vec::new(),
            trigger_keys: std::collections::HashSet::new(),
        };
        core.load_modules()?;
        // 免打扰开关的初始值同步给 host(托盘状态图标,§127):
        // 此后每次成功 commit 在 apply_setting_inner 里通知。
        if let Some(notify) = core.config.notify_dnd_mode.as_mut() {
            notify(core.settings.dnd_mode());
        }
        Ok(core)
    }

    fn load_modules(&mut self) -> Result<(), ModuleError> {
        // 触发词 spec(§128)需要 descriptor;先取一份快照脱离 registry 借用。
        let launchers: Vec<(ModuleId, LauncherDescriptor)> = self
            .registry
            .launcher_descriptors()
            .map(|(id, d)| (id.clone(), d))
            .collect();
        for id in self.registry.ids() {
            // 触发词 spec(§128)先于模块 schema 注册:若模块 schema
            // 误带 module.<id>.trigger 条目,first-wins 去重让 Core
            // 的行生效——触发词归 Core,不归模块(§128)。
            if let Some((_, desc)) = launchers.iter().find(|(mid, _)| mid == &id)
                && !desc.is_default
                && let Some(trigger) = &desc.trigger
            {
                let name = self
                    .registry
                    .module(&id)
                    .map(|m| m.descriptor().name)
                    .unwrap_or(id.as_str());
                let key = self.settings.register_trigger_spec(&id, name, trigger);
                // 持久化文件里的空值(手工改坏)在路由层自愈回落声明值;
                // 显示层一并归一,设置页所见即生效值(仅内存,随下次
                // 事务的整体重写落盘)。
                if matches!(self.settings.value(&key), Some(SettingValue::String(v)) if v.is_empty())
                {
                    self.settings
                        .set_value_no_persist(&key, SettingValue::String(trigger.clone()));
                }
                self.trigger_keys.insert(key);
            }

            // 再注册模块 schema,然后 build_context——load 时的
            // ModuleSettings 快照才能带上该模块自己的默认值与持久化值。
            let schema = self
                .registry
                .module(&id)
                .map(|m| m.settings_schema())
                .unwrap_or_default();
            self.settings.register_module_specs(&id, schema);

            let epoch = self.registry.epoch(&id).unwrap_or(0);
            let ctx = self.build_context(&id, epoch);
            let module = self
                .registry
                .module_mut(&id)
                .ok_or_else(|| ModuleError::InvalidState(format!("module {id} vanished")))?;
            module.load(ctx)?;
        }
        Ok(())
    }

    fn build_context(&self, id: &ModuleId, epoch: u64) -> ModuleContext {
        let module_root = self.config.storage_root.join("modules").join(id.as_str());
        let storage = ModuleStorage {
            data: module_root.join("data"),
            state: module_root.join("state"),
            cache: module_root.join("cache"),
        };
        // 目录创建失败不阻止 load;Module 自己的后续 IO 会以 ModuleError 正常上报。
        for dir in [&storage.data, &storage.state, &storage.cache] {
            let _ = std::fs::create_dir_all(dir);
        }
        ModuleContext {
            module_id: id.clone(),
            storage,
            settings: self.settings.values_for_module(id),
            usage: self.usage.reader_for(id),
            logger: Arc::new(StderrLogger {
                module: id.as_str().to_string(),
            }),
            events: Arc::new(CoreEventSink {
                tx: self.event_tx.clone(),
                module_id: id.clone(),
                module_epoch: epoch,
            }),
        }
    }

    /// 取走事件队列消费端(只能取一次)。由 UI 线程的泵消费。
    pub fn take_event_receiver(&mut self) -> UnboundedReceiver<CoreEvent> {
        self.event_rx.take().expect("event receiver already taken")
    }

    /// 事件队列生产端,供编排层接入 host 事件。
    pub fn event_sender(&self) -> CoreEventSender {
        self.event_tx.clone()
    }

    pub fn config(&self) -> &CoreConfig {
        &self.config
    }

    // ------------------------------------------------------------------
    // 状态迁移(Host/UI 事件入口)
    // ------------------------------------------------------------------

    pub fn open_session(&mut self) {
        if self.session.is_some() {
            return;
        }
        let id = SessionId(self.next_session_id);
        self.next_session_id += 1;
        let Some(default_module) = self.registry.default_module().cloned() else {
            return;
        };
        self.session = Some(SessionState::new(id, default_module));
        self.visible = true;
        self.focused = true;
        self.effects.push(CoreEffect::ShowLauncher);
        self.effects.push(CoreEffect::FocusInput);
        // 空查询——打开后由 Module 决定空查询展示什么(usage Top Apps 等)。
        self.run_query();
    }

    pub fn close_session(&mut self) {
        if self.session.take().is_some() {
            self.visible = false;
            self.focused = false;
            self.effects.push(CoreEffect::HideLauncher);
        }
    }

    /// toggle:隐藏 → 打开;可见且聚焦 → 关闭;可见未聚焦 → 聚焦。
    /// 设置页开着时,热键等价 Esc(关闭设置)。
    pub fn hotkey_pressed(&mut self) {
        if self.settings_view.is_some() {
            self.dismiss_settings();
        } else if !self.visible {
            // 免打扰模式(§127):前台全屏时静默忽略唤起。
            // 只门控 show 路径——hide/focus 半段、托盘唤起照常。
            if self.settings.dnd_mode()
                && self
                    .config
                    .fullscreen_probe
                    .as_mut()
                    .map(|probe| probe())
                    .unwrap_or(false)
            {
                return;
            }
            self.open_session();
        } else if self.focused {
            self.close_session();
        } else {
            self.focused = true;
            self.effects.push(CoreEffect::FocusInput);
        }
    }

    /// 第二实例请求 show / focus。
    pub fn show_requested(&mut self) {
        if self.settings_view.is_some() {
            self.focused = true;
            self.effects.push(CoreEffect::FocusInput);
        } else if !self.visible {
            self.open_session();
        } else {
            self.focused = true;
            self.effects.push(CoreEffect::FocusInput);
        }
    }

    pub fn focus_lost(&mut self) {
        self.focused = false;
        if self.visible && self.settings.hide_on_focus_loss() {
            if self.settings_view.is_some() {
                self.dismiss_settings();
            } else {
                self.close_session();
            }
        }
    }

    pub fn focus_gained(&mut self) {
        self.focused = true;
    }

    // ------------------------------------------------------------------
    // Settings UI 的 Core 侧:出模型、收变更,永不渲染。
    // ------------------------------------------------------------------

    /// 打开设置视图(托盘菜单入口)。设置不是 module session:
    /// 搜索会话静默退场(其未完成的 query 由 ticket 自然失效)。
    pub fn open_settings(&mut self) {
        if self.settings_view.is_some() {
            return;
        }
        self.session = None;
        self.settings_view = Some(SettingsViewState::default());
        if !self.visible {
            self.visible = true;
            self.effects.push(CoreEffect::ShowLauncher);
        }
        self.focused = true;
        self.effects.push(CoreEffect::FocusInput);
    }

    pub fn dismiss_settings(&mut self) {
        if self.settings_view.take().is_some() {
            self.visible = false;
            self.focused = false;
            self.effects.push(CoreEffect::HideLauncher);
        }
    }

    pub fn in_settings(&self) -> bool {
        self.settings_view.is_some()
    }

    pub fn settings_model(&self) -> Option<SettingsModel> {
        self.settings_view
            .as_ref()
            .map(|view| self.settings.model(view))
    }

    pub fn settings_select_next(&mut self) {
        if let Some(view) = self.settings_view.as_mut() {
            let max = self.settings.row_count().saturating_sub(1);
            view.selected = (view.selected + 1).min(max);
        }
    }

    pub fn settings_select_prev(&mut self) {
        if let Some(view) = self.settings_view.as_mut() {
            view.selected = view.selected.saturating_sub(1);
        }
    }

    /// 当前生效的热键(编排层启动注册时读取)。
    pub fn hotkey(&self) -> Hotkey {
        self.settings.hotkey()
    }

    /// 事务入口:校验 → try-apply → commit → persist。
    /// 失败不 commit、返回错误;UI 展示错误并保留旧值显示。
    pub fn apply_setting(&mut self, key: &str, candidate: SettingValue) -> Result<(), String> {
        let result = self.apply_setting_inner(key, candidate);
        if let Some(view) = self.settings_view.as_mut() {
            view.error = match &result {
                Ok(()) => None,
                Err(msg) => Some(msg.as_str().into()),
            };
        }
        result
    }

    /// Path 类设置行的激活:用系统默认程序打开该路径(不是值变更,
    /// 不走事务)。失败信息进模型,与 apply 错误同位置回显。
    pub fn open_setting_path(&mut self, key: &str) -> Result<(), String> {
        let result = self.settings.open_path(key);
        if let Some(view) = self.settings_view.as_mut() {
            view.error = match &result {
                Ok(()) => None,
                Err(msg) => Some(msg.as_str().into()),
            };
        }
        result
    }

    fn apply_setting_inner(&mut self, key: &str, candidate: SettingValue) -> Result<(), String> {
        // 第一步:规格存在性 + 类型/取值校验。
        let Some(spec) = self.settings.spec(key) else {
            return Err(format!("unknown setting: {key}"));
        };
        if !kind_matches(spec.kind, &candidate) {
            return Err(format!("type mismatch for {key}: expected {:?}", spec.kind));
        }
        if let SettingValue::Hotkey(h) = &candidate
            && h.modifiers.is_empty()
        {
            return Err("热键至少需要一个修饰键".into());
        }
        let policy = spec.apply_policy;
        match policy {
            ApplyPolicy::Immediate => {
                // 触发词(§128):trim 归一 + Core 校验,不走模块 try_apply。
                let candidate = if self.trigger_keys.contains(key) {
                    let SettingValue::String(t) = &candidate else {
                        return Err("type mismatch".into());
                    };
                    let t = t.trim().to_string();
                    self.validate_trigger(key, &t)?;
                    SettingValue::String(t)
                } else {
                    candidate
                };
                // 第二步:try-apply(core.* 由所有者执行;module.* 经 registry)。
                if key == KEY_HOTKEY {
                    let SettingValue::Hotkey(h) = &candidate else {
                        return Err("type mismatch".into());
                    };
                    self.settings.try_apply_hotkey(h)?;
                } else if key == KEY_START_ON_BOOT {
                    let SettingValue::Bool(on) = &candidate else {
                        return Err("type mismatch".into());
                    };
                    self.settings.try_apply_start_on_boot(*on)?;
                } else if self.trigger_keys.contains(key) {
                    // 触发词归 Core 路由:校验已在上方完成,无宿主副作用。
                } else if key.starts_with("module.") {
                    let mut cs = SettingsChangeSet::default();
                    cs.changes
                        .push((SettingKey(Arc::from(key)), candidate.clone()));
                    let Some(id) = module_id_of(key) else {
                        return Err(format!("malformed module setting key: {key}"));
                    };
                    let module = self
                        .registry
                        .module_mut(&id)
                        .ok_or_else(|| format!("module {id} not loaded"))?;
                    module.try_apply_settings(cs).map_err(|e| e.to_string())?;
                }
                // 第三、四步:commit + persist(host 内为原子的一对)。
                self.settings.commit(key, candidate);
                // core.dnd_mode commit 后通知 host(托盘状态图标,§127)。
                if key == KEY_DND_MODE
                    && let Some(notify) = self.config.notify_dnd_mode.as_mut()
                {
                    notify(self.settings.dnd_mode());
                }
            }
            ApplyPolicy::RestartApplication => {
                self.settings.commit(key, candidate);
                self.settings.mark_restart_required();
            }
            ApplyPolicy::ReloadModule => {
                // 允许 V1 只实现 Immediate 与 RestartApplication。
                return Err("V1 不支持 ReloadModule 策略".into());
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // 输入与选择
    // ------------------------------------------------------------------

    pub fn input_changed(&mut self, input: String) {
        if self.session.is_none() {
            return;
        }
        // 输入未变(空输入上的 Backspace、粘贴相同内容)不视为变化:
        // 不 bump generation、不重查、不把选择重置回第 0 行。
        if self.session.as_ref().is_some_and(|s| s.raw_input == input) {
            return;
        }
        // route 读 settings(生效触发词,§128),先算再借 session。
        let (module_id, query) = self.route(&input);
        {
            let s = self.session.as_mut().expect("checked above");
            s.raw_input = input;
            if module_id != s.active_module {
                s.active_module = module_id.clone();
            }
            // 输入变化立即清空——stale 结果永不可激活;
            // 动作菜单引用旧选中项的动作快照,一并关闭。
            s.generation += 1;
            s.results.clear();
            s.selected = None;
            s.error = None;
            s.action_menu = None;
        }
        self.run_query_with(module_id, query);
    }

    pub fn push_text(&mut self, text: &str) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let mut input = s.raw_input.clone();
        input.push_str(text);
        self.input_changed(input);
    }

    pub fn backspace(&mut self) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let mut input = s.raw_input.clone();
        input.pop();
        self.input_changed(input);
    }

    /// 粘贴允许 Unicode(IME 禁用只针对 composition)。
    pub fn paste(&mut self, text: &str) {
        let cleaned: String = text
            .chars()
            .map(|c| {
                if c == '\n' || c == '\r' || c == '\t' {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        // 不 trim:前导/尾随空格可能是用户有意粘贴的(接在已有输入后)。
        self.push_text(&cleaned);
    }

    pub fn select_next(&mut self) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        if s.results.is_empty() {
            s.selected = None;
            return;
        }
        s.selected = Some(match s.selected {
            Some(i) => (i + 1).min(s.results.len() - 1),
            None => 0,
        });
    }

    pub fn select_prev(&mut self) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        if s.results.is_empty() {
            s.selected = None;
            return;
        }
        s.selected = Some(match s.selected {
            Some(i) => i.saturating_sub(1),
            None => 0,
        });
    }

    // ------------------------------------------------------------------
    // 次级动作菜单(Tab 打开):状态在 session 上,键盘路由由
    // UI 按 in_action_menu() 决定(同设置页模式)。
    // ------------------------------------------------------------------

    /// Tab:对选中项快照 Module::actions 并打开菜单。无选中项或
    /// 模块返回空动作列表时不打开。
    pub fn open_action_menu(&mut self) {
        let opened = self
            .session
            .as_ref()
            .is_some_and(|s| s.action_menu.is_some());
        if opened {
            return;
        }
        let Some((module_id, item)) = self
            .session
            .as_ref()
            .and_then(|s| Some((s.active_module.clone(), s.selected_item()?.clone())))
        else {
            return;
        };
        let Some(module) = self.registry.module(&module_id) else {
            return;
        };
        let actions = module.actions(&item);
        if actions.is_empty() {
            return;
        }
        let item_title = module.present(&item).title;
        if let Some(s) = self.session.as_mut() {
            s.action_menu = Some(ActionMenuState {
                actions,
                selected: 0,
                item_title,
            });
        }
    }

    pub fn close_action_menu(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.action_menu = None;
        }
    }

    pub fn in_action_menu(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.action_menu.is_some())
    }

    pub fn action_menu_select_next(&mut self) {
        let Some(menu) = self.session.as_mut().and_then(|s| s.action_menu.as_mut()) else {
            return;
        };
        menu.selected = (menu.selected + 1).min(menu.actions.len().saturating_sub(1));
    }

    pub fn action_menu_select_prev(&mut self) {
        let Some(menu) = self.session.as_mut().and_then(|s| s.action_menu.as_mut()) else {
            return;
        };
        menu.selected = menu.selected.saturating_sub(1);
    }

    /// 菜单的 UI 行模型(Core 出模型,UI 只渲染,同设置页)。
    pub fn action_menu_model(&self) -> Option<ActionMenuModel> {
        let menu = self.session.as_ref()?.action_menu.as_ref()?;
        Some(ActionMenuModel {
            item_title: menu.item_title.clone(),
            rows: menu
                .actions
                .iter()
                .map(|a| ActionMenuRow {
                    label: a.label.clone(),
                    // Shortcut 与 Hotkey 同形(protocol 注释:刻意不共类型);
                    // 显示格式复用 Hotkey 的 Display。
                    shortcut: a.shortcut.map(|s| {
                        Hotkey {
                            modifiers: s.modifiers,
                            key: s.key,
                        }
                        .to_string()
                    }),
                })
                .collect(),
            selected: menu.selected,
        })
    }

    // ------------------------------------------------------------------
    // Query / Activation
    // ------------------------------------------------------------------

    /// Input Routing:Core 只解析 Module Trigger;
    /// trigger 之后的剩余输入原样交给 Module(如 `ext:pdf` 语义不属于 Core)。
    /// 触发词取生效值(设置覆盖 ?? 模块声明,§128)。
    fn route(&self, input: &str) -> (ModuleId, String) {
        for (id, descriptor) in self.registry.launcher_descriptors() {
            if let Some(trigger) = self.effective_trigger_of(id, descriptor.trigger.as_deref())
                && let Some(query) = match_trigger(input, &trigger)
            {
                return (id.clone(), query);
            }
        }
        // 不变量:session 存在 ⇒ 默认模块存在——open_session 在无默认
        // 模块时根本不开 session,§65 又保证 AppModule 不可禁用。
        let default = self
            .registry
            .default_module()
            .cloned()
            .expect("registry has no default module");
        (default, input.to_string())
    }

    /// 生效触发词 = 设置值(非空)?? 模块声明值(§128)。
    /// 空值只会来自手工改坏的 settings.tsv(设置事务拒绝空),
    /// 按"未设置"处理、回落声明值——自愈,不把模块入口弄丢。
    fn effective_trigger_of(&self, id: &ModuleId, declared: Option<&str>) -> Option<String> {
        let key = format!("module.{}.trigger", id.as_str());
        match self.settings.value(&key) {
            Some(SettingValue::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => declared.map(str::to_string),
        }
    }

    /// 触发词校验(§128):非空、无空白、长度封顶、不与其他模块的
    /// 生效触发词冲突。纯 Core 校验——触发词不走模块 try_apply。
    fn validate_trigger(&self, self_key: &str, candidate: &str) -> Result<(), String> {
        if candidate.is_empty() {
            return Err("触发词不能为空".into());
        }
        if candidate.chars().any(char::is_whitespace) {
            return Err("触发词不能包含空白字符".into());
        }
        if candidate.chars().count() > 16 {
            return Err("触发词最长 16 个字符".into());
        }
        let self_id = module_id_of(self_key);
        for (id, descriptor) in self.registry.launcher_descriptors() {
            if self_id.as_ref() == Some(id) {
                continue;
            }
            if let Some(other) = self.effective_trigger_of(id, descriptor.trigger.as_deref())
                && other == candidate
            {
                return Err(format!(
                    "触发词 \"{candidate}\" 已被模块 {} 使用",
                    id.as_str()
                ));
            }
        }
        Ok(())
    }

    fn run_query(&mut self) {
        let Some(raw) = self.session.as_ref().map(|s| s.raw_input.clone()) else {
            return;
        };
        let (module_id, query) = self.route(&raw);
        self.run_query_with(module_id, query);
    }

    fn run_query_with(&mut self, module_id: ModuleId, query: String) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let ticket = QueryTicket {
            session_id: s.id,
            module_id: module_id.clone(),
            // query 在那个模块实例上发起,epoch 取发起时的当前值。
            module_epoch: self.registry.epoch(&module_id).unwrap_or(0),
            generation: s.generation,
        };
        let ctx = QueryContext {
            query,
            result_limit: self.config.result_limit,
        };
        let Some(module) = self.registry.module_mut(&module_id) else {
            return;
        };
        // 创建 Future 必须 < 1 ms、不得触碰 IO;真正的执行在 executor 上。
        let fut = module.query(ctx);
        let tx = self.event_tx.clone();
        self.spawner.spawn(Box::pin(async move {
            let result = fut.await;
            let _ = tx.unbounded_send(CoreEvent::QueryCompleted { ticket, result });
        }));
    }

    pub fn activate_selected(&mut self) {
        self.activate_with(ActionId::PRIMARY);
    }

    /// Enter on 动作菜单:以菜单选中的 ActionId 激活当前选中项。
    /// 菜单先关——失败时用户看到的是结果列表 + 错误横幅。
    pub fn activate_action_menu_selection(&mut self) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let Some(menu) = s.action_menu.take() else {
            return;
        };
        let Some(action) = menu.actions.get(menu.selected).map(|a| a.id) else {
            return;
        };
        self.activate_with(action);
    }

    fn activate_with(&mut self, action: ActionId) {
        let Some(s) = self.session.as_mut() else {
            return;
        };
        if s.activation_in_flight {
            return;
        }
        let Some(item) = s.selected_item().cloned() else {
            return;
        };
        let ticket = ActivationTicket {
            session_id: s.id,
            module_id: s.active_module.clone(),
            module_epoch: self.registry.epoch(&s.active_module).unwrap_or(0),
        };
        // 先取模块再置 flag:查找失败提前返回时不会有 completion
        // 事件来复位,flag 会卡死本 session 的 Enter(今天不可达,
        // 但顺序必须防御)。
        let Some(module) = self.registry.module_mut(&ticket.module_id) else {
            return;
        };
        s.activation_in_flight = true;
        let fut = module.activate(&item, action);
        let tx = self.event_tx.clone();
        self.spawner.spawn(Box::pin(async move {
            let outcome = fut.await;
            let _ = tx.unbounded_send(CoreEvent::ActivationCompleted { ticket, outcome });
        }));
    }

    // ------------------------------------------------------------------
    // 事件处理(ticket 校验; activation 处置)
    // ------------------------------------------------------------------

    /// 处理一个回流事件。返回 true 表示可见状态发生了变化(UI 应重绘)。
    pub fn handle_event(&mut self, event: CoreEvent) -> bool {
        match event {
            CoreEvent::QueryCompleted { ticket, result } => {
                self.on_query_completed(&ticket, result)
            }
            CoreEvent::ActivationCompleted { ticket, outcome } => {
                self.on_activation_completed(&ticket, outcome)
            }
            CoreEvent::ModuleEvent {
                module_id,
                module_epoch,
                event,
            } => self.on_module_event(&module_id, module_epoch, event),
            CoreEvent::Host(host) => {
                self.on_host_event(host);
                true
            }
        }
    }

    fn on_query_completed(&mut self, ticket: &QueryTicket, result: QueryResult) -> bool {
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        // 四项全部匹配才接受。epoch 与 registry 当前值比较——
        // reload 之后旧实例的结果必死。
        let current_epoch = self.registry.epoch(&s.active_module).unwrap_or(u64::MAX);
        if ticket.session_id != s.id
            || ticket.module_id != s.active_module
            || ticket.module_epoch != current_epoch
            || ticket.generation != s.generation
        {
            return false;
        }
        match result {
            Ok(resp) => {
                let mut items = resp.items;
                items.truncate(self.config.result_limit);
                // 非空默认选中第 0 项。
                s.selected = if items.is_empty() { None } else { Some(0) };
                s.results = items;
                s.error = None;
            }
            Err(e) => {
                s.results.clear();
                s.selected = None;
                s.error = Some(e);
            }
        }
        // 结果集在菜单打开期间被替换:菜单的动作快照属于旧选中项,
        // 继续用会把旧动作打到新结果上——关掉。
        s.action_menu = None;
        true
    }

    fn on_activation_completed(
        &mut self,
        ticket: &ActivationTicket,
        outcome: ModuleOutcome,
    ) -> bool {
        // usage 总是记录(激活真实发生过)。
        if let Some(req) = &outcome.usage {
            self.usage.record(&ticket.module_id, req);
        }
        // session 处置只对发起它的那个 session 生效。
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        if ticket.session_id != s.id {
            return false;
        }
        s.activation_in_flight = false;
        // epoch 失配(模块 reload 换过实例):处置是旧实例激活的意图,
        // 不落到当前实例上;flag 已复位、usage 已记录(§96 三元绑定)。
        let current_epoch = self.registry.epoch(&ticket.module_id).unwrap_or(u64::MAX);
        if ticket.module_epoch != current_epoch {
            return true;
        }
        match &outcome.status {
            OutcomeStatus::Success => {
                if outcome.session == SessionDisposition::Close {
                    self.close_session();
                }
            }
            OutcomeStatus::Failed(e) => {
                // 失败默认 KeepOpen + 错误展示。
                s.error = Some(e.clone());
            }
        }
        true
    }

    fn on_module_event(
        &mut self,
        module_id: &ModuleId,
        module_epoch: u64,
        event: ModuleEvent,
    ) -> bool {
        let Some(s) = self.session.as_ref() else {
            return false;
        };
        // epoch 与 registry 当前值比较,旧实例事件一律丢弃;
        // 非 active module 同样丢弃。
        let current_epoch = self.registry.epoch(module_id).unwrap_or(u64::MAX);
        if *module_id != s.active_module || module_epoch != current_epoch {
            return false;
        }
        match event {
            ModuleEvent::PresentationInvalidated { items } => {
                // 只关心当前可见的 item;命中则 UI 需要重跑 present()。
                items.iter().any(|id| s.contains_item(*id))
            }
        }
    }

    fn on_host_event(&mut self, host: HostEvent) {
        match host {
            HostEvent::HotkeyPressed => self.hotkey_pressed(),
            HostEvent::ShowRequested => self.show_requested(),
            HostEvent::FocusLost => self.focus_lost(),
            HostEvent::OpenSettings => self.open_settings(),
        }
    }

    // ------------------------------------------------------------------
    // 供 UI 读取
    // ------------------------------------------------------------------

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn session(&self) -> Option<&SessionState> {
        self.session.as_ref()
    }

    /// 对当前 active module 执行 present()。UI 只对可见行调用(< 1 ms)。
    pub fn present(&self, item: &ModuleItem) -> Option<ResultPresentation> {
        let s = self.session.as_ref()?;
        self.registry
            .module(&s.active_module)
            .map(|m| m.present(item))
    }

    /// 取走待执行的 CoreEffect,由编排层执行。
    pub fn take_effects(&mut self) -> Vec<CoreEffect> {
        std::mem::take(&mut self.effects)
    }

    /// usage 查询(主要供测试与 Settings UI)。
    pub fn usage_stat(
        &self,
        module: &ModuleId,
        item_key: &str,
        action: ActionId,
    ) -> Option<UsageStat> {
        self.usage.stat(module, item_key, action)
    }

    /// 测试与 ReloadModule 策略用:替换模块实例并递增 epoch。
    /// 旧实例的在途 query / 事件随 epoch 失效;
    /// Core 不自动重查——session 中的旧结果保留到下一次输入变化。
    pub fn reload_module(&mut self, module: Box<dyn LauncherModule>) -> Result<(), ModuleError> {
        let id = module.descriptor().id.clone();
        if let Some(old) = self.registry.module_mut(&id) {
            old.unload();
        }
        self.registry.replace(module)?;
        let epoch = self.registry.epoch(&id).unwrap_or(0);
        let ctx = self.build_context(&id, epoch);
        self.registry
            .module_mut(&id)
            .ok_or_else(|| ModuleError::InvalidState(format!("module {id} vanished")))?
            .load(ctx)
    }
}

/// 触发词匹配规则:以字母/数字结尾的触发词(`b`、`ext`)要求
/// 词边界——trigger 之后必须是空白或输入结束,否则 `baidu` 会被 `b`
/// 吞掉;边界空白不进查询(`b  github` 的查询是 `github`)。以标点
/// 结尾的触发词(`/`)逐字前缀匹配,查询原样传递。
fn match_trigger(input: &str, trigger: &str) -> Option<String> {
    let rest = input.strip_prefix(trigger)?;
    let wordy = trigger.chars().last().is_some_and(char::is_alphanumeric);
    if wordy {
        match rest.chars().next() {
            None => return Some(String::new()),
            Some(c) if c.is_whitespace() => return Some(rest.trim_start().to_string()),
            Some(_) => return None,
        }
    }
    Some(rest.to_string())
}

/// 动作菜单的 UI 行模型(Core 出模型,UI 只渲染,同设置页)。
pub struct ActionMenuModel {
    /// 菜单归属的选中项标题(UI 头部展示)。
    pub item_title: Arc<str>,
    pub rows: Vec<ActionMenuRow>,
    pub selected: usize,
}

pub struct ActionMenuRow {
    pub label: Arc<str>,
    /// 预格式化的快捷键提示(如 "Ctrl+Enter");模块未给则为 None。
    pub shortcut: Option<String>,
}

/// 校验:kind 与值类型必须匹配。
fn kind_matches(kind: SettingKind, v: &SettingValue) -> bool {
    matches!(
        (kind, v),
        (SettingKind::Bool, SettingValue::Bool(_))
            | (SettingKind::Integer, SettingValue::Integer(_))
            | (SettingKind::String, SettingValue::String(_))
            | (SettingKind::Enum, SettingValue::Enum(_))
            | (SettingKind::Path, SettingValue::Path(_))
            | (SettingKind::Hotkey, SettingValue::Hotkey(_))
    )
}

/// `module.<id>.<rest>` → ModuleId。
fn module_id_of(key: &str) -> Option<ModuleId> {
    let rest = key.strip_prefix("module.")?;
    let (id, _) = rest.split_once('.')?;
    Some(ModuleId::new(id))
}

/// sink 在 load 时绑定 (ModuleId, ModuleEpoch)。
struct CoreEventSink {
    tx: CoreEventSender,
    module_id: ModuleId,
    module_epoch: u64,
}

impl ModuleEventSend for CoreEventSink {
    fn send(&self, event: ModuleEvent) {
        let _ = self.tx.unbounded_send(CoreEvent::ModuleEvent {
            module_id: self.module_id.clone(),
            module_epoch: self.module_epoch,
            event,
        });
    }
}

/// 统一 logger 的最小实现(写 stderr;日志文件随后续迭代落地)。
struct StderrLogger {
    module: String,
}

impl ModuleLog for StderrLogger {
    fn log(&self, level: LogLevel, message: &str) {
        logln!("[{level:?}] [{}] {message}", self.module);
    }
}

#[cfg(test)]
mod tests {
    use super::match_trigger;

    #[test]
    fn wordy_trigger_requires_boundary() {
        // 词边界命中:EOI 与空白
        assert_eq!(match_trigger("b", "b"), Some(String::new()));
        assert_eq!(match_trigger("b github", "b"), Some("github".to_string()));
        // 边界空白 trim,不进查询
        assert_eq!(match_trigger("b   github", "b"), Some("github".to_string()));
        // 无边界不命中:baidu 不能被 b 吞掉
        assert_eq!(match_trigger("baidu", "b"), None);
        assert_eq!(match_trigger("ext:pdf", "ext"), None);
        // 多字符词触发词同规则
        assert_eq!(match_trigger("ext pdf", "ext"), Some("pdf".to_string()));
    }

    #[test]
    fn punctuation_trigger_matches_verbatim() {
        assert_eq!(match_trigger("/", "/"), Some(String::new()));
        assert_eq!(
            match_trigger("/etc/hosts", "/"),
            Some("etc/hosts".to_string())
        );
        // 标点触发词不要求边界,也不 trim
        assert_eq!(match_trigger("/  x", "/"), Some("  x".to_string()));
    }
}
