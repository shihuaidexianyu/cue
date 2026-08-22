# CUE 代码签名。
# 用法:
#   签名(dev 自签名):  powershell -ExecutionPolicy Bypass -File scripts\sign.ps1 -SelfSignedDev
#   签名(已有证书):   ... sign.ps1 -Thumbprint <sha1>        (CurrentUser\My 或 LocalMachine\My)
#                    ... sign.ps1 -PfxPath cert.pfx -PfxPassword <pwd>
#
# 注意:自签名证书只证明"文件自签名后未被篡改",不建立发布者信任——
# SmartScreen 信誉需要 OV/EV 代码签名证书(公开分发再购买)。
# 本脚本不会把任何证书导入受信任根存储(不改机器信任链)。
param(
    [string]$ExePath = "target\release\cue.exe",
    [string]$Thumbprint,
    [string]$PfxPath,
    [string]$PfxPassword,
    [switch]$SelfSignedDev
)
$ErrorActionPreference = "Stop"

# signtool:取 Windows Kits 下最新版本。
$kits = "C:\Program Files (x86)\Windows Kits\10\bin"
$signtool = Get-ChildItem $kits -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending |
    ForEach-Object { Join-Path $_.FullName "x64\signtool.exe" } |
    Where-Object { Test-Path $_ } |
    Select-Object -First 1
if (-not $signtool) { throw "signtool.exe not found (install Windows SDK)" }

$ExePath = Resolve-Path $ExePath

if ($SelfSignedDev) {
    $subject = "CN=CUE Dev (self-signed)"
    $cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert |
        Where-Object { $_.Subject -eq $subject } |
        Select-Object -First 1
    if (-not $cert) {
        $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject `
            -CertStoreLocation Cert:\CurrentUser\My -NotAfter (Get-Date).AddYears(3)
        Write-Host "created dev cert: $subject ($($cert.Thumbprint))"
    }
    $Thumbprint = $cert.Thumbprint
}

$args = @("sign", "/fd", "SHA256")
if ($PfxPath) {
    $args += @("/f", $PfxPath)
    if ($PfxPassword) { $args += @("/p", $PfxPassword) }
} elseif ($Thumbprint) {
    $args += @("/sha1", $Thumbprint)
} else {
    throw "specify -SelfSignedDev, -Thumbprint, or -PfxPath"
}
# 时间戳:先尝试(DigiCert 公共服务器),失败则降级为无时间戳并告警。
$signed = $false
foreach ($ts in @($true, $false)) {
    $a = $args
    if ($ts) { $a += @("/tr", "http://timestamp.digicert.com", "/td", "SHA256") }
    & $signtool @a $ExePath | Out-Null
    if ($LASTEXITCODE -eq 0) { $signed = $true; break }
    if ($ts) { Write-Host "timestamping failed, retrying without timestamp..." }
}
if (-not $signed) { throw "signtool sign failed" }

if ($SelfSignedDev) {
    # 自签名证书不(也不应)在受信任根里,/pa 验证必失败——
    # 这里只校验签名本身存在且摘要匹配;信任链检查留给正式证书。
    $sig = Get-AuthenticodeSignature $ExePath
    if ($sig.Status -eq "NotSigned" -or $sig.Status -eq "HashMismatch" -or
        $sig.Status -eq "Incompatible" -or $sig.Status -eq "NotSupportedFileFormat") {
        throw "signature check failed: $($sig.Status)"
    }
    Write-Host "signed: $ExePath (self-signed, status=$($sig.Status) — 信任链警告属预期)"
} else {
    & $signtool verify /pa $ExePath
    if ($LASTEXITCODE -ne 0) { throw "signtool verify failed" }
    Write-Host "signed: $ExePath"
}
