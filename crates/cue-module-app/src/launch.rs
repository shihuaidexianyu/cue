//! 启动(§29):Win32 = ShellExecuteEx;Packaged =
//! IApplicationActivationManager::ActivateApplication(AUMID)。
//! 绝不走 AppsFolder。

use crate::catalog::LaunchTarget;
use crate::com::ComGuard;
use cue_protocol::ModuleError;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::UI::Shell::{
    ApplicationActivationManager, IApplicationActivationManager, ShellExecuteExW,
    ACTIVATEOPTIONS, SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub fn launch(target: &LaunchTarget) -> Result<(), ModuleError> {
    match target {
        LaunchTarget::Win32 {
            exe,
            args,
            working_dir,
        } => launch_win32(exe, args, working_dir.as_deref()),
        LaunchTarget::Packaged { aumid } => launch_packaged(aumid),
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn launch_win32(
    exe: &Path,
    args: &str,
    working_dir: Option<&Path>,
) -> Result<(), ModuleError> {
    let exe_w = to_wide(&exe.to_string_lossy());
    let args_w = (!args.is_empty()).then(|| to_wide(args));
    let dir_w = working_dir.map(|d| to_wide(&d.to_string_lossy()));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        lpFile: PCWSTR(exe_w.as_ptr()),
        lpParameters: args_w
            .as_ref()
            .map(|a| PCWSTR(a.as_ptr()))
            .unwrap_or(PCWSTR::null()),
        lpDirectory: dir_w
            .as_ref()
            .map(|d| PCWSTR(d.as_ptr()))
            .unwrap_or(PCWSTR::null()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info)
            .map_err(|e| ModuleError::ActivationFailed(format!("{}: {e}", exe.display())))
    }
}

fn launch_packaged(aumid: &str) -> Result<(), ModuleError> {
    let _com = ComGuard::new();
    let aumid_w = to_wide(aumid);
    unsafe {
        let mgr: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_ALL)
                .map_err(|e| ModuleError::ActivationFailed(format!("activation manager: {e}")))?;
        mgr.ActivateApplication(
            PCWSTR(aumid_w.as_ptr()),
            PCWSTR::null(),
            ACTIVATEOPTIONS(0),
        )
        .map_err(|e| ModuleError::ActivationFailed(format!("{aumid}: {e}")))?;
    }
    Ok(())
}
