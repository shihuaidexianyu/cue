//! 锁键服务(§140):WH_KEYBOARD_LL 全局钩子 + 专用线程。
//!
//! 手势引擎移植自 WinCaps(https://github.com/coekfung/WinCaps,MIT):
//! 中文输入法前台时,轻点触发键 = 注入输入法切换快捷键(Ctrl+Space /
//! Shift 可配),长按 = 切换该键的锁定态;其余前台(含我们自己被
//! §107 强制英文的 launcher)原样透传。扩展:触发键可配(CapsLock /
//! ScrollLock)、NumLock 三模式(原生 / 长按才切换 / 常开守护)、
//! 状态变化 OSD 上报。
//!
//! 与 WinCaps 的两处结构性差异:
//! - 配置不经消息投递:Mutex 共享 (LockKeysConfig, epoch),钩子每次
//!   事件读最新值;epoch 变化 = 丢弃挂起手势(配置变更 / 会话复位),
//!   全部取消逻辑都收在 worker 线程一侧,无跨线程状态刺穿。
//! - OSD 触发用状态 diff(GetKeyState 逐事件比对)取代 confirm
//!   定时器:任何来源的切换(我们的注入 / 真实按键 / 别的程序)
//!   都上报,无遗漏也无专用确认路径。
//!
//! 线程模型与 WinCaps 一致:钩子与 message-only 窗口同线程,
//! GetMessage 泵投递钩子回调与定时器;钩子回调里只做 O(1) 判定与
//! PostMessage,绝不阻塞系统输入。

use crate::host::WM_SAKANA_LOCKKEY;
use sakana_protocol::{LockKeysConfig, LockRemapKey, LockTapAction, NumLockMode};
use std::cell::RefCell;
use std::mem::size_of;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

// LockKey 定义在 protocol(纯数据,OSD 视图也按它区分文案);
// 此处再导出,host.rs 等引用 crate::lockkeys::LockKey 的地方不动。
pub use sakana_protocol::LockKey;

/// 自家 SendInput 的标记:钩子见到即跳过手势逻辑(状态 diff 仍跑,
/// 我们的注入引起的锁定态变化也要上报 OSD)。
const INJECTED: usize = 0x534B_4E41; // "SKNA"
const HOLD_TIMER: usize = 1;
const NUM_HOLD_TIMER: usize = 2;

thread_local! { static STATE: RefCell<Option<Keyboard>> = const { RefCell::new(None) }; }

/// 共享配置 + 代次。UI 线程写(set_config / reset),worker 逐事件读。
type Shared = Arc<Mutex<(LockKeysConfig, u64)>>;

struct Keyboard {
    window: HWND,
    host: HWND,
    shared: Shared,
    seen_epoch: u64,
    /// 挂起的触发键手势(按下被吞、等待轻点/长按判定)。
    press: Option<Press>,
    /// 挂起的 NumLock 手势(hold 模式)。
    num_press: Option<Press>,
    /// 吞下的触发键按下对应的放行标记:透传模式下按下没吞,
    /// 抬起也不吞(一次按键要么全程原生,要么全程被吞)。
    passthrough: bool,
    num_passthrough: bool,
    foreground: HWND,
    last_caps: bool,
    last_num: bool,
}

/// 手势判定结果(纯数据,执行在 worker 窗口过程里)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GestureAction {
    /// 轻点:注入输入法切换快捷键。
    Tap,
    /// 把该键的锁定态设为指定值(注入一次按键翻转)。
    SetToggle(bool),
}

/// 轻点松开时的两种语义:触发键轻点 = 注入;NumLock 防误触轻点 = 吞掉。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TapMeaning {
    Inject,
    Swallow,
}

/// 轻点/长按状态机(移植 WinCaps state.rs 的 Press):
/// 判定一律用事件时间戳(GetTickCount 环绕安全),不依赖定时器送达顺序。
pub(crate) struct Press {
    started: u32,
    toggle_on: bool,
    handled: bool,
    hold_ms: u32,
    tap: TapMeaning,
}

