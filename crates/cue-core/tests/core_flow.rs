//! §106 可测试性:异步模型的注入点(Spawner、事件队列)同时是测试点。
//! 这些测试不启动 GPUI、不创建窗口。

use cue_core::*;
use cue_protocol::*;
use futures::channel::oneshot;
use futures::future::BoxFuture;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------
// 手动 Spawner(§97 测试实现):不真正执行,只排队;
// poll_all 把队列中的 future 各 poll 一次,未完成的保留——
// 从而精确控制"哪些完成、哪些仍在途",且永远不会阻塞。
// ---------------------------------------------------------------------

struct ManualSpawner {
    queue: Mutex<Vec<BoxFuture<'static, ()>>>,
}

impl ManualSpawner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(Vec::new()),
        })
    }

    fn poll_all(&self) {
        let waker = futures::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        let mut queue = self.queue.lock().unwrap();
        let mut remaining = Vec::new();
        for mut f in queue.drain(..) {
            if f.as_mut().poll(&mut cx).is_pending() {
                remaining.push(f);
            }
        }
        queue.extend(remaining);
    }
}

impl TaskSpawner for ManualSpawner {
    fn spawn(&self, fut: BoxFuture<'static, ()>) {
        self.queue.lock().unwrap().push(fut);
    }
}

// ---------------------------------------------------------------------
// FakeModule:可预设 query 计划的演示模块。
// ---------------------------------------------------------------------

struct FakeItem {
    title: String,
}

enum QueryPlan {
    Ready(Vec<ModuleItem>),
    Pending(oneshot::Receiver<QueryResult>),
}

struct FakeModule {
    descriptor: ModuleDescriptor,
    plans: Mutex<VecDeque<QueryPlan>>,
    pending_activations: Mutex<VecDeque<oneshot::Receiver<ModuleOutcome>>>,
    sink: Arc<Mutex<Option<ModuleEventSink>>>,
}

impl FakeModule {
    fn new(id: &'static str) -> Self {
        Self {
            descriptor: ModuleDescriptor {
                id: ModuleId::from_static(id),
                name: "Fake",
                version: "0.1.0",
            },
            plans: Mutex::new(VecDeque::new()),
            pending_activations: Mutex::new(VecDeque::new()),
            sink: Arc::new(Mutex::new(None)),
        }
    }

    fn push_ready(&self, titles: &[&str]) {
        self.plans
            .lock()
            .unwrap()
            .push_back(QueryPlan::Ready(make_items(titles)));
    }

    fn push_pending(&self) -> oneshot::Sender<QueryResult> {
        let (tx, rx) = oneshot::channel();
        self.plans.lock().unwrap().push_back(QueryPlan::Pending(rx));
        tx
    }

    fn push_pending_activation(&self) -> oneshot::Sender<ModuleOutcome> {
        let (tx, rx) = oneshot::channel();
        self.pending_activations.lock().unwrap().push_back(rx);
        tx
    }

    fn sink_handle(&self) -> Arc<Mutex<Option<ModuleEventSink>>> {
        Arc::clone(&self.sink)
    }
}

fn make_items(titles: &[&str]) -> Vec<ModuleItem> {
    titles
        .iter()
        .enumerate()
        .map(|(i, t)| {
            ModuleItem::new(
                ItemId(i as u64),
                FakeItem {
                    title: t.to_string(),
                },
            )
        })
        .collect()
}

fn ready_items(titles: &[&str]) -> QueryResult {
    Ok(QueryResponse {
        items: make_items(titles),
    })
}

impl Module for FakeModule {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn load(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        *self.sink.lock().unwrap() = Some(ctx.events);
        Ok(())
    }

    fn unload(&mut self) {}

    fn settings_schema(&self) -> SettingsSchema {
        Vec::new()
    }

    fn try_apply_settings(&mut self, _changes: SettingsChangeSet) -> Result<(), ModuleError> {
        Ok(())
    }
}

impl LauncherModule for FakeModule {
    fn launcher_descriptor(&self) -> LauncherDescriptor {
        LauncherDescriptor {
            trigger: None,
            is_default: true,
        }
    }

    fn query(&mut self, _ctx: QueryContext) -> QueryFuture {
        match self.plans.lock().unwrap().pop_front() {
            Some(QueryPlan::Ready(items)) => Box::pin(async move { Ok(QueryResponse { items }) }),
            Some(QueryPlan::Pending(rx)) => Box::pin(async move {
                rx.await
                    .unwrap_or_else(|_| Err(ModuleError::Internal("sender dropped".into())))
            }),
            None => Box::pin(async move { Ok(QueryResponse { items: vec![] }) }),
        }
    }

