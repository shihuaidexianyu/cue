> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 120. FileModule 噪声目录排除(V1.x)

`/ds` 之类短查询的结果曾被 `C:\Program Files\...` 安装目录内部文件
淹没——那些不是用户工作时会用的文件。决议:黑名单而非白名单——
白名单砍掉 Everything"全盘随查"的核心价值(无法预知目标文件落在
哪个角落),而噪声高度可预测。

## 决议

```text
形态     = 默认开启的排除名单(黑名单),不是结果后过滤:
           FileModule 把否定子句拼进发给 Everything 的查询串,
           噪声路径根本不占结果位
实现     = Everything 原生语法:含反斜杠的词按全路径子串匹配,
           `!"片段"` 即否定。查询 = `{用户输入} {EXCLUDE_CLAUSE}`
名单     = C:\Windows\、C:\Program Files\、C:\Program Files (x86)\、
           \$Recycle.Bin\、\node_modules\、\.git\
           (保守:AppData、ProgramData 不排除——确有用户文件)
逃生口   = 查询含 `\` 视为显式路径输入,原样发送不加排除——
           刻意找系统文件(如 /C:\Windows\explorer.exe)不被拦
设置     = module.file.exclude_noise_paths(Bool,默认 true,
           Immediate)——首个 module.* 设置;V1 设置 UI 的 Bool
           行直接可编辑,settings.tsv 持久化
           ~~(名单本身不设 String 编辑 UI)~~(已推翻,§121)
明确不做 = ~~用户自定义名单 UI~~(已落地,§121)、
           按名单重排(软黑名单)、白名单模式
```

> 名单的"保守"判断(AppData 不排除)在真机使用中证伪:
> AppData\Local、.vscode\extensions 才是短查询的主要噪声源。
> 后续链条:名单细化见 §121,外置为模块数据文件见 §122,
> TOML 定案见 §123,默认名单最终形态(目录锚定通杀)见 §125。

## 验证

```text
单测 = effective_search 三条路径(默认拼接/开关关闭/显式路径逃生)、
       ext: 函数查询仍走排除;schema 声明与 try_apply 回合
       (类型错误 Err 不留半状态);Core 集成:module.* 设置
       事务直达模块、入模型、未知 key 拒绝(首个 module.* 设置
       的端到端覆盖)
E2E  = 真机:/ds 结果从满屏 MATLAB 安装目录变为用户目录文件
       (截图);/C:\Windows\explorer.exe 照常命中(逃生口,截图)
```

---

