//! cue-core —— 薄的 Host Runtime(architecture.md §5)。
//!
//! 单线程状态机,运行在 UI 线程上(§91):
//! 异步工作以 Future 形式离开 Core,以事件形式回到 Core。
//!
//! 北极星(§91):**Core 不取消异步工作;Core 通过 SessionId、ModuleEpoch
//! 和 Generation 判定异步结果是否仍然有效。**

mod effects;
mod event;
mod registry;
mod session;
mod spawner;
mod usage;

pub use effects::CoreEffect;
pub use event::{ActivationTicket, CoreEvent, HostEvent, QueryTicket};
pub use registry::{ModuleRegistry, RegistryError};
pub use session::{SessionId, SessionState};
pub use spawner::TaskSpawner;
pub use usage::UsageStore;

use cue_protocol::*;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use std::path::PathBuf;
use std::sync::Arc;

/// Core 事件队列的生产端(§96)。可克隆、可跨线程;
/// host 事件、query 完成、activation 完成、module 自发事件都从这里回流。
pub type CoreEventSender = UnboundedSender<CoreEvent>;

pub struct CoreConfig {
    /// §43 存储根(由编排层解析,如 `%LOCALAPPDATA%\CUE`)。
    pub storage_root: PathBuf,
    /// §94 Core/UI 请求预算。V1 为固定值,不来自任何 `module.*` 设置。
    pub result_limit: usize,
    /// §54 / §115 `core.hide_on_focus_loss`(V1 固定默认 true)。
    pub hide_on_focus_loss: bool,
    /// §53 默认 Alt+Space。
    pub default_hotkey: Hotkey,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            storage_root: PathBuf::from("cue-data"),
            result_limit: 20,
            hide_on_focus_loss: true,
            default_hotkey: Hotkey::default(),
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
    /// 窗口可见 / 聚焦状态(§53 toggle 的依据),由 host/UI 事件维护。
    visible: bool,
    focused: bool,
    /// §112 待执行的 CoreEffect 出站队列。
    effects: Vec<CoreEffect>,
}

impl Core {
    pub fn new(
        config: CoreConfig,
        registry: ModuleRegistry,
        spawner: Arc<dyn TaskSpawner>,
    ) -> Result<Self, ModuleError> {
        let (event_tx, event_rx) = mpsc::unbounded();
        let mut core = Self {
            config,
            registry,
            spawner,
            event_tx,
            event_rx: Some(event_rx),
            session: None,
            next_session_id: 1,
            usage: UsageStore::default(),
            visible: false,
            focused: false,
            effects: Vec::new(),
        };
        core.load_modules()?;
        Ok(core)
    }