    fn present(&self, item: &ModuleItem) -> ResultPresentation {
        let fake = item
            .downcast_ref::<FakeItem>()
            .expect("FakeModule received a foreign ModuleItem");
        ResultPresentation::new(fake.title.clone())
    }

    fn actions(&self, _item: &ModuleItem) -> Vec<ActionDescriptor> {
        vec![ActionDescriptor {
            id: ActionId::PRIMARY,
            label: "Open".into(),
            shortcut: None,
        }]
    }

    fn activate(&mut self, item: &ModuleItem, action: ActionId) -> ActivationFuture {
        if let Some(rx) = self.pending_activations.lock().unwrap().pop_front() {
            return Box::pin(async move {
                rx.await
                    .unwrap_or_else(|_| ModuleOutcome::failed(ModuleError::Internal("dropped".into())))
            });
        }
        let title = item
            .downcast_ref::<FakeItem>()
            .map(|f| f.title.clone())
            .unwrap_or_default();
        Box::pin(async move {
            ModuleOutcome::success(
                SessionDisposition::Close,
                Some(UsageRecordRequest {
                    item_key: title,
                    action_id: action,
                }),
            )
        })
    }
}

// ---------------------------------------------------------------------
// 测试工具
// ---------------------------------------------------------------------

fn test_config() -> CoreConfig {
    CoreConfig {
        storage_root: std::env::temp_dir().join(format!("cue-core-test-{}", std::process::id())),
        ..CoreConfig::default()
    }
}

fn setup(module: FakeModule) -> (Core, Arc<ManualSpawner>) {
    let spawner = ManualSpawner::new();
    let mut registry = ModuleRegistry::new();
    registry.register(Box::new(module)).unwrap();
    let core = Core::new(test_config(), registry, spawner.clone()).unwrap();
    (core, spawner)
}

fn drain(core: &mut Core, rx: &mut futures::channel::mpsc::UnboundedReceiver<CoreEvent>) {
    while let Ok(ev) = rx.try_recv() {
        core.handle_event(ev);
    }
}

fn results(core: &Core) -> usize {
    core.session().map(|s| s.results.len()).unwrap_or(0)
}

fn selected(core: &Core) -> Option<usize> {
    core.session().and_then(|s| s.selected)
}

// ---------------------------------------------------------------------
// §96 ticket 校验
// ---------------------------------------------------------------------

#[test]
fn open_session_runs_empty_query_and_selects_first() {
    let module = FakeModule::new("fake");
    module.push_ready(&["Alpha", "Beta"]);
    let (mut core, spawner) = setup(module);
    let mut rx = core.take_event_receiver();

    core.open_session();
    spawner.poll_all();
    drain(&mut core, &mut rx);

    assert!(core.is_visible());
    assert_eq!(results(&core), 2);
    // §115:非空默认选中第 0 项。
    assert_eq!(selected(&core), Some(0));

    let effects = core.take_effects();
    assert!(effects.contains(&CoreEffect::ShowLauncher));
    assert!(effects.contains(&CoreEffect::FocusInput));
}

#[test]
fn stale_generation_is_dropped() {
    let module = FakeModule::new("fake");
    let tx_empty = module.push_pending(); // 空查询(gen 0)
    let tx_a = module.push_pending(); // 输入 "a"(gen 1)
    let (mut core, spawner) = setup(module);
    let mut rx = core.take_event_receiver();

    core.open_session();
    core.input_changed("a".into());

    // 新 query(gen 1)先完成。
    tx_a.send(ready_items(&["New"])).unwrap();
    spawner.poll_all();
    drain(&mut core, &mut rx);
    assert_eq!(results(&core), 1);

    // 旧 query(gen 0)后完成,必须被丢弃。
    tx_empty.send(ready_items(&["Old", "Older"])).unwrap();
    spawner.poll_all();
    drain(&mut core, &mut rx);
    assert_eq!(results(&core), 1);
}

