; CUE 安装包(Inno Setup 6):每用户安装、免管理员、简体中文向导。
; 不经手运行:由 scripts\package.ps1 在 release 构建后调 ISCC 编译,
; 版本号通过 /DAppVersion=<ver> 传入。
#if !Defined(AppVersion)
  #error "pass /DAppVersion=<ver> (scripts\package.ps1 does this)"
#endif

[Setup]
; 固定 AppId:升级安装/卸载条目靠它识别"同一个 CUE"。
AppId={{9A4F2C6D-1E7B-4A38-8F5C-2D6E9B0A3C71}
AppName=CUE
AppVersion={#AppVersion}
AppPublisher=CUE
DefaultDirName={localappdata}\Programs\CUE
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=CUE-Setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; setup.exe 与向导用品牌图标;"应用和功能"里的卸载条目用安装后的 exe 图标。
SetupIconFile=..\assets\cue.ico
UninstallDisplayIcon={app}\cue.exe
; 运行中的 CUE 持有单实例 mutex(§113)——安装/卸载前 Inno 会提示关闭;
; CloseApplications 走 Restart Manager 兜底自动关。
AppMutex=Local\CUE.SingleInstance
CloseApplications=yes
; 多语言:按系统 UI 语言自动选,匹配不上才弹语言选择框。
ShowLanguageDialog=auto

[Languages]
Name: "chs"; MessagesFile: "lang\ChineseSimplified.isl"
Name: "en";  MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "开机自动启动"; Flags: unchecked

[Files]
Source: "..\target\release\cue.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; 直接落在 开始菜单\Programs\CUE.lnk(AppModule 扫开始菜单,装完即可被 CUE 自己搜到)。
Name: "{autoprograms}\CUE"; Filename: "{app}\cue.exe"; Comment: "CUE —— 轻量启动器 (Alt+Space)"

[Registry]
; 勾选"开机自动启动"才写 Run 键;卸载时删除。与应用内
; 设置→开机自启(§36)写的是同一个键,两边天然一致。
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "CUE"; ValueData: """{app}\cue.exe"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\cue.exe"; Description: "运行 CUE"; Flags: nowait postinstall skipifsilent

; 注:数据目录 %LOCALAPPDATA%\CUE(设置/使用统计)卸载时保留——
; Inno 卸载器只清程序与快捷方式,不碰用户数据。
