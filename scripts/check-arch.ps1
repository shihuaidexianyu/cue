# 架构护栏(§70–73、§110–111、§153):Cargo 依赖图 + 源码平台纯净度。
# §141:依赖方向改用 Cargo 图,源码继续扫描;任何违规退出码非零。
# §153:平台纯净从 windows 系扩为双平台绑定族(Windows + macOS/ObjC),
#   守卫范围扩到 util-common(共享 util 平台中立,§72–73 首次机器化);
#   ui 不受约束——GPUI 后端允许合法持有 cfg(target_os) 渲染差异代码。
# 用法:Windows 上 powershell -File scripts/check-arch.ps1;
#       macOS/Linux 上 pwsh scripts/check-arch.ps1(源码扫描经 rg 或
#       git grep 回退,两者跨平台)。CI 在 windows 与 macos runner 各跑一次。
$ErrorActionPreference = "Stop"
$script:fail = 0
function Bad([string]$msg) { Write-Host "FAIL: $msg" -ForegroundColor Red; $script:fail = 1 }

# 源码扫描:优先 ripgrep,没有则退回 git grep(§142——上一版硬依赖 rg,
# 没装 ripgrep 的机器会直接 throw)。两者都是"无命中返回 1、出错返回 >1"。
$script:hasRg = $null -ne (Get-Command rg -ErrorAction SilentlyContinue)
function Scan([string]$pattern, [string[]]$paths) {
    if ($script:hasRg) {
        $out = & rg -n $pattern @paths
    } else {
        $out = & git grep -n -E $pattern -- @paths
    }
    if ($LASTEXITCODE -gt 1) { throw "source scan failed: $pattern" }
    if ($LASTEXITCODE -eq 0) { return $out }
    return @()
}

# --- 1) 平台纯净度(§110–111、§153):core / protocol / util-common 不得有平台代码 ---
# 双平台绑定族:Windows 侧沿用,新增 macOS/ObjC 侧(§153)。git grep -E
# 回退是 POSIX ERE,模式只用字面交替,不用 \b。raw extern FFI 不查——
# 与 windows 侧同水位:护栏是 tripwire,不是证明。
$hits = Scan 'std::os::windows|std::os::unix|windows::Win32|use windows|windows_sys|objc2|cocoa|core_foundation|core_graphics|core_text|use objc|objc::' @('crates/sakana-core/src', 'crates/sakana-protocol/src', 'crates/sakana-util-common/src')
if ($hits) { $hits | ForEach-Object { Write-Host "  $_" }; Bad "sakana-core/sakana-protocol/sakana-util-common 出现平台代码(§110/§153)" }
# Cargo.toml 锚定行首 crate 名 + [.\s=-](workspace 等普通词不带这些
# 前缀,不吃子串误伤);objc2 家族 crate(objc2-app-kit 等)由 objc2 前缀覆盖。
foreach ($toml in "crates/sakana-core/Cargo.toml", "crates/sakana-protocol/Cargo.toml", "crates/sakana-util-common/Cargo.toml") {
    if (Select-String -Path $toml -Pattern '^\[dependencies\.(windows|windows-sys|objc|objc2|cocoa|core-foundation|core-graphics|core-text|metal)\]|^\s*(windows|windows-sys|objc|objc2|cocoa|core-foundation|core-graphics|core-text|metal)[.\s=-]' -Quiet) {
        Bad "$toml 依赖平台绑定 crate(§110/§153)"
    }
}

# --- 2) Cargo 解析后的依赖图:覆盖完整模块名、workspace、别名与表格写法 ---
function Allowed([string]$owner, [string]$dependency) {
    if ($owner -eq 'sakana') { return $true }
    # 平台绑定族(§153):前缀匹配覆盖家族成员(objc2-app-kit、
    # core-foundation-sys 等),不逐一枚举。
    if ($owner -in @('sakana-core', 'sakana-protocol', 'sakana-util-common') -and (
            $dependency -like 'windows*' -or $dependency -like 'objc*' -or $dependency -like 'cocoa*' -or
            $dependency -like 'core-foundation*' -or $dependency -like 'core-graphics*' -or
            $dependency -like 'core-text*' -or $dependency -eq 'metal')) { return $false }
    if ($dependency -notlike 'sakana-*') { return $true }
    switch -Wildcard ($owner) {
        'sakana-protocol' { return $false }
        'sakana-core' { return $dependency -eq 'sakana-protocol' }
        'sakana-ui' { return $dependency -in @('sakana-core', 'sakana-protocol') }
        'sakana-windows' { return $dependency -eq 'sakana-protocol' }
        'sakana-util-common' { return $dependency -eq 'sakana-protocol' }
        'sakana-util-win' { return $dependency -eq 'sakana-protocol' }
        'sakana-module-*' { return $dependency -in @('sakana-protocol', 'sakana-util-common', 'sakana-util-win') }
        default { return $false }
    }
}
# 回归:原正则 module- 分支匹配不到 sakana-module-app。
foreach ($owner in @('sakana-core', 'sakana-ui', 'sakana-util-win', 'sakana-module-file', 'sakana-module-web')) {
    if (Allowed $owner 'sakana-module-app') { throw "guard regression: $owner -> module-app" }
}
if (!(Allowed 'sakana-module-file' 'sakana-util-win')) { throw 'guard regression: util dependency' }
if (!(Allowed 'sakana-util-common' 'sakana-protocol')) { throw 'guard regression: util-common protocol' }
if (Allowed 'sakana-util-common' 'sakana-core') { throw 'guard regression: util-common direction' }
if (!(Allowed 'sakana-module-app' 'sakana-util-common')) { throw 'guard regression: module -> util-common' }
if (Allowed 'sakana-core' 'windows') { throw "guard regression: platform dependency" }
if (Allowed 'sakana-core' 'objc2') { throw "guard regression: platform dependency (macOS)" }
if (Allowed 'sakana-util-common' 'core-foundation') { throw "guard regression: util-common platform" }
if (Allowed 'sakana-protocol' 'cocoa') { throw "guard regression: protocol platform (macOS)" }
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
$hits = Scan 'use sakana_(windows|ui|module|util_win)|sakana_windows::|sakana_ui::|sakana_module_' @('crates/sakana-core/src')
if ($hits) { $hits | ForEach-Object { Write-Host "  $_" }; Bad "sakana-core 引用具体 crate(§70)" }

if ($script:fail -ne 0) { exit 1 }
Write-Host "arch check OK:平台纯净度 + 依赖方向 + composition root" -ForegroundColor Green
exit 0
