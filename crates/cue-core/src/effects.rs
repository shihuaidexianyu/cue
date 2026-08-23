/// Core 输出的平台无关效果,由 launcher 编排层执行。
/// Core 不知道 GPUI / HWND / RegisterHotKey 的存在。
///
/// V1 只有这三个。同步例外——注入的 host 回调(apply_hotkey /
/// apply_start_on_boot / open_path / fullscreen_probe)不走这里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreEffect {
    ShowLauncher,
    HideLauncher,
    FocusInput,
}
