> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 133. 应用发现源扩展(App Paths + 便携目录)

便携软件(绿色版、解压即用)不写开始菜单、不是商店包,按 §29
的两个发现源完全不可见——用户报告搜不到自装的 Throne。参照
Flow Launcher Program 插件的发现策略补齐 Win32 覆盖,取其两源、
弃其一源。

## 决议

```text
新增源 1 = 注册表 App Paths(HKLM + HKCU 各一份,
           SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths):
           子键默认值 = exe 路径(可含引号/环境变量,REG_SZ 与
           REG_EXPAND_SZ 都收,ExpandEnvironmentStringsW 展开);
           .exe 后缀 + 文件存在过滤。这是 Windows 官方的"已安装
           Win32 应用"登记处,覆盖 NSIS/Inno/MSI 安装却不写开始
           菜单的一类
新增源 2 = 用户声明便携目录:设置 module.app.extra_dirs
           (String,分号分隔,默认空,RestartApplication——
           catalog 只在进程启动时构建,§56 不破)。递归 ≤4 层,
           扫 .exe/.lnk(lnk 复用 start_menu 的解析),跳过
           隐藏项;目录不存在仅 Warn。对应 Flow 的 Program
           Sources,但默认不带任何预置目录——Flow 预置扫描
           用户目录全家桶,噪声大
明确不做 = PATH 扫描(Flow 有,非递归):CLI 工具占 PATH 的
           主体,会淹没 GUI 结果;便携软件几乎不进 PATH,覆盖
           收益小。也不做 Flow 的 Win32 全称模糊纠错等排序层
           特性——本记录只管发现
顺序     = 开始菜单 → 商店 → App Paths → 便携目录,dedup 首见
           者胜(§30 偏好重复而非激进去重的结论不动):lnk 的
           显示名通常比 exe 文件名漂亮,先见的赢
过滤     = 卸载条目判定(is_uninstall_entry)从 start_menu
           提为 pub(crate) 三源共用;App Paths 的键名即显示名
           (去 .exe),便携目录用文件 stem
预算     = 两源都在模块发现线程内(§56 spike 模式),load 与
           热路径零变化
```

## 验证

```text
单测 = parse_dirs 分隔/裁剪/空段
E2E  = 真机:设置 module.app.extra_dirs=C:\Users\<u>\Apps,
       重启后搜 "throne" 命中(此前零结果);App Paths 源在
       本机新发现 10+ 条目(catalog ready 日志四源计数)
```

---

