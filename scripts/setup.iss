; sakana 安装包(Inno Setup 6):每用户安装、免管理员、简体中文向导。
; 不经手运行:由 scripts\package.ps1 在 release 构建后调 ISCC 编译,
; 版本号通过 /DAppVersion=<ver> 传入。
#if !Defined(AppVersion)
  #error "pass /DAppVersion=<ver> (scripts\package.ps1 does this)"
#endif

[Setup]
; 固定 AppId:CUE 时代沿用至今(§139 改名不换),升级安装/卸载条目
; 靠它识别"同一个应用",旧版 CUE 安装会被当作本应用的前身升级。
AppId={{9A4F2C6D-1E7B-4A38-8F5C-2D6E9B0A3C71}
AppName=sakana
AppVersion={#AppVersion}
AppPublisher=sakana
; §139:不复用旧版的安装目录(CUE 时代是 Programs\CUE)。
UsePreviousAppDir=no
DefaultDirName={localappdata}\Programs\sakana
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=sakana-setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; setup.exe 与向导用品牌图标;"应用和功能"里的卸载条目用安装后的 exe 图标。
SetupIconFile=..\assets\sakana.ico
UninstallDisplayIcon={app}\sakana.exe
; 运行中的实例持有单实例 mutex——安装/卸载前 Inno 会提示关闭;
; CloseApplications 走 Restart Manager 兜底自动关。
; 新名 + CUE 时代旧名都列出,升级时运行中的旧版同样被检测到。
AppMutex=Local\sakana.SingleInstance,Local\CUE.SingleInstance
CloseApplications=yes
; 多语言:按系统 UI 语言自动选,匹配不上才弹语言选择框。
ShowLanguageDialog=auto

[Languages]
Name: "chs"; MessagesFile: "lang\ChineseSimplified.isl"
Name: "en";  MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "开机自动启动"; Flags: unchecked

[Files]
Source: "..\target\release\sakana.exe"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; §139:升级自 CUE 时代时清掉旧安装目录(旧 cue.exe 留在里面)。
Type: filesandordirs; Name: "{localappdata}\Programs\CUE"

[Icons]
; 直接落在 开始菜单\Programs\sakana.lnk(AppModule 扫开始菜单,装完即可被 sakana 自己搜到)。
Name: "{autoprograms}\sakana"; Filename: "{app}\sakana.exe"; Comment: "sakana —— 轻量启动器 (Alt+Space)"

[Registry]
; 勾选"开机自动启动"才写 Run 键;卸载时删除。与应用内
; 设置→开机自启写的是同一个键,两边天然一致。
; 旧值名 "CUE" 由应用启动时的 §139 迁移接管换名;卸载时一并删除兜底。
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "sakana"; ValueData: """{app}\sakana.exe"""; Tasks: autostart; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "CUE"; Flags: deletevalue uninsdeletevalue

[Run]
Filename: "{app}\sakana.exe"; Description: "运行 sakana"; Flags: nowait postinstall skipifsilent

; 注:数据目录 %LOCALAPPDATA%\sakana(设置/使用统计)卸载时保留——
; Inno 卸载器只清程序与快捷方式,不碰用户数据。
