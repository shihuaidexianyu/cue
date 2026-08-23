//! ShellExecuteExW 打开/启动(§29):文件走系统关联程序,文件夹进
//! 资源管理器,URL 路由到默认浏览器;lpVerb = None(默认动词)。

use cue_protocol::ModuleError;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// 打开 `file`(exe / 文档 / 文件夹 / URL),可带参数与工作目录。
pub fn shell_execute(
    file: &str,
    params: Option<&str>,
    working_dir: Option<&Path>,
) -> Result<(), ModuleError> {
    let file_w = to_wide(file);
    let params_w = params.map(to_wide);
    let dir_w = working_dir.map(|d| to_wide(&d.to_string_lossy()));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        lpFile: PCWSTR(file_w.as_ptr()),
        lpParameters: params_w
            .as_ref()
            .map(|p| PCWSTR(p.as_ptr()))
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
            .map_err(|e| ModuleError::ActivationFailed(format!("{file}: {e}")))
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
