# CUE 打包:release 构建 → dist  staging → zip。
# 用法: powershell -ExecutionPolicy Bypass -File scripts\package.ps1 [-Sign]
#   -Sign  先用 scripts\sign.ps1 给 exe 签名(自签名 dev 证书见 sign.ps1)
param(
    [switch]$Sign
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# 版本号取 crates/cue/Cargo.toml
$ver = (Select-String -Path "crates\cue\Cargo.toml" -Pattern '^version = "([^"]+)"').Matches.Groups[1].Value
if (-not $ver) { throw "cannot read version from crates/cue/Cargo.toml" }

# 运行中的 cue.exe 会锁死二进制。
Get-Process cue -ErrorAction SilentlyContinue | Stop-Process -Force -Confirm:$false

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed" }

if ($Sign) {
    & "$PSScriptRoot\sign.ps1" -ExePath "$root\target\release\cue.exe" -SelfSignedDev
    if ($LASTEXITCODE -ne 0) { throw "sign failed" }
}

$stage = "$root\dist\CUE"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item "$root\target\release\cue.exe" "$stage\cue.exe"
foreach ($f in @("install.ps1", "uninstall.ps1", "install.cmd", "uninstall.cmd", "README.txt")) {
    Copy-Item "$PSScriptRoot\$f" "$stage\$f"
}

$zip = "$root\dist\CUE-$ver-win-x64.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path "$stage\*" -DestinationPath $zip -Force
Write-Host ""
Write-Host "packed: $zip"
Write-Host "  cue.exe: $([Math]::Round((Get-Item "$stage\cue.exe").Length / 1MB, 1)) MB"

# Inno Setup 向导安装包:检测到 ISCC 就出,没有就跳过(zip 不受影响)。
$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
          "C:\Program Files (x86)\Inno Setup 6\ISCC.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if ($iscc) {
    & $iscc "/DAppVersion=$ver" "$PSScriptRoot\setup.iss"
    if ($LASTEXITCODE -ne 0) { throw "ISCC compile failed" }
    Write-Host "  setup: $root\dist\CUE-Setup-$ver.exe"
} else {
    Write-Host "  Inno Setup (ISCC.exe) 未安装,跳过 setup.exe(仅出 zip)"
}