impl Press {
    pub(crate) fn new(started: u32, toggle_on: bool, hold_ms: u32, tap: TapMeaning) -> Self {
        Self {
            started,
            toggle_on,
            handled: false,
            hold_ms,
            tap,
        }
    }

    pub(crate) fn cancel(&mut self) {
        self.handled = true;
    }

    pub(crate) fn finished(&self) -> bool {
        self.handled
    }

    /// 定时器到点:跨过阈值 = 长按成立,立即翻转锁定态。
    pub(crate) fn hold(&mut self, now: u32) -> Option<GestureAction> {
        if self.handled || now.wrapping_sub(self.started) < self.hold_ms {
            return None;
        }
        self.handled = true;
        Some(GestureAction::SetToggle(!self.toggle_on))
    }

    /// 松开:已处理(长按已触发 / 被取消)则无动作;跨过阈值按长按补判。
    /// 阈值内松开 = 轻点:Inject 语义下,锁定态已开 = 关掉,未开 = 注入;
    /// Swallow 语义(NumLock 防误触)下轻点一律吞掉,与当前态无关。
    pub(crate) fn release(mut self, now: u32) -> Option<GestureAction> {
        if self.handled {
            return None;
        }
        if now.wrapping_sub(self.started) >= self.hold_ms {
            return self.hold(now);
        }
        match self.tap {
            TapMeaning::Inject if self.toggle_on => Some(GestureAction::SetToggle(false)),
            TapMeaning::Inject => Some(GestureAction::Tap),
            TapMeaning::Swallow => None,
        }
    }
}

/// 修饰键是否按下(手势取消条件之一)。
fn modifiers_down() -> bool {
    unsafe {
        [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
            .iter()
            .any(|key| GetAsyncKeyState(key.0 as i32) < 0)
    }
}

/// 指定锁定键当前的开关态。
fn toggle_on(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) & 1 != 0 }
}

/// 前台窗口的线程输入法是否中文(PRIMARYLANGID == 0x04 覆盖全部中文子语言)。
fn chinese_input(window: HWND) -> bool {
    unsafe {
        let thread = GetWindowThreadProcessId(window, None);
        thread != 0 && is_chinese_layout(GetKeyboardLayout(thread))
    }
}

fn is_chinese_layout(layout: HKL) -> bool {
    layout.0 as usize & 0x03ff == 0x04
}

fn trigger_vk(key: LockRemapKey) -> VIRTUAL_KEY {
    match key {
        LockRemapKey::CapsLock => VK_CAPITAL,
        LockRemapKey::ScrollLock => VK_SCROLL,
    }
}

pub struct Worker {
    window: HWND,
    thread: Option<thread::JoinHandle<()>>,
    shared: Shared,
}

