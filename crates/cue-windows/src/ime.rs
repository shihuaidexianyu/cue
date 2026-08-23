//! 输入法:**首选候选实现,Phase 1 spike 验证对象**。
//!
//! 产品决定已固定(Launcher 输入框强制英文输入),本模块是候选实现:
//! 1. `ActivateKeyboardLayout` 把 UI 线程切到英文布局;
//! 2. `ImmAssociateContext(hwnd, NULL)` 使 IME 不附加到搜索框。
//!
//! **一次唤醒必须成对**:show 之前记录用户(前台线程)的
//! 布局,hide 之前恢复——否则在系统默认的全局输入法模式下,
//! 用户的其他应用会被留在英文。
//!
//! spike 要验证的风险:GPUI 的 Windows 后端自己维护 IME 状态,从外部
//! 操作同一 HWND 的 IMM 上下文可能与 GPUI 内部状态不同步。若证实冲突,
//! 候选替代是在 GPUI 层禁用该窗口的 text input 处理。

use std::sync::atomic::{AtomicIsize, Ordering};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::Ime::ImmAssociateContext;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ACTIVATE_KEYBOARD_LAYOUT_FLAGS, ActivateKeyboardLayout, GetKeyboardLayout, HKL, KLF_ACTIVATE,
    LoadKeyboardLayoutW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::core::{Error, w};

/// 唤醒时记录的用户原布局(前台线程 HKL),hide 时恢复。
static SAVED_LAYOUT: AtomicIsize = AtomicIsize::new(0);

/// ShowLauncher 流程,**必须在窗口抢到前台之前**调用:
/// 1. 记录当前前台线程的键盘布局(用户正在用的输入法);
/// 2. 本线程切到 en-US 并使 IME 不附加到搜索框。
///
/// 必须在 UI 线程调用(键盘布局 per-thread)。
pub fn enter_english_mode(hwnd: HWND) -> Result<(), Error> {
    unsafe {
        // 此刻前台仍是用户之前的应用;等 show_and_focus 之后
        // 前台就是我们自己,再记录就只剩 en-US 了。
        let fg = GetForegroundWindow();
        if !fg.0.is_null() {
            let tid = GetWindowThreadProcessId(fg, None);
            if tid != 0 {
                let prev = GetKeyboardLayout(tid);
                SAVED_LAYOUT.store(prev.0 as isize, Ordering::SeqCst);
            }
        }
        // en-US (00000409)。目标 HKL 需已加载,故先 LoadKeyboardLayout。
        let hkl = LoadKeyboardLayoutW(w!("00000409"), KLF_ACTIVATE)?;
        let _ = ActivateKeyboardLayout(hkl, ACTIVATE_KEYBOARD_LAYOUT_FLAGS(0))?;
        let _ = ImmAssociateContext(hwnd, Default::default());
    }
    Ok(())
}

/// HideLauncher 流程,**在窗口仍处前台时**调用才有效:
/// 全局输入法模式下,布局切换沿前台线程传播,藏窗之后再恢复就晚了。
/// 失焦隐藏路径调用时窗口已不在前台,恢复不保证生效(已知边界)。
pub fn restore_saved_layout() {
    let saved = SAVED_LAYOUT.swap(0, Ordering::SeqCst);
    if saved != 0 {
        unsafe {
            let _ = ActivateKeyboardLayout(
                HKL(saved as *mut core::ffi::c_void),
                ACTIVATE_KEYBOARD_LAYOUT_FLAGS(0),
            );
        }
    }
}