#[test]
fn cross_session_same_generation_is_dropped() {
    let module = FakeModule::new("fake");
    let tx_a = module.push_pending(); // session A 空查询(gen 0)
    let tx_b = module.push_pending(); // session B 空查询(gen 0)
    let (mut core, spawner) = setup(module);
    let mut rx = core.take_event_receiver();

    core.open_session();
    core.close_session();
    core.open_session();

    tx_b.send(ready_items(&["B"])).unwrap();
    spawner.poll_all();
    drain(&mut core, &mut rx);
    assert_eq!(results(&core), 1);

    // session A 的结果带着相同的 generation 到达——session_id 拦截(§96)。
    tx_a.send(ready_items(&["A", "A2"])).unwrap();
    spawner.poll_all();
    drain(&mut core, &mut rx);
    assert_eq!(results(&core), 1);
}

#[test]
fn module_epoch_is_dropped_after_reload() {
    let module = FakeModule::new("fake");
    let tx_old = module.push_pending(); // 旧实例的空查询
    let (mut core, spawner) = setup(module);
    let mut rx = core.take_event_receiver();

    core.open_session();

    // reload:epoch 递增,旧实例的在途 query 必死(§49、§96)。
    let new_module = FakeModule::new("fake");
    new_module.push_ready(&["Fresh"]);
    core.reload_module(Box::new(new_module)).unwrap();

    tx_old.send(ready_items(&["Stale"])).unwrap();
    spawner.poll_all();
    drain(&mut core, &mut rx);
    assert_eq!(results(&core), 0);

    // 新实例正常工作。
    core.input_changed("f".into());
    spawner.poll_all();
    drain(&mut core, &mut rx);
    assert_eq!(results(&core), 1);
}

// ---------------------------------------------------------------------
// §102 / §115 输入与选择
// ---------------------------------------------------------------------

#[test]
fn input_change_clears_results_immediately() {
    let module = FakeModule::new("fake");
    module.push_ready(&["Alpha"]);
    let _tx = module.push_pending(); // 下一次 query 永不完成
    let (mut core, spawner) = setup(module);
    let mut rx = core.take_event_receiver();

    core.open_session();
    spawner.poll_all();
    drain(&mut core, &mut rx);
    assert_eq!(results(&core), 1);
    assert_eq!(selected(&core), Some(0));

    // §102:输入变化立即清空,stale 结果永不可激活。
    core.input_changed("x".into());
    assert_eq!(results(&core), 0);
    assert_eq!(selected(&core), None);
}

#[test]
fn selection_clamps_at_edges() {
    let module = FakeModule::new("fake");
    module.push_ready(&["A", "B", "C"]);
    let (mut core, spawner) = setup(module);
    let mut rx = core.take_event_receiver();

    core.open_session();
    spawner.poll_all();
    drain(&mut core, &mut rx);

    core.select_next();
    core.select_next();
    core.select_next(); // 钳制在末尾
    assert_eq!(selected(&core), Some(2));
    core.select_prev();
    core.select_prev();
    core.select_prev(); // 钳制在开头
    assert_eq!(selected(&core), Some(0));
}

// ---------------------------------------------------------------------
// §103 activation
// ---------------------------------------------------------------------

#[test]
fn activation_close_records_usage_and_hides() {
    let module = FakeModule::new("fake");
    module.push_ready(&["Alpha"]);
    let (mut core, spawner) = setup(module);
    let mut rx = core.take_event_receiver();

    core.open_session();
    spawner.poll_all();
    drain(&mut core, &mut rx);
    core.take_effects();

    core.activate_selected();
    spawner.poll_all();
    drain(&mut core, &mut rx);

    // success + Close → session 关闭(§103)。
    assert!(core.session().is_none());
    assert!(core.take_effects().contains(&CoreEffect::HideLauncher));
    // usage 总是记录(§103)。
    let stat = core
        .usage_stat(&ModuleId::from_static("fake"), "Alpha", ActionId::PRIMARY)
        .expect("usage recorded");
    assert_eq!(stat.count, 1);
}

#[test]
fn activation_from_old_session_does_not_close_new_session() {
    let module = FakeModule::new("fake");
    module.push_ready(&["A"]);
    module.push_ready(&["B"]);
    let tx_act = module.push_pending_activation();
    let (mut core, spawner) = setup(module);
    let mut rx = core.take_event_receiver();

    // Session A:激活 "A"(activation 在途),立即 Esc。
    core.open_session();
    spawner.poll_all();
    drain(&mut core, &mut rx);
    core.activate_selected();
    core.close_session();

    // Session B 打开。
    core.open_session();
    spawner.poll_all();
    drain(&mut core, &mut rx);
    core.take_effects();

    // session A 的 activation 晚于 session B 打开才完成。
    tx_act
        .send(ModuleOutcome::success(
            SessionDisposition::Close,
            Some(UsageRecordRequest {
                item_key: "a-key".into(),
                action_id: ActionId::PRIMARY,
            }),
        ))
        .unwrap();
    spawner.poll_all();
    drain(&mut core, &mut rx);

    // usage 仍然记录(§103),但 session B 不被误关(§103 / v0.2 P0 修复)。
    assert!(core.session().is_some());
    assert!(!core.take_effects().contains(&CoreEffect::HideLauncher));
    assert!(core
        .usage_stat(&ModuleId::from_static("fake"), "a-key", ActionId::PRIMARY)
        .is_some());
}

