> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 121. 排除名单细化与可编辑名单(String 设置)

§120 的初版名单过粗:`/d` 的结果仍被 `AppData\Local\MathWorks`、
`.vscode\extensions` 这类"工具内脏"淹没。两处修订:默认名单细化,
名单本身成为可编辑的 module 设置。

## 现成口径的调研结论

```text
VS Code   = files.exclude / search.exclude 默认名单是事实标准:
            **/node_modules、**/bower_components、**/.git、**/.svn、
            **/.hg(+ .DS_Store / Thumbs.db 这类文件级,对目录排除
            不适用)
Windows   = Search 索引器默认范围 = 用户目录但明确排除 AppData——
Search    "AppData 默认不可见"有系统级先例
社区实践  = Everything 用户名单普遍加 __pycache__、.venv、
            go/pkg/mod 等语言包缓存(voidtools 论坛 / 配置分享)
```

## 决议

```text
默认名单 = 两类片段(都以 \ 结尾,锚定"目录"):
           全局片段(任意位置生效,VS Code 口径):
             C:\Windows\、C:\Program Files\、C:\Program Files (x86)\、
             \$Recycle.Bin\、\node_modules\、\.git\、\.svn\、\.hg\、
             \__pycache__\、\.venv\、\bower_components\
           USERPROFILE 展开(工具内脏,Windows Search 口径):
             AppData、.vscode、.cursor、.cargo、.rustup、.gradle、
             .m2、.npm、.nuget、.docker、.android
           (有意保留 .claude / .ssh 等:用户会直接找;
           项目内 .vscode 不在名单——编辑的是项目配置,不是扩展缓存)
设置     = ~~module.file.excluded_paths(String,分号分隔片段,
           Immediate;try_apply 时重建否定子句)~~(已推翻,§122:
           名单搬出 Settings Host,成为模块数据文件)
           + module.file.exclude_noise_paths(Bool 总开关,§120)
UI       = ~~设置页 String 行进入编辑态(视图本地缓冲,
           与搜索页同一套文本录入:命名键 + key_char + space 特判
           + Ctrl+V;Enter 提交走 §42 事务、Esc 放弃)——
           V1 编辑面扩展为 Bool + Hotkey + String
           (首个 String 设置,无独立文本框组件,复用行内渲染)~~
           (已推翻,§122:长名单行内编辑无光标移动,编辑面回退为
           Bool + Hotkey;名单改由默认编辑器打开)
逃生口   = 不变(查询含 \ 原样发送);名单清空 = 不排除
明确不做 = 名单的正则/通配语法(子串足矣)、按名单重排、
           白名单模式、AppData 内细分(整目录排除 + 逃生口已够)
```

> 本节的默认名单后被 §125 再次修订:USERPROFILE 展开在
> 真机证伪(多用户/沙箱配置的 AppData 照样涌入),改为
> 目录锚定 `\AppData\` 通杀 + ProgramData;String 设置本身
> 也已在 §122 随名单外置一并撤除。本节保留决议过程。

## 验证

```text
单测 = build_clause 解析(分号/trim/空段)、默认名单含
       AppData 与 .vscode 根、effective_search 四条路径
       (默认拼接/总开关关闭/空名单/显式路径逃生)、
       try_apply 双键回合(类型错误 Err 不留半状态)
E2E  = 真机:/d 结果不再出现 MathWorks ServiceHost 与
       .vscode\extensions(截图);设置页名单行可编辑(截图)
```

---

