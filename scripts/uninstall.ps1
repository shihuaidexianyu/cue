# CUE 卸载:停进程 → 删 Run 键/快捷方式/安装目录。
# 默认保留数据目录 %LOCALAPPDATA%\CUE(设置、使用统计);-Purge 一并删除。
param(
    [switch]$Purge
)
$ErrorActionPreference = "Stop"

Get-Process cue -ErrorAction SilentlyContinue | Stop-Process -Force -Confirm:$false

$runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
if (Get-ItemProperty -Path $runKey -Name "CUE" -ErrorAction SilentlyContinue) {
    Remove-ItemProperty -Path $runKey -Name "CUE"
    Write-Host "removed autostart entry"
}

$lnk = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\CUE.lnk"
Remove-Item $lnk -Force -ErrorAction SilentlyContinue

$dst = Join-Path $env:LOCALAPPDATA "Programs\CUE"
Remove-Item $dst -Recurse -Force -ErrorAction SilentlyContinue
Write-Host "removed $dst"

$data = Join-Path $env:LOCALAPPDATA "CUE"
if ($Purge) {
    Remove-Item $data -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "purged $data"
} elseif (Test-Path $data) {
    Write-Host "kept data dir $data (re-run with -Purge to delete)"
}
Write-Host "uninstalled."