    fn load_modules(&mut self) -> Result<(), ModuleError> {
        for id in self.registry.ids() {
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
        let module_root = self
            .config
            .storage_root
            .join("modules")
            .join(id.as_str());
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
            settings: ModuleSettings::default(),
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

    /// 取走事件队列消费端(只能取一次)。由 UI 线程的泵消费(§96)。
    pub fn take_event_receiver(&mut self) -> UnboundedReceiver<CoreEvent> {
        self.event_rx
            .take()
            .expect("event receiver already taken")
    }

    /// 事件队列生产端,供编排层接入 host 事件(§112)。
    pub fn event_sender(&self) -> CoreEventSender {
        self.event_tx.clone()
    }

    pub fn config(&self) -> &CoreConfig {
        &self.config
    }

    // ------------------------------------------------------------------
    // §112 状态迁移(Host/UI 事件入口)
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
        // 空查询:§115 —— 打开后由 Module 决定空查询展示什么(usage Top Apps 等)。
        self.run_query();
    }

    pub fn close_session(&mut self) {
        if self.session.take().is_some() {
            self.visible = false;
            self.focused = false;
            self.effects.push(CoreEffect::HideLauncher);
        }
    }

    /// §53 toggle:隐藏 → 打开;可见且聚焦 → 关闭;可见未聚焦 → 聚焦。
    pub fn hotkey_pressed(&mut self) {
        if !self.visible {
            self.open_session();
        } else if self.focused {
            self.close_session();
        } else {
            self.focused = true;
            self.effects.push(CoreEffect::FocusInput);
        }
    }

    /// §113:第二实例请求 show / focus。
    pub fn show_requested(&mut self) {
        if !self.visible {
            self.open_session();
        } else {
            self.focused = true;
            self.effects.push(CoreEffect::FocusInput);
        }
    }

    pub fn focus_lost(&mut self) {
        self.focused = false;
        if self.visible && self.config.hide_on_focus_loss {
            self.close_session();
        }
    }

    pub fn focus_gained(&mut self) {
        self.focused = true;
    }

    // ------------------------------------------------------------------
    // 输入与选择(§5.2 / §5.5 / §102)
    // ------------------------------------------------------------------

    pub fn input_changed(&mut self, input: String) {
        let (module_id, query) = {
            let Some(s) = self.session.as_mut() else {
                return;
            };
            s.raw_input = input;
            let (module_id, query) = route(&self.registry, &s.raw_input);
            if module_id != s.active_module {
                s.active_module = module_id.clone();
            }
            // §102:输入变化立即清空——stale 结果永不可激活。
            s.generation += 1;
            s.results.clear();
            s.selected = None;
            s.error = None;
            (module_id, query)
        };
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

    /// §115:粘贴允许 Unicode(IME 禁用只针对 composition)。
    pub fn paste(&mut self, text: &str) {
        let cleaned: String = text
            .chars()
            .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
            .collect();
        self.push_text(cleaned.trim());
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
    // Query / Activation(§94–96、§103)
    // ------------------------------------------------------------------

    fn run_query(&mut self) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let (module_id, query) = route(&self.registry, &s.raw_input);
        self.run_query_with(module_id, query);
    }

    fn run_query_with(&mut self, module_id: ModuleId, query: String) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        let ticket = QueryTicket {
            session_id: s.id,
            module_id: module_id.clone(),
            // query 在那个模块实例上发起,epoch 取发起时的当前值(§96)。
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
        // 创建 Future 必须 < 1 ms、不得触碰 IO(§93);真正的执行在 executor 上。
        let fut = module.query(ctx);
        let tx = self.event_tx.clone();
        self.spawner.spawn(Box::pin(async move {
            let result = fut.await;
            let _ = tx.unbounded_send(CoreEvent::QueryCompleted { ticket, result });
        }));
    }

    pub fn activate_selected(&mut self) {
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
        s.activation_in_flight = true;
        let Some(module) = self.registry.module_mut(&ticket.module_id) else {
            return;
        };
        let fut = module.activate(&item, ActionId::PRIMARY);
        let tx = self.event_tx.clone();
        self.spawner.spawn(Box::pin(async move {
            let outcome = fut.await;
            let _ = tx.unbounded_send(CoreEvent::ActivationCompleted { ticket, outcome });
        }));
    }

    // ------------------------------------------------------------------
    // 事件处理(§96 ticket 校验;§103 activation 处置)
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
        // §96:四项全部匹配才接受。epoch 与 registry 当前值比较——
        // reload 之后旧实例的结果必死(§49)。
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
                // §115:非空默认选中第 0 项。
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
        true
    }

    fn on_activation_completed(&mut self, ticket: &ActivationTicket, outcome: ModuleOutcome) -> bool {
        // §103:usage 总是记录(激活真实发生过)。
        if let Some(req) = &outcome.usage {
            self.usage.record(&ticket.module_id, req);
        }
        // session 处置只对发起它的那个 session 生效(§103)。
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        if ticket.session_id != s.id {
            return false;
        }
        s.activation_in_flight = false;
        match &outcome.status {
            OutcomeStatus::Success => {
                if outcome.session == SessionDisposition::Close {
                    self.close_session();
                }
            }
            OutcomeStatus::Failed(e) => {
                // §115:失败默认 KeepOpen + 错误展示。
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
        // §109:epoch 与 registry 当前值比较,旧实例事件一律丢弃;
        // 非 active module 同样丢弃。
        let current_epoch = self.registry.epoch(module_id).unwrap_or(u64::MAX);
        if *module_id != s.active_module || module_epoch != current_epoch {
            return false;
        }
        match event {
            ModuleEvent::PresentationInvalidated { items } => {
                // 只关心当前可见的 item;命中则 UI 需要重跑 present()。
                items
                    .iter()
                    .any(|id| s.results.iter().any(|r| r.id() == *id))
            }
        }
    }

    fn on_host_event(&mut self, host: HostEvent) {
        match host {
            HostEvent::HotkeyPressed => self.hotkey_pressed(),
            HostEvent::ShowRequested => self.show_requested(),
            HostEvent::FocusLost => self.focus_lost(),
        }
    }

    // ------------------------------------------------------------------
    // 供 UI 读取(§5.5 / §13)
    // ------------------------------------------------------------------

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn session(&self) -> Option<&SessionState> {
        self.session.as_ref()
    }

    /// 对当前 active module 执行 present()。UI 只对可见行调用(§105 < 1 ms)。
    pub fn present(&self, item: &ModuleItem) -> Option<ResultPresentation> {
        let s = self.session.as_ref()?;
        self.registry
            .module(&s.active_module)
            .map(|m| m.present(item))
    }

    /// §112:取走待执行的 CoreEffect,由编排层执行。
    pub fn take_effects(&mut self) -> Vec<CoreEffect> {
        std::mem::take(&mut self.effects)
    }

    /// §50 usage 查询(主要供测试与 Settings UI)。
    pub fn usage_stat(
        &self,
        module: &ModuleId,
        item_key: &str,
        action: ActionId,
    ) -> Option<UsageStat> {
        self.usage.stat(module, item_key, action)
    }

    /// 测试与 §42 ReloadModule 策略用:替换模块实例并递增 epoch。
    /// 旧实例的在途 query / 事件随 epoch 失效(§96、§109);
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

/// §5.2 Input Routing:Core 只解析 Module Trigger;
/// trigger 之后的剩余输入原样交给 Module(如 `ext:pdf` 语义不属于 Core)。
fn route(registry: &ModuleRegistry, input: &str) -> (ModuleId, String) {
    for (id, descriptor) in registry.launcher_descriptors() {
        if let Some(trigger) = &descriptor.trigger {
            if let Some(rest) = input.strip_prefix(trigger.as_str()) {
                return (id.clone(), rest.to_string());
            }
        }
    }
    let default = registry
        .default_module()
        .cloned()
        .expect("registry has no default module");
    (default, input.to_string())
}

/// §109:sink 在 load 时绑定 (ModuleId, ModuleEpoch)。
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

/// §64 统一 logger 的最小实现(写 stderr;日志文件随后续迭代落地)。
struct StderrLogger {
    module: String,
}

impl ModuleLog for StderrLogger {
    fn log(&self, level: LogLevel, message: &str) {
        eprintln!("[{level:?}] [{}] {message}", self.module);
    }
}
