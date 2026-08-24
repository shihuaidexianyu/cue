> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 125. 排除名单通用化(目录锚定 `\AppData\` + ProgramData)

§121 的默认名单把 AppData 与工具缓存按 USERPROFILE 展开,真机
证伪:多用户配置与沙箱配置(WsAccount、CodexSandboxOffline/Online、
Default)的 AppData 照样涌入结果;ProgramData 同样是应用数据
而非用户文件(Windows Search 仅为 Start Menu 收录它)。§121 对
AppData 的反转先例在此继续:排除口径从"精确但只管自己"改为
"目录锚定通杀"。

> **当前状态**(§120 → §121 → §122 → §123 → §125 五节链条的
> 落点):名单 = 模块数据文件 `modules/file/data/excluded-paths.toml`
> (TOML `excluded` 数组,首启播种,内容恰为旧默认才一次性升级);
> 设置页两行 = Bool 总开关 `module.file.exclude_noise_paths` +
> Path 行 `module.file.excluded_paths_file`(回车用默认编辑器打开,
> 非值变更不走事务);生效 = 查询串拼 `!"片段"` 否定子句,mtime
> 指纹惰性重读;逃生口 = 查询含 `\` 原样发送。

## 决议

```text
新默认 = 系统目录(C:\Windows\、Program Files ×2、ProgramData、
         $Recycle.Bin)+ 通用 '\AppData\'(任意配置通杀)+ 依赖
         目录(node_modules/.git/…,口径不变)+ USERPROFILE 展开
         的工具缓存(.vscode/.cargo/…,保持按户:项目级同名目录
         多是配置而非缓存,不按通用排除)
存量升级 = 一次性:文件内容恰为旧默认名单(用户一个片段都没
         动过)才重写为新默认;增删过任何片段即不触碰;幂等
         (新默认 ≠ 旧默认,不会二次触发);读取/解析/写入失败
         都不致命,沿用现状
```

## 验证

```text
单测 = 旧默认内容触发重写且幂等、自定义内容不动、
       seed 子句含 ProgramData 与通用 \AppData\
```

---

