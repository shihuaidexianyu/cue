# sakana 打包:release 构建 → Inno Setup 编译 dist\sakana-setup-<ver>.exe。
# 用法: powershell -ExecutionPolicy Bypass -File scripts\package.ps1 [-Sign]
#   -Sign  用 scripts\sign.ps1 给 sakana.exe 与 setup.exe 签名(自签名 dev 证书见 sign.ps1)
param(
    [switch]$Sign
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# 版本号取 cargo metadata(binary crate `sakana`):版本已统一为
# [workspace.package] 的 version.workspace 继承,crate 的 Cargo.toml
# 里不再有字面 version 行,旧正则会失配。
$ver = (cargo metadata --format-version 1 --no-deps |
    ConvertFrom-Json).packages |
    Where-Object { $_.name -eq 'sakana' } |
    Select-Object -ExpandProperty version -First 1
if (-not $ver) { throw "cannot read version from cargo metadata" }

# 运行中的 sakana.exe 会锁死二进制。
Get-Process sakana -ErrorAction SilentlyContinue | Stop-Process -Force -Confirm:$false

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed" }

if ($Sign) {
    & "$PSScriptRoot\sign.ps1" -ExePath "$root\target\release\sakana.exe" -SelfSignedDev
    if ($LASTEXITCODE -ne 0) { throw "sign failed" }
}

# 唯一分发形态:Inno Setup 向导安装包(没有 zip/绿色包)。
$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
          "C:\Program Files (x86)\Inno Setup 6\ISCC.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 (ISCC.exe) 未安装——setup.exe 是唯一产物,先装 Inno" }
& $iscc "/DAppVersion=$ver" "$PSScriptRoot\setup.iss"
if ($LASTEXITCODE -ne 0) { throw "ISCC compile failed" }
$setup = "$root\dist\sakana-setup-$ver.exe"

if ($Sign) {
    & "$PSScriptRoot\sign.ps1" -ExePath $setup -SelfSignedDev
    if ($LASTEXITCODE -ne 0) { throw "sign setup failed" }
}

Write-Host ""
Write-Host "packed: $setup"
Write-Host "  size: $([Math]::Round((Get-Item $setup).Length / 1MB, 1)) MB"
