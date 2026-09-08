# 架构护栏(§70–73、§110–111):Cargo 依赖图 + 源码平台纯净度。
# §141:依赖方向改用 Cargo 图,源码继续扫描;任何违规退出码非零。
# 用法:powershell -File scripts/check-arch.ps1(建议 pre-push 跑一次;仓库尚无 CI)
$ErrorActionPreference = "Stop"
$script:fail = 0
function Bad([string]$msg) { Write-Host "FAIL: $msg" -ForegroundColor Red; $script:fail = 1 }

# --- 1) 平台纯净度(§110–111):sakana-core / sakana-protocol 不得有平台代码 ---
$hits = rg -n 'std::os::windows|windows::Win32|use windows|windows_sys' crates/sakana-core/src crates/sakana-protocol/src
if ($LASTEXITCODE -gt 1) { throw 'platform source scan failed' }
if ($LASTEXITCODE -eq 0) { $hits | ForEach-Object { Write-Host "  $_" }; Bad "sakana-core/sakana-protocol 出现平台代码(§110)" }
foreach ($toml in "crates/sakana-core/Cargo.toml", "crates/sakana-protocol/Cargo.toml") {
    if (Select-String -Path $toml -Pattern "^\[dependencies\.windows\]|^windows(-sys)?[.\s=]" -Quiet) {
        Bad "$toml 依赖 windows crate(§110)"
    }
}

# --- 2) Cargo 解析后的依赖图:覆盖完整模块名、workspace、别名与表格写法 ---
function Allowed([string]$owner, [string]$dependency) {
    if ($owner -eq 'sakana') { return $true }
    if ($owner -in @('sakana-core', 'sakana-protocol') -and $dependency -in @('windows', 'windows-sys')) { return $false }
    if ($dependency -notlike 'sakana-*') { return $true }
    switch -Wildcard ($owner) {
        'sakana-protocol' { return $false }
        'sakana-core' { return $dependency -eq 'sakana-protocol' }
        'sakana-ui' { return $dependency -in @('sakana-core', 'sakana-protocol') }
        'sakana-windows' { return $dependency -eq 'sakana-protocol' }
        'sakana-util-win' { return $dependency -eq 'sakana-protocol' }
        'sakana-module-*' { return $dependency -in @('sakana-protocol', 'sakana-util-win') }
        default { return $false }
    }
}
# 回归:原正则 module- 分支匹配不到 sakana-module-app。
foreach ($owner in @('sakana-core', 'sakana-ui', 'sakana-util-win', 'sakana-module-file')) {
    if (Allowed $owner 'sakana-module-app') { throw "guard regression: $owner -> module-app" }
}
if (!(Allowed 'sakana-module-file' 'sakana-util-win')) { throw 'guard regression: util dependency' }
if (Allowed 'sakana-core' 'windows') { throw 'guard regression: platform dependency' }
$metadataText = cargo metadata --format-version 1 --no-deps --locked
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
$metadata = ($metadataText -join "`n") | ConvertFrom-Json
foreach ($package in $metadata.packages) {
    if ($package.id -notin $metadata.workspace_members) { continue }
    foreach ($dependency in $package.dependencies) {
        if (!(Allowed $package.name $dependency.name)) { Bad "$($package.name) -> $($dependency.name): dependency direction" }
    }
}

# --- 3) Composition Root(§70–71):sakana-core 源码不得点名任何具体宿主/模块 crate ---
$hits = rg -n 'use sakana_(windows|ui|module|util_win)|sakana_windows::|sakana_ui::|sakana_module_' crates/sakana-core/src
if ($LASTEXITCODE -gt 1) { throw 'composition source scan failed' }
if ($LASTEXITCODE -eq 0) { $hits | ForEach-Object { Write-Host "  $_" }; Bad "sakana-core 引用具体 crate(§70)" }

if ($script:fail -ne 0) { exit 1 }
Write-Host "arch check OK:平台纯净度 + 依赖方向 + composition root" -ForegroundColor Green
exit 0
