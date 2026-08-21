/// §112 Core 输出的平台无关效果,由 launcher 编排层执行。
/// Core 不知道 GPUI / HWND / RegisterHotKey 的存在。
///
/// V1 只有这三个。唯一的同步例外是热键 try-apply(§53),不走这里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreEffect {
    ShowLauncher,
    HideLauncher,
    FocusInput,
}
