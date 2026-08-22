# CUE 安装(每用户,无需管理员):
#   %LOCALAPPDATA%\Programs\CUE\cue.exe  +  开始菜单快捷方式
# 开机自启默认关闭;装好后在 托盘 → 设置 → 开机自启 里打开(§42 事务写 Run 键)。
$ErrorActionPreference = "Stop"
$src = Join-Path $PSScriptRoot "cue.exe"
if (-not (Test-Path $src)) { throw "cue.exe not found next to install.ps1" }

Get-Process cue -ErrorAction SilentlyContinue | Stop-Process -Force -Confirm:$false

$dst = Join-Path $env:LOCALAPPDATA "Programs\CUE"
New-Item -ItemType Directory -Force $dst | Out-Null
Copy-Item $src (Join-Path $dst "cue.exe") -Force

# 开始菜单快捷方式:CUE 自己的 AppModule 也能搜到它。
$lnkPath = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\CUE.lnk"
$shell = New-Object -ComObject WScript.Shell
$lnk = $shell.CreateShortcut($lnkPath)
$lnk.TargetPath = Join-Path $dst "cue.exe"
$lnk.WorkingDirectory = $dst
$lnk.Description = "CUE - lightweight launcher (Alt+Space)"
$lnk.Save()
[System.Runtime.InteropServices.Marshal]::ReleaseComObject($shell) | Out-Null

Write-Host "installed: $dst\cue.exe"
Write-Host "shortcut:  $lnkPath"
Write-Host ""
Write-Host "按 Alt+Space 唤起。开机自启:托盘右键 → 设置 → 开机自启。"
