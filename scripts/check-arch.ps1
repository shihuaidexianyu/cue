# 架构护栏(§70–73、§110–111):依赖方向 + 平台纯净度,纯 grep。
# 规格声称"grep-checkable"——本脚本就是兑现;任何一条违规退出码非零。
# 用法:powershell -File scripts/check-arch.ps1(建议 pre-push 跑一次;仓库尚无 CI)
$ErrorActionPreference = "Stop"
$script:fail = 0
function Bad([string]$msg) { Write-Host "FAIL: $msg" -ForegroundColor Red; $script:fail = 1 }

# --- 1) 平台纯净度(§110–111):sakana-core / sakana-protocol 不得有平台代码 ---
$hits = git grep -n -E "std::os::windows|windows::Win32|use windows|windows_sys" `
    -- crates/sakana-core/src crates/sakana-protocol/src
if ($LASTEXITCODE -eq 0) { $hits | ForEach-Object { Write-Host "  $_" }; Bad "sakana-core/sakana-protocol 出现平台代码(§110)" }
foreach ($toml in "crates/sakana-core/Cargo.toml", "crates/sakana-protocol/Cargo.toml") {
    if (Select-String -Path $toml -Pattern "^\[dependencies\.windows\]|^windows(-sys)?[.\s=]" -Quiet) {
        Bad "$toml 依赖 windows crate(§110)"
    }
}

# --- 2) 依赖方向(§71,按 manifest 行首 dep key) ---
# 规则表:manifest → 禁止出现的 sakana-* 依赖(允许的:sakana → 一切;
# sakana-core → protocol;protocol → 无;sakana-ui → core+protocol;
# sakana-module-* → protocol(+util-win);sakana-util-win → protocol)
$rules = @(
    @{ File = "crates/sakana-core/Cargo.toml";     Deny = "^sakana-(ui|windows|module-|util-win)[.\s=]" }
    @{ File = "crates/sakana-protocol/Cargo.toml"; Deny = "^sakana-[a-z]" }
    @{ File = "crates/sakana-ui/Cargo.toml";       Deny = "^sakana-(windows|module-|util-win)[.\s=]" }
    @{ File = "crates/sakana-util-win/Cargo.toml"; Deny = "^sakana-(core|ui|windows|module-)[.\s=]" }
)
$rules += Get-ChildItem "crates/sakana-module-*/Cargo.toml" | ForEach-Object {
    @{ File = $_.FullName; Deny = "^sakana-(core|ui|windows)[.\s=]" }
}
foreach ($r in $rules) {
    $m = Select-String -Path $r.File -Pattern $r.Deny
    if ($m) { $m | ForEach-Object { Write-Host "  $($_.Filename): $($_.Line.Trim())" }; Bad "$($r.File) 依赖方向违规(§71)" }
}

# --- 3) Composition Root(§70–71):sakana-core 源码不得点名任何具体宿主/模块 crate ---
$hits = git grep -n -E "use sakana_(windows|ui|module|util_win)|sakana_windows::|sakana_ui::|sakana_module_" `
    -- crates/sakana-core/src
if ($LASTEXITCODE -eq 0) { $hits | ForEach-Object { Write-Host "  $_" }; Bad "sakana-core 引用具体 crate(§70)" }

if ($script:fail -ne 0) { exit 1 }
Write-Host "arch check OK:平台纯净度 + 依赖方向 + composition root" -ForegroundColor Green
exit 0
