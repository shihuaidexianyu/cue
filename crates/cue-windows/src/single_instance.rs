//! 单实例:named mutex 检测 + 薄 IPC(PostMessage 到第一实例的
//! host window)请求 show/focus + 退出。
//!
//! 锁定一个全局假设:settings 与 usage store 永远单写者。

use crate::host::{HOST_WINDOW_CLASS, WM_CUE_SHOW};
use windows::core::w;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, LPARAM, WPARAM,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, PostMessageW,
};

const MUTEX_NAME: windows::core::PCWSTR = w!("Local\\CUE.SingleInstance");

pub enum AcquireOutcome {
    /// 第一实例。guard 持有 mutex,进程退出时释放。
    Primary(SingleInstanceGuard),
    /// 已有实例在跑:已通知它 show / focus,本进程应立即退出。
    AlreadyRunning,
}

pub struct SingleInstanceGuard(HANDLE);

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// 必须在最早时机调用(在任何状态文件被打开之前)。
pub fn acquire() -> AcquireOutcome {
    unsafe {
        let handle = match CreateMutexW(None, true, MUTEX_NAME) {
            Ok(h) => h,
            // 失败关闭:无法判定唯一性时拒绝运行,避免 settings/usage 双写。
            Err(_) => return AcquireOutcome::AlreadyRunning,
        };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            signal_first_instance();
            let _ = CloseHandle(handle);
            return AcquireOutcome::AlreadyRunning;
        }
        AcquireOutcome::Primary(SingleInstanceGuard(handle))
    }
}

fn signal_first_instance() {
    unsafe {
        if let Ok(host) = FindWindowW(HOST_WINDOW_CLASS, windows::core::PCWSTR::null()) {
            if !host.0.is_null() {
                // 第二实例通常由用户输入(Start Menu / 快捷键)启动,
                // 此刻拥有前台权限;先把权限移交给第一实例,
                // 否则第一实例的 SetForegroundWindow 会被系统拒绝,
                // 窗口 show 出来立刻被失焦隐藏。
                let mut pid = 0u32;
                GetWindowThreadProcessId(host, Some(&mut pid));
                if pid != 0 {
                    let _ = AllowSetForegroundWindow(pid);
                }
                let _ = PostMessageW(Some(host), WM_CUE_SHOW, WPARAM(0), LPARAM(0));
            }
        }
    }
}