#[test]
fn failed_activation_keeps_session_open_with_error() {
    let module = FakeModule::new("fake");
    module.push_ready(&["Alpha"]);
    let tx_act = module.push_pending_activation();
    let (mut core, spawner) = setup(module);
    let mut rx = core.take_event_receiver();

    core.open_session();
    spawner.poll_all();
    drain(&mut core, &mut rx);
    core.activate_selected();

    tx_act
        .send(ModuleOutcome::failed(ModuleError::ActivationFailed(
            "boom".into(),
        )))
        .unwrap();
    spawner.poll_all();
    drain(&mut core, &mut rx);

    // §115:失败默认 KeepOpen + 错误展示。
    assert!(core.session().is_some());
    assert!(core.session().unwrap().error.is_some());
    assert!(!core.take_effects().contains(&CoreEffect::HideLauncher));
}

// ---------------------------------------------------------------------
// §109 Module 自发事件
// ---------------------------------------------------------------------

#[test]
fn presentation_invalidated_only_for_visible_items() {
    let module = FakeModule::new("fake");
    module.push_ready(&["Alpha", "Beta"]);
    let sink = module.sink_handle();
    let (mut core, spawner) = setup(module);
    let mut rx = core.take_event_receiver();

    core.open_session();
    spawner.poll_all();
    drain(&mut core, &mut rx);

    let sink = sink.lock().unwrap().clone().expect("sink bound at load");

    // 可见 item(ItemId 1 = "Beta")→ changed。
    sink.send(ModuleEvent::PresentationInvalidated {
        items: vec![ItemId(1)],
    });
    let ev = rx.try_recv().unwrap();
    assert!(core.handle_event(ev));

    // 不可见 item → 忽略。
    sink.send(ModuleEvent::PresentationInvalidated {
        items: vec![ItemId(999)],
    });
    let ev = rx.try_recv().unwrap();
    assert!(!core.handle_event(ev));

    // session 关闭后 → 丢弃。
    core.close_session();
    sink.send(ModuleEvent::PresentationInvalidated {
        items: vec![ItemId(0)],
    });
    let ev = rx.try_recv().unwrap();
    assert!(!core.handle_event(ev));
}

// ---------------------------------------------------------------------
// §53 toggle / §54 失焦
// ---------------------------------------------------------------------

#[test]
fn hotkey_toggles_and_focus_loss_hides() {
    let module = FakeModule::new("fake");
    module.push_ready(&[]);
    module.push_ready(&[]);
    let (mut core, spawner) = setup(module);
    let mut rx = core.take_event_receiver();

    // 隐藏 → 打开
    core.hotkey_pressed();
    assert!(core.is_visible());
    let effects = core.take_effects();
    assert!(effects.contains(&CoreEffect::ShowLauncher));

    // 可见且聚焦 → 关闭
    core.hotkey_pressed();
    assert!(!core.is_visible());
    assert!(core.take_effects().contains(&CoreEffect::HideLauncher));

    // 再次打开,失焦 → 隐藏(§54 hide_on_focus_loss 默认 true)
    core.hotkey_pressed();
    assert!(core.is_visible());
    spawner.poll_all();
    drain(&mut core, &mut rx);
    core.take_effects();
    core.focus_lost();
    assert!(!core.is_visible());
    assert!(core.take_effects().contains(&CoreEffect::HideLauncher));
}

// ---------------------------------------------------------------------
// present 路由
// ---------------------------------------------------------------------

#[test]
fn present_delegates_to_active_module() {
    let module = FakeModule::new("fake");
    module.push_ready(&["Alpha"]);
    let (mut core, spawner) = setup(module);
    let mut rx = core.take_event_receiver();

    core.open_session();
    spawner.poll_all();
    drain(&mut core, &mut rx);

    let item = core.session().unwrap().results[0].clone();
    let presentation = core.present(&item).expect("presentation");
    assert_eq!(&*presentation.title, "Alpha");
}