impl Worker {
    /// host = 接收 WM_SAKANA_LOCKKEY 的 host 窗口(已存在于 UI 线程)。
    pub fn start(host: HWND, config: LockKeysConfig) -> Result<Self> {
        let shared: Shared = Arc::new(Mutex::new((config, 0)));
        let (tx, rx) = mpsc::sync_channel(1);
        let host_raw = host.0 as usize;
        let shared_clone = Arc::clone(&shared);
        let thread = thread::spawn(move || {
            // SAFETY: 钩子、窗口、消息循环都由本线程持有到清理结束。
            unsafe {
                let setup = (|| -> Result<(HWND, HHOOK)> {
                    let instance = GetModuleHandleW(None)?;
                    let class = WNDCLASSW {
                        lpfnWndProc: Some(worker_wnd_proc),
                        hInstance: instance.into(),
                        lpszClassName: w!("sakana.LockKeys"),
                        ..Default::default()
                    };
                    if RegisterClassW(&class) == 0 {
                        return Err(Error::from_thread());
                    }
                    let window = CreateWindowExW(
                        WINDOW_EX_STYLE::default(),
                        class.lpszClassName,
                        w!(""),
                        WINDOW_STYLE::default(),
                        0,
                        0,
                        0,
                        0,
                        Some(HWND_MESSAGE),
                        None,
                        Some(instance.into()),
                        None,
                    )?;
                    STATE.with_borrow_mut(|state| {
                        *state = Some(Keyboard {
                            window,
                            host: HWND(host_raw as _),
                            shared: shared_clone,
                            seen_epoch: 0,
                            press: None,
                            num_press: None,
                            passthrough: false,
                            num_passthrough: false,
                            foreground: HWND::default(),
                            last_caps: toggle_on(VK_CAPITAL),
                            last_num: toggle_on(VK_NUMLOCK),
                        })
                    });
                    match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), Some(instance.into()), 0) {
                        Ok(hook) => Ok((window, hook)),
                        Err(error) => {
                            let _ = DestroyWindow(window);
                            Err(error)
                        }
                    }
                })();
                match setup {
                    Ok((window, hook)) => {
                        // 常开守护的启动巡检:开局就是关的,立刻打回开。
                        STATE.with_borrow_mut(|state| {
                            let state = state.as_mut().unwrap();
                            let (config, _) = *state.shared.lock().unwrap();
                            if config.enabled
                                && config.numlock_mode == NumLockMode::AlwaysOn
                                && !toggle_on(VK_NUMLOCK)
                            {
                                send(&[(VK_NUMLOCK, false), (VK_NUMLOCK, true)]);
                            }
                        });
                        let _ = tx.send(Ok(window.0 as usize));
                        let mut msg = MSG::default();
                        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                            DispatchMessageW(&msg);
                        }
                        let _ = UnhookWindowsHookEx(hook);
                        let _ = DestroyWindow(window);
                        STATE.with_borrow_mut(|state| *state = None);
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error));
                    }
                }
            }
        });
        let window = rx
            .recv()
            .map_err(|_| Error::new(E_FAIL, "lockkeys thread did not start"))??;
        Ok(Self {
            window: HWND(window as _),
            thread: Some(thread),
            shared,
        })
    }

    /// 推送新配置。实现 = 存值 + 代次 +1:代次变化让 worker 丢弃
    /// 挂起手势(改配置时手势语义已经变了,不该用旧语义结算)。
    pub fn set_config(&self, config: LockKeysConfig) {
        let mut g = self.shared.lock().unwrap();
        g.0 = config;
        g.1 += 1;
    }

    /// 会话复位(锁屏/休眠):释放可能永远等不到抬起的挂起手势。
    pub fn reset(&self) {
        let mut g = self.shared.lock().unwrap();
        g.1 += 1;
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // SAFETY: worker 窗口活着,直到本 owner 的 Drop;WM_CLOSE 退消息循环。
        unsafe {
            let _ = PostMessageW(Some(self.window), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// 逐事件读最新配置;代次变了 = 丢弃挂起手势(在 worker 线程完成
/// KillTimer 等清理,跨线程不碰定时器)。
fn sync_config(state: &mut Keyboard) -> LockKeysConfig {
    let (config, epoch) = *state.shared.lock().unwrap();
    if epoch != state.seen_epoch {
        state.seen_epoch = epoch;
        if let Some(press) = &mut state.press {
            press.cancel();
        }
        if let Some(press) = &mut state.num_press {
            press.cancel();
        }
        state.press = None;
        state.num_press = None;
        state.passthrough = false;
        state.num_passthrough = false;
        unsafe {
            let _ = KillTimer(Some(state.window), HOLD_TIMER);
            let _ = KillTimer(Some(state.window), NUM_HOLD_TIMER);
        }
    }
    config
}

/// 锁定键状态 diff:任何事件(含自家注入)后比对,变了就上报 host。
/// 总开关或 osd 关闭时不上报(diff 本身仍维护,免得重开时补报旧账)。
fn report_state_changes(state: &mut Keyboard, config: &LockKeysConfig) {
    let caps = toggle_on(VK_CAPITAL);
    let num = toggle_on(VK_NUMLOCK);
    let report = config.enabled && config.osd;
    if report && caps != state.last_caps {
        post_lockkey(state.host, 0, caps);
    }
    if report && num != state.last_num {
        post_lockkey(state.host, 1, num);
    }
    state.last_caps = caps;
    state.last_num = num;
}

fn post_lockkey(host: HWND, key: usize, on: bool) {
    // SAFETY: 只投递整数;host 窗口与进程同寿。
    unsafe {
        let _ = PostMessageW(
            Some(host),
            WM_SAKANA_LOCKKEY,
            WPARAM(key),
            LPARAM(on as isize),
        );
    }
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: HC_ACTION 时 lparam 是有效的 KBDLLHOOKSTRUCT,回调期内有效。
    unsafe {
        if code != HC_ACTION as i32 {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let injected = event.dwExtraInfo == INJECTED;
        let swallowed = STATE.with_borrow_mut(|state| {
            let state = state.as_mut().unwrap();
            let config = sync_config(state);
            let down = wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN;
            let vk = VIRTUAL_KEY(event.vkCode as u16);
            let mut swallowed = false;
            if !injected {
                swallowed = handle_event(state, &config, vk, down, event.time);
            }
            // NumLock 常开守护:任何事件顺带巡检(含自家注入的后续事件,
            // 注入引起的翻转在下一次事件被抓到并再次打回——状态 diff
            // 先上报,OSD 会短暂显示"关"再显示"开",与真实时序一致)。
            if config.enabled
                && config.numlock_mode == NumLockMode::AlwaysOn
                && !toggle_on(VK_NUMLOCK)
            {
                send(&[(VK_NUMLOCK, false), (VK_NUMLOCK, true)]);
            }
            report_state_changes(state, &config);
            swallowed
        });
        if swallowed {
            LRESULT(1)
        } else {
            CallNextHookEx(None, code, wparam, lparam)
        }
    }
}

/// 单个非注入按键事件的判定;返回 true = 吞掉(不传给后续钩子/系统)。
fn handle_event(
    state: &mut Keyboard,
    config: &LockKeysConfig,
    vk: VIRTUAL_KEY,
    down: bool,
    time: u32,
) -> bool {
    // 修饰键按下取消挂起手势(Ctrl+C 里的 Ctrl 不该让 CapsLock 攒着)。
    if down
        && [
            VK_LCONTROL,
            VK_RCONTROL,
            VK_LSHIFT,
            VK_RSHIFT,
            VK_LMENU,
            VK_RMENU,
            VK_LWIN,
            VK_RWIN,
        ]
        .contains(&vk)
    {
        if let Some(press) = &mut state.press {
            press.cancel();
        }
        if let Some(press) = &mut state.num_press {
            press.cancel();
        }
        return false;
    }

    if vk == trigger_vk(config.remap_key) {
        return handle_trigger(state, config, down, time);
    }
    if vk == VK_NUMLOCK {
        return handle_numlock(state, config, down, time);
    }
    false
}

fn handle_trigger(state: &mut Keyboard, config: &LockKeysConfig, down: bool, time: u32) -> bool {
    if down {
        if state.passthrough {
            return false;
        }
        // 按住自动重复:第一次按下已吞,后续重复报文同样吞掉。
        if state.press.is_some() {
            return true;
        }
        let foreground = unsafe { GetForegroundWindow() };
        if !config.enabled || modifiers_down() || !chinese_input(foreground) {
            state.passthrough = true;
            return false;
        }
        state.press = Some(Press::new(
            time,
            toggle_on(trigger_vk(config.remap_key)),
            config.hold_ms,
            TapMeaning::Inject,
        ));
        state.foreground = foreground;
        // SAFETY: worker 窗口与本状态同线程同寿。
        if unsafe { SetTimer(Some(state.window), HOLD_TIMER, config.hold_ms, None) } == 0 {
            state.press = None;
            state.passthrough = true;
            return false;
        }
        true
    } else {
        let was_passthrough = state.passthrough;
        state.passthrough = false;
        let Some(mut press) = state.press.take() else {
            return false; // 透传的按下,抬起也透传
        };
        let _ = was_passthrough;
        unsafe {
            let _ = KillTimer(Some(state.window), HOLD_TIMER);
        }
        let foreground_now = unsafe { GetForegroundWindow() };
        if !config.enabled
            || foreground_now != state.foreground
            || modifiers_down()
            || !chinese_input(state.foreground)
        {
            press.cancel();
        }
        if let Some(action) = press.release(time) {
            execute(config, trigger_vk(config.remap_key), action);
        }
        true
    }
}

fn handle_numlock(state: &mut Keyboard, config: &LockKeysConfig, down: bool, time: u32) -> bool {
    match (config.enabled, config.numlock_mode) {
        (false, _) | (_, NumLockMode::Native) => false,
        (_, NumLockMode::AlwaysOn) => true, // 全吞;状态由守护巡检维持
        (_, NumLockMode::Hold) => {
            if down {
                if state.num_passthrough {
                    return false;
                }
                if state.num_press.is_some() {
                    return true; // 自动重复
                }
                state.num_press = Some(Press::new(
                    time,
                    toggle_on(VK_NUMLOCK),
                    config.hold_ms,
                    TapMeaning::Swallow,
                ));
                // SAFETY: 同 HOLD_TIMER。
                if unsafe { SetTimer(Some(state.window), NUM_HOLD_TIMER, config.hold_ms, None) }
                    == 0
                {
                    state.num_press = None;
                    state.num_passthrough = true;
                    return false;
                }
                true
            } else {
                state.num_passthrough = false;
                let Some(mut press) = state.num_press.take() else {
                    return false;
                };
                unsafe {
                    let _ = KillTimer(Some(state.window), NUM_HOLD_TIMER);
                }
                if modifiers_down() {
                    press.cancel();
                }
                if let Some(action) = press.release(time) {
                    execute(config, VK_NUMLOCK, action);
                }
                true
            }
        }
    }
}

/// 手势结算(吞下的按下在这里变现):轻点 = 注入 IME 切换;
/// SetToggle = 当前态与目标不符时注入一次该键翻转。
/// vk_for_toggle 由调用点给出(触发键手势 → 触发键;NumLock 手势
/// → VK_NUMLOCK)——结算时 Press 已被 take,目标只能靠调用点。
fn execute(config: &LockKeysConfig, vk_for_toggle: VIRTUAL_KEY, action: GestureAction) {
    match action {
        GestureAction::Tap => {
            let keys: &[(VIRTUAL_KEY, bool)] = match config.tap_action {
                LockTapAction::CtrlSpace => &[
                    (VK_CONTROL, false),
                    (VK_SPACE, false),
                    (VK_SPACE, true),
                    (VK_CONTROL, true),
                ],
                LockTapAction::Shift => &[(VK_SHIFT, false), (VK_SHIFT, true)],
            };
            if !send(keys)
                && let LockTapAction::CtrlSpace = config.tap_action
            {
                // 部分注入失败的收尾:把按下的键抬起来,不重播快捷键。
                send(&[(VK_SPACE, true), (VK_CONTROL, true)]);
            }
        }
        GestureAction::SetToggle(desired) => {
            if toggle_on(vk_for_toggle) != desired {
                send(&[(vk_for_toggle, false), (vk_for_toggle, true)]);
            }
        }
    }
}

fn send(keys: &[(VIRTUAL_KEY, bool)]) -> bool {
    let inputs: Vec<INPUT> = keys
        .iter()
        .map(|&(key, up)| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key,
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS::default()
                    },
                    dwExtraInfo: INJECTED,
                    ..Default::default()
                },
            },
        })
        .collect();
    // SAFETY: 每个 INPUT 都是键盘变体,结构尺寸正确。
    unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) as usize == inputs.len() }
}

