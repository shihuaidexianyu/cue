> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 118. FileModule 实现记录(Everything,V1.x)

文件搜索。§31–33 的实现落点;设计裁剪与关键决策记录于此。

## 范围与触发词

```text
module id  = file
trigger    = /(标点触发,§5.2 verbatim 匹配;/ 之后原样进 Everything)
范围       = Everything 索引内的文件 + 文件夹,同一模态(§33)
查询语法   = Everything 原生语法原样透传(子串、ext:、路径过滤……)
             ——query syntax 归 Module,Core 不解析(§3)
明确不做   = 自建索引(§76)、随包分发 Everything、Everything.dll/SDK 进仓库、
             按扩展名的真实类型图标、usage 重排、~~次级 action~~(已落地,§119)
```

## 依赖与 IPC(§31 定案的执行)

```text
依赖   = 本机已安装并运行的 Everything 1.4(QUERY2W 需要 1.4.1+);
         未运行 / 主版本不符 → ModuleError::Unavailable,行内错误文案
协议   = 直连 WM_COPYDATA,逐字节对齐官方 SDK(everything_ipc.h /
         Everything.c,随取随弃,不进仓库):
         FindWindowW("EVERYTHING_TASKBAR_NOTIFICATION") →
         WM_USER 握手主版本 → WM_COPYDATA(dwData=18, QUERY2
         = 7×u32 头 + NUL 结尾 UTF-16 搜索串)→ 应答 WM_COPYDATA
         回 reply_hwnd(LIST2 = 5×u32 头 + ITEM2[n] + 变长数据)
变长   = 按 request flag 位升序;字符串 = u32 字符数(不含 NUL)+
         文本 + NUL;SIZE/DATE = 8 字节。一切读取查边界(§63)
排序   = NAME_ASCENDING 原样展示(SDK 保证该序无性能损失);
         V1 不做 usage 重排(Everything 的序就是预期序)
请求字段 = FULL_PATH_AND_NAME | SIZE | DATE_MODIFIED
         (DATE_MODIFIED V1 不展示——留它让变长解析的位序有第三个
         数据点,测试断言其值域;V1.x 展示/排序直接用)
```

## 线程模型(§99 照办)

```text
专用 IPC 线程(Win32 IPC 同步阻塞)+ 容量 1 的 latest-wins 请求槽:
新输入顶掉旧请求,旧 future 以 Canceled 结束,Core 按 ticket 丢弃(§91)
应答窗口(message-only)在 IPC 线程上创建,GetMessage 泵派发"发送"型
应答(官方 dll 同款流程);2 s WM_TIMER 兜底,不应答不吊死
load() 只起线程,不触碰 IPC(§55:唤烧热路径无 Everything 初始化)
```

## 空查询与展示

```text
空查询 = 空结果。UsageRead 只能按键查、不能枚举(§50),
         给不出 Top Files;不显示任何推荐内容(§115 精神)
行展示 = title(文件名)+ 副标题(父目录)+ accessory
         (文件夹 → "文件夹";文件 → 格式化尺寸)
图标   = 文件夹 / 通用文件各一张,load 时后台线程一次性提取
         (SHGFI_USEFILEATTRIBUTES,不触盘);失败 → SystemIcon 兜底。
         逐类型图标(Flow 同款)需要按扩展名缓存,V1.x 再议
usage  = item_key = 全路径(§51);PRIMARY Open = ShellExecute 默认动词
         (文件走系统关联,文件夹进资源管理器);成功 Close(§115)
```

## 验证

```text
单测 = QUERY2 布局、LIST2 解析(合成缓冲)、畸形输入不 panic(§63)、
       parent/name 拆分、present 兜底、空查询、live 冒烟
       (Everything 不在自动跳过)
实测 = 真机 E2E:/explorer → 8 行(文件夹图标/名称/父目录/配件齐全),
       input→rows 18–40 ms(IPC 往返,逐键),Enter 打开首行文件夹,
       usage.tsv 正确落(file, PRIMARY, 全路径)
注意 = FileModule 的覆盖范围 = Everything 的索引范围。Everything
       服务未运行 / 索引过期时,结果偏少是数据源问题,模块如实展示
```

Rule of Three:icon / com / shell_execute 在此第三次复制(§72),
随后即下沉 cue-util-win(模块共享 Win32 助手,只下沉不上浮)。
---

