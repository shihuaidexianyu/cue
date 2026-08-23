//! 启动:Win32 = ShellExecuteEx(cue-util-win);Packaged =
//! IApplicationActivationManager::ActivateApplication(AUMID)。
//! 绝不走 AppsFolder。

use crate::catalog::LaunchTarget;
use cue_protocol::ModuleError;
use cue_util_win::com::ComGuard;
use windows::core::PCWSTR;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::UI::Shell::{
    ApplicationActivationManager, IApplicationActivationManager, ACTIVATEOPTIONS,
};

pub fn launch(target: &LaunchTarget) -> Result<(), ModuleError> {
    match target {
        LaunchTarget::Win32 {
            exe,
            args,
            working_dir,
        } => cue_util_win::shell::shell_execute(
            &exe.to_string_lossy(),
            (!args.is_empty()).then_some(&**args),
            working_dir.as_deref(),
        ),
        LaunchTarget::Packaged { aumid } => launch_packaged(aumid),
    }
}

/// 以管理员身份运行:仅 Win32 目标——packaged 应用由系统代理激活,
/// 没有可提权的 exe,actions() 也不为它们声明此动作。
pub fn launch_elevated(target: &LaunchTarget) -> Result<(), ModuleError> {
    match target {
        LaunchTarget::Win32 {
            exe,
            args,
            working_dir,
        } => cue_util_win::shell::shell_execute_elevated(
            &exe.to_string_lossy(),
            (!args.is_empty()).then_some(&**args),
            working_dir.as_deref(),
        ),
        LaunchTarget::Packaged { aumid } => Err(ModuleError::ActivationFailed(format!(
            "packaged app 不支持以管理员身份运行:{aumid}"
        ))),
    }
}

/// 打开所在位置:仅 Win32 目标(packaged 同上)。
pub fn reveal_location(target: &LaunchTarget) -> Result<(), ModuleError> {
    match target {
        LaunchTarget::Win32 { exe, .. } => {
            cue_util_win::shell::reveal_in_explorer(&exe.to_string_lossy())
        }
        LaunchTarget::Packaged { aumid } => Err(ModuleError::ActivationFailed(format!(
            "packaged app 不支持打开所在位置:{aumid}"
        ))),
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn launch_packaged(aumid: &str) -> Result<(), ModuleError> {
    let _com = ComGuard::new();
    let aumid_w = to_wide(aumid);
    unsafe {
        let mgr: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_ALL)
                .map_err(|e| ModuleError::ActivationFailed(format!("activation manager: {e}")))?;
        mgr.ActivateApplication(PCWSTR(aumid_w.as_ptr()), PCWSTR::null(), ACTIVATEOPTIONS(0))
            .map_err(|e| ModuleError::ActivationFailed(format!("{aumid}: {e}")))?;
    }
    Ok(())
}