unsafe extern "system" fn worker_wnd_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    _lparam: LPARAM,
) -> LRESULT {
    // SAFETY: 全部在本 worker 线程的消息泵上运行;不跨回调持有引用。
    unsafe {
        match message {
            WM_TIMER if wparam.0 == HOLD_TIMER || wparam.0 == NUM_HOLD_TIMER => {
                STATE.with_borrow_mut(|state| {
                    let state = state.as_mut().unwrap();
                    let config = sync_config(state);
                    let now = GetTickCount();
                    let is_trigger = wparam.0 == HOLD_TIMER;
                    let slot = if is_trigger {
                        &mut state.press
                    } else {
                        &mut state.num_press
                    };
                    if let Some(press) = slot {
                        // 定时器送达时前台/修饰键可能已变:复核取消条件。
                        if is_trigger
                            && (!config.enabled
                                || GetForegroundWindow() != state.foreground
                                || modifiers_down()
                                || !chinese_input(state.foreground))
                        {
                            press.cancel();
                        }
                        let action = press.hold(now);
                        let finished = press.finished();
                        if finished {
                            let _ = KillTimer(
                                Some(window),
                                if is_trigger {
                                    HOLD_TIMER
                                } else {
                                    NUM_HOLD_TIMER
                                },
                            );
                        }
                        if let Some(action) = action {
                            let vk = if is_trigger {
                                trigger_vk(config.remap_key)
                            } else {
                                VK_NUMLOCK
                            };
                            execute(&config, vk, action);
                        }
                    }
                });
            }
            WM_CLOSE => PostQuitMessage(0),
            _ => return DefWindowProcW(window, message, wparam, _lparam),
        }
        LRESULT(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 轻点/长按契约(移植 WinCaps 的 tap_and_hold_contract):
    /// 关态轻点 = Tap;到阈值 = SetToggle(true);开态轻点/长按 = SetToggle(false)。
    #[test]
    fn tap_and_hold_contract() {
        for threshold in [250u32, 350, 500] {
            assert_eq!(
                Press::new(0, false, threshold, TapMeaning::Inject).release(threshold - 1),
                Some(GestureAction::Tap)
            );
            assert_eq!(
                Press::new(0, false, threshold, TapMeaning::Inject).release(threshold),
                Some(GestureAction::SetToggle(true))
            );
        }
        assert_eq!(
            Press::new(0, true, 350, TapMeaning::Inject).release(349),
            Some(GestureAction::SetToggle(false))
        );
        assert_eq!(
            Press::new(0, true, 350, TapMeaning::Inject).release(350),
            Some(GestureAction::SetToggle(false))
        );
        for on in [false, true] {
            let mut press = Press::new(0, on, 350, TapMeaning::Inject);
            assert_eq!(press.hold(349), None);
            assert_eq!(press.hold(350), Some(GestureAction::SetToggle(!on)));
            assert_eq!(press.hold(700), None);
            assert_eq!(press.release(701), None);
        }
    }

    /// NumLock 防误触:轻点一律吞掉(与当前态无关),长按才翻转。
    #[test]
    fn numlock_hold_swallows_tap() {
        for on in [false, true] {
            assert_eq!(
                Press::new(0, on, 350, TapMeaning::Swallow).release(349),
                None
            );
            assert_eq!(
                Press::new(0, on, 350, TapMeaning::Swallow).release(350),
                Some(GestureAction::SetToggle(!on))
            );
        }
    }

    /// 取消与时钟环绕(移植 WinCaps cancellation_and_clock_wrap)。
    #[test]
    fn cancellation_and_clock_wrap() {
        let mut press = Press::new(0, false, 350, TapMeaning::Inject);
        press.cancel();
        assert_eq!(press.hold(350), None);
        assert_eq!(press.release(351), None);
        assert_eq!(
            Press::new(u32::MAX - 100, false, 350, TapMeaning::Inject).release(249),
            Some(GestureAction::SetToggle(true))
        );
    }

    /// 中文布局判定:全部中文子语言共享 PRIMARYLANGID 0x04。
    #[test]
    fn chinese_input_languages() {
        for layout in [0x0804usize, 0x0404, 0x0c04, 0x1004, 0x1404, 0xe0200804] {
            assert!(is_chinese_layout(HKL(layout as _)));
        }
        for layout in [0usize, 0x0409, 0x0809, 0x0419, 0x0411, 0x0412] {
            assert!(!is_chinese_layout(HKL(layout as _)));
        }
    }
}
