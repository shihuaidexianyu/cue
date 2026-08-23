//! ShellExecuteExW 打开/启动:文件走系统关联程序,文件夹进
//! 资源管理器,URL 路由到默认浏览器;lpVerb = None(默认动词)。
//! 次级动作:runas 提权、explorer /select 定位。

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
    shell_execute_verb(file, params, working_dir, None)
}

/// 以管理员身份运行("Run as administrator"):lpVerb = "runas"
/// 触发 UAC;用户取消(ERROR_CANCELLED)按普通失败上报——session
/// 保持打开、错误横幅展示,不算异常。
pub fn shell_execute_elevated(
    file: &str,
    params: Option<&str>,
    working_dir: Option<&Path>,
) -> Result<(), ModuleError> {
    shell_execute_verb(file, params, working_dir, Some("runas"))
}

/// 在资源管理器中定位并选中("Open containing folder"):
/// `explorer.exe /select,"<path>"`,文件与文件夹通用。Windows 路径
/// 本身不含 `"`,内层引号不会断裂;路径不存在时 explorer 自行报错,
/// ShellExecute 层面仍算成功。
pub fn reveal_in_explorer(path: &str) -> Result<(), ModuleError> {
    shell_execute("explorer.exe", Some(&format!("/select,\"{path}\"")), None)
}

fn shell_execute_verb(
    file: &str,
    params: Option<&str>,
    working_dir: Option<&Path>,
    verb: Option<&str>,
) -> Result<(), ModuleError> {
    let file_w = to_wide(file);
    let params_w = params.map(to_wide);
    let dir_w = working_dir.map(|d| to_wide(&d.to_string_lossy()));
    let verb_w = verb.map(to_wide);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        lpVerb: verb_w
            .as_ref()
            .map(|v| PCWSTR(v.as_ptr()))
            .unwrap_or(PCWSTR::null()),
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
