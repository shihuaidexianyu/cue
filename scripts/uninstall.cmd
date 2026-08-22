@echo off
rem 双击卸载入口:转调 uninstall.ps1(绕过执行策略,仅此进程)。
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0uninstall.ps1"
pause
