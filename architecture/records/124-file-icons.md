> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 124. 文件结果的真实图标(按路径/扩展名异步提取)

初版文件结果只有两枚通用图标(文件夹/文件),`/geek.exe` 这类
可执行文件不显示内嵌图标。补全为 AppModule 同款图标管线:
提取异步化,完成推 `PresentationInvalidated` 让 Core 重画可见行。

## 决议

```text
缓存 key = exe 类(exe/lnk/msi/bat/cmd/com/scr,图标内嵌于文件)
           按全路径;其余按小写扩展名(同类文件共享一枚,
           虚拟名 "x.<ext>" 走 SHGFI_USEFILEATTRIBUTES,不触盘);
           文件夹与无扩展名文件不进队列(通用图标)
线程模型 = module 自有 worker 串行提取(AppModule IconPipeline
           同款;两处相似而非相同——Slot 模型与 key 策略不同,
           按 §72 第二处允许重复,不下沉 util)
失效寻址 = 完成时以当前行快照(last_items)发失效事件;
           Core 只取与当前结果的交集(any 命中即整体重画),
           同扩展名兄弟行天然覆盖,请求登记行滚走也不怕
纪律     = present() 热路径零 IO(锁内查表,未命中登记 Pending);
           失败负缓存不重试;缓存超 512 枚整体清空(≈ 19 MB,
           重建便宜,不做 LRU);通用图标仍是全部兜底,
           再退 SystemIcon
明确不做 = 图标磁盘缓存(提取是毫秒级,重启重建足够)、
           队列优先级/可见行优先(20 行规模无意义)、
           第三处出现前的管线下沉(§72 Rule of Three)
```

## 验证

```text
单测 = icon_key 路由(文件夹/无扩展名 None;exe 类全路径;
       扩展名小写共享)、extract 未知前缀拒绝
E2E  = 真机:/geek.exe 行从通用白图标变为 exe 内嵌图标
       (异步提取 + 失效重画,截图)
```

---

