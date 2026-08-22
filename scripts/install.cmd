@echo off
rem 双击安装入口:转调 install.ps1(绕过执行策略,仅此进程)。
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"
pause
