> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 138. FileModule 自建索引(取代 Everything)

用户决策:不需要 Everything 级别的全盘强搜索,FileModule 收窄为
"简化版 Everything"——覆盖日常文件区域即可,换取零第三方依赖
(此前 Everything 1.4 必须已安装且运行,否则模块整体 Unavailable)。

**本章取代 §31(v0.2 定案)与 §118(Everything IPC 实现);§32/§33
(FileEntry 内部持有、文件文件夹同模态)不变。**

## 选型记录

```text
调研   = Flow Launcher / PowerToys Run:Windows Search(OLE DB
         SystemIndex)——C# 一行的事,Rust 要手撸 COM,且 WDS
         默认只索引库目录,范围反而更窄
       = MFT/USN 全家(UltraSearch / usn-parser-rs / Orange 的
         MFT 模式):全部要求管理员——USN journal 读、MFT 直读
         都是运行时特权,安装时提权解决不了
MFT 路线否决 = 让 launcher 全程管理员运行:启动的每个子进程都
         继承高权限(等于把提权传播给用户所有应用)+ UIPI
         交互限制;唯一干净形态是独立 SYSTEM 服务 + IPC——
         那就是把 Everything 重新造一遍,违背"简化"目标
定案   = Orange(naaive/orange,Rust+Tauri,1.8k★)验证过的
         无管理员形态:遍历建索引 + ReadDirectoryChangesW
         增量 watcher,全部用户态 API
```

## 决议

```text
范围   = %USERPROFILE% 整树 + Desktop/Documents/Downloads
         已知文件夹(SHGetKnownFolderPath,可指向树外)+
         module.file.index_dirs(`;` 分隔,RestartApplication);
         小写归一 + 去嵌套后逐根爬行
建索引 = 模块自有线程:先起 watcher(爬窗期的变更进队列不丢)
         → 迭代栈遍历 → 发布快照;query 用 futures::poll_fn
         等一次性 readiness gate(§99 资源自限:模块自己
         兜住首次爬行时长,Core 无感)
watcher = 每根一个阻塞 ReadDirectoryChangesW(bWatchSubtree,
         64KB 缓冲);事件 200ms 静默窗合批(上限 4096 条)
         → 增量应用;缓冲溢出 / watcher 死亡 → 全量重爬
         (watcher 死亡不重生该根:记录在案的限制,根级致命
         错误极少见,留给下次进程启动,内容已由重爬修正)
语义   = rename = delete+add(不配对新旧名);目录删除 =
         前缀批量移除;新增目录 = 子树补爬
reparse = reparse point 目录一律跳过(防 OneDrive/联接点成环);
         OneDrive 占位文件(普通文件)保留
排除   = 名单(§120–125)双重应用:爬行时子树剪枝(省内存)
         + 查询时文件级过滤(逃生口一致性);`\` 逃生口语义
         收窄:只对"已收进索引"的条目生效,够不到被剪枝的子树
查询   = 子串匹配(小写 path)+ ext: 后缀子集;Everything
         原生语法的其余部分(正则/大小写/操作符)随之退役
排序   = 名称命中 > 路径命中,各自 name_lower 升序
         (Everything NAME_ASCENDING 原样展示的对等物)
删除   = everything.rs 整体移除(WM_COPYDATA 客户端、
         message-only 应答窗、latest-wins 请求槽);§99 的
         "专用线程 + 资源自限"精神由 index 线程继承
```

## 验证

```text
单测 = index:roots 去重去嵌套 / search 语义(名称优先、ext:、
       排除过滤、`\` 逃生口)/ 剪枝 / 临时目录真爬
     = lib:名单文件链路(seed/重读/mtime 跟踪/坏 TOML 保持
       旧值)/ 设置 schema 3 行
E2E  = 真机:杀掉 Everything 进程(或卸载)→ `/` 搜索照常;
       新建文件秒级可搜(watcher);cue.log 看爬行条数与耗时
```
