> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 139. 产品改名 CUE → sakana

产品名 CUE 改为 sakana。动机:所有者偏好;无功能含义。本章记录
改名的执行范围与迁移矩阵——历史记录(§1–§138)中的 CUE 字样
不改写(append-only),本章之后的新章节用 sakana。

## 决议

```text
全量改名 = crate 目录与包名(cue-* → sakana-*,含 binary
         cue → sakana.exe)、产品字面值(托盘/菜单/设置文案/
         安装器字段)、持久标识(数据目录/互斥体/窗口类名/
         Run 键值名/日志文件名/安装目录)、脚本与文档
版本    = 改名当版 0.4.10 → 0.5.0
迁移    = 新进程启动时一次性完成,全部幂等、失败仅记日志重试:
         存储根   = %LOCALAPPDATA%\CUE → sakana(新目录不存在
                    且旧目录在 → fs::rename 同卷原子迁移;
                    两边都在 → 保留新目录,旧目录不删)
         自启值   = HKCU Run 的 `CUE` 值存在 → 以当前 exe
                    路径写入 `sakana` 值成功后再删旧值
         安装器   = AppId GUID 不变(CUE 时代沿用)→ 旧安装
                    被识别为前身走升级路径;AppMutex 同时列
                    新旧两个互斥体名,运行中的旧版也能被检测;
                    UsePreviousAppDir=no 强制装到新目录,
                    InstallDelete 清旧 Programs\CUE 目录;
                    卸载时兜底删旧 Run 值名
已知边界 = 新旧版互不构成单实例(互斥体/窗口类名都换了)——
         双跑只可能发生在升级前手工并行的场景,无害;
         第二实例唤起信号对新旧版互不送达
明确不改 = 文件格式魔数(cue-settings-v1 / cue-usage-v1 /
         图标缓存 "CUEI")——它们是格式版本标识符不是品牌,
         改了=用户数据作废,零收益;目录迁移后旧头照常解析
         品牌图标图形(assets 只换文件名,鱼形新图标后续单独
         设计);GitHub 仓库名与本地仓库目录名(改名是仓外
         操作,链接有重定向兜底)
```

## 验证

```text
编译器 = crate 改名后 cargo test / clippy / check-arch.ps1 全绿
       (import 面:protocol 31 文件、util-win 11、其余各 1–2)
E2E  = 旧版在 %LOCALAPPDATA%\CUE 有数据 → 跑新版 → 目录随迁、
       设置/用量/图标缓存都在;HKCU Run 旧值换名指向新 exe;
       setup 打包装旧版之上 → 检测运行中的旧版、同 AppId 升级、
       旧安装目录被清
```
