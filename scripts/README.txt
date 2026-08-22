CUE —— 轻量 Windows 启动器
=============================

Alt+Space 唤起,直接输入搜应用(拼音全拼/首字母均可),Enter 启动。
托盘图标右键 = 显示 / 设置 / 退出。

安装(每用户,无需管理员)
------------------------
双击 install.cmd,或:
    powershell -ExecutionPolicy Bypass -File install.ps1

安装内容:
    %LOCALAPPDATA%\Programs\CUE\cue.exe     程序本体
    开始菜单\程序\CUE.lnk                    快捷方式

开机自启默认关闭。开启:托盘右键 → 设置 → 开机自启
(写入 HKCU Run 键,无需管理员)。

卸载
----
双击 uninstall.cmd(或 uninstall.ps1)。
默认保留数据目录 %LOCALAPPDATA%\CUE(设置与使用统计);
彻底清除:powershell -ExecutionPolicy Bypass -File uninstall.ps1 -Purge

说明
----
- 单实例:重复启动只会唤起已在运行的实例。
- 设置与统计:%LOCALAPPDATA%\CUE\settings.tsv / usage.tsv
- 应用目录在进程启动时刷新;新装的应用重启 CUE 后可搜到。