// ---------------------------------------------------------------------
// §42 设置事务:validate → try-apply → commit → persist
// ---------------------------------------------------------------------

fn setup_with_config(module: FakeModule, config: CoreConfig) -> (Core, Arc<ManualSpawner>) {
    let spawner = ManualSpawner::new();
    let mut registry = ModuleRegistry::new();
    registry.register(Box::new(module)).unwrap();
    let core = Core::new(config, registry, spawner.clone()).unwrap();
    (core, spawner)
}

#[test]
fn hotkey_apply_failure_keeps_old_value() {
    // try-apply 失败(热键被占用):不 commit,旧值保留(§42/§53)。
    let mut config = test_config();
    config.apply_hotkey = Some(Box::new(|_| Err("occupied by another app".to_string())));
    let (mut core, _spawner) = setup_with_config(FakeModule::new("fake"), config);

    core.open_settings();
    let before = core.hotkey();
    let candidate: Hotkey = "ctrl+alt+k".parse().unwrap();
    let err = core
        .apply_setting("core.hotkey", SettingValue::Hotkey(candidate))
        .expect_err("must fail");
    assert!(err.contains("occupied"));
    assert_eq!(core.hotkey(), before); // 旧值保留
    // 错误进入模型,UI 据此展示;再次成功 apply 后错误清除。
    let model = core.settings_model().unwrap();
    assert!(model.error.is_some());
}

#[test]
fn hotkey_apply_success_commits() {
    let applied = Arc::new(Mutex::new(Vec::new()));
    let mut config = test_config();
    let seen = Arc::clone(&applied);
    config.apply_hotkey = Some(Box::new(move |h: &Hotkey| {
        seen.lock().unwrap().push(*h);
        Ok(())
    }));
    let (mut core, _spawner) = setup_with_config(FakeModule::new("fake"), config);

    let candidate: Hotkey = "ctrl+shift+f5".parse().unwrap();
    core.apply_setting("core.hotkey", SettingValue::Hotkey(candidate))
        .unwrap();
    assert_eq!(core.hotkey(), candidate);
    assert_eq!(applied.lock().unwrap().as_slice(), &[candidate]);
}

#[test]
fn apply_setting_validates_type_and_value() {
    let (mut core, _spawner) = setup(FakeModule::new("fake"));

    // 类型不匹配
    assert!(core
        .apply_setting("core.hotkey", SettingValue::Bool(true))
        .is_err());
    // 取值不合法:无修饰键
    let no_mod = Hotkey {
        modifiers: Modifiers::NONE,
        key: Key::Space,
    };
    assert!(core
        .apply_setting("core.hotkey", SettingValue::Hotkey(no_mod))
        .is_err());
    // 未知 key
    assert!(core
        .apply_setting("core.nope", SettingValue::Bool(true))
        .is_err());
    // 默认值未被破坏
    assert_eq!(core.hotkey(), Hotkey::default());
}

#[test]
fn settings_view_lifecycle_and_effects() {
    let (mut core, spawner) = setup(FakeModule::new("fake"));
    let mut rx = core.take_event_receiver();

    // 打开设置:搜索会话退场,窗口显示 + 聚焦。
    core.open_session();
    spawner.poll_all();
    drain(&mut core, &mut rx);
    core.take_effects();

    core.open_settings();
    assert!(core.in_settings());
    assert!(core.session().is_none());
    let model = core.settings_model().unwrap();
    assert_eq!(model.rows.len(), 2); // core.hotkey + core.hide_on_focus_loss
    assert_eq!(model.selected, 0);

    core.settings_select_next();
    core.settings_select_next(); // 夹紧在最后一行
    assert_eq!(core.settings_model().unwrap().selected, 1);
    core.settings_select_prev();
    assert_eq!(core.settings_model().unwrap().selected, 0);

    // 热键在设置页 = Esc(关闭设置)。
    core.hotkey_pressed();
    assert!(!core.in_settings());
    assert!(core.take_effects().contains(&CoreEffect::HideLauncher));

    // Bool 切换立即生效并体现在 Core 行为上(§54)。
    core.open_settings();
    core.settings_select_next(); // hide_on_focus_loss 行
    core
        .apply_setting("core.hide_on_focus_loss", SettingValue::Bool(false))
        .unwrap();
    core.dismiss_settings();
    core.open_session();
    spawner.poll_all();
    drain(&mut core, &mut rx);
    core.take_effects();
    core.focus_lost(); // 关闭后:失焦不隐藏
    assert!(core.is_visible());
}
