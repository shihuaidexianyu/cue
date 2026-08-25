> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 132. 诊断日志:全局 sink、有界单文件、后台写线程

V1 的全部诊断输出走 `eprintln!`(stderr)。debug 下 `cargo run`
可见,但 release 是 GUI 子系统、没有控制台,stderr 无人可见——
**用户不是没遇到报错,而是遇到了看不见**。实机案例:某台机器上
`packaged discovery unavailable: 拒绝访问 (0x80070005)` 每次启动
都写(stderr),packaged 应用长期全体缺席,用户无感,直到一个
具体应用搜不到才顺藤摸瓜发现。诊断信息必须落在用户能拿到的地方。

## 决议

```text
落点     = cue-protocol::log(LogLevel/ModuleLogger 本就住在
           protocol;std-only,平台中立,§110 不受影响;依赖方向内
           core/windows/编排层都可达)
文件     = <storage_root>/cue.log;启动时 >1MB 滚动为 cue.log.old
           (只留一代,总量封顶 2MB——usage.tsv 的"有界"同纪律)
写路径   = 调用方 format + 一次 channel send(纳秒级);专属
           log-writer 线程承载全部文件 IO——杀毒软件/磁盘抖动
           卡的是写线程,不是 UI 线程(§55 热路径零负担)
落盘     = 不做 fsync:WriteFile 进 OS 页缓存后进程崩溃也不丢
           (缓存归内核管);电源失效级持久化不是诊断日志的目标
失败纪律 = 打开失败 / 写线程死亡 → 退回纯 stderr;日志永不
           panic(§63 同款)
接入     = logln! 宏与 eprintln! 同形,全量替换(仅测试辅助
           输出保留 eprintln);Core 的模块 logger 实现同走 sink;
           文件 + stderr 双写,debug 的 cargo run 体验不变
可发现性 = 设置页只读 Path 行 core.log_file,Enter 用系统默认
           程序打开(§122 现成机制)——"看不到的报错怎么办"的
           正面回答
时间戳   = UTC RFC3339 秒级(protocol 平台中立,不碰本地时区);
           手算 civil_from_days,零新依赖
```

## 性能论证(回应"IO 慢,要不要延迟写")

延迟/异步写的担忧针对的是两种成本,需要拆开:

```text
WriteFile(进页缓存) = 一次系统调用,µs 级;磁盘由 OS 惰性回写,
                      不挡调用方。CUE 日志最热路径 = 每击键一条
                      ~100B 行,对 P50<5ms 预算无感
fsync(强制落盘)    = 5–50 ms,真正贵——但诊断日志不需要:
                      进程崩溃不丢页缓存内容,只有断电/蓝屏才丢
结论               = 既不用 fsync,也不靠"延迟写"摊薄——直接让
                     IO 离开 UI 线程(写线程),热路径只剩 channel
                     send;这是比缓冲批量写更简单的结构性答案
```

## 验证

```text
单测(protocol::log):civil_from_days 已知日期(含闰日/纪元前)、
  时间戳格式、未 init 时 write 不 panic、超 1MB 滚动 + 写线程
  回写 roundtrip
真机:cue.log 记录 [boot]/[host]/[dnd] 全链路行;cargo run
  (debug)控制台输出不变;scripts/check-arch.ps1 通过
```
