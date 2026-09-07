> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 137. 应用图标磁盘缓存 + 启动预载

症状:重启 CUE 后第一次搜索,图标肉眼可见地逐个弹入。§124 的
"提取是毫秒级"论证对 FileModule 的虚拟图标成立(SHGFI_USEFILEATTRIBUTES
不触盘),对应用图标不成立——Win32 要触盘提取 exe 资源,packaged 要
WinRT GetLogo + 解码,且 worker 串行逐枚处理。内存缓存在进程退出时
清空,每次重启都从零开始。

## 决议

```text
落盘   = 最终渲染完成的 96×96 RGBA 直接序列化到
         modules/app/cache/icons/<fnv64(icon_key)>.bin;
         重启后 load 阶段整目录预读进内存缓存,worker 照常补缺
头格式 = MAGIC("CUEI")+ PARSE_VERSION + RENDER_EPOCH
         + stamp kind + 3×u64 stamp + key 校验串(防哈希碰撞发错图标)
stamp  = 失效指纹:Win32 = exe mtime + size;Packaged = 打包后
         的包版本号(Major.Minor.Build.Revision → u64)
         ——应用更新必然改写 exe / 升版本,旧缓存自动失配重建
渲染 epoch = 缓存存的是 bbox 归一化之后的像素;改渲染管线
         (fill_bbox / 解码策略)时手动 RENDER_EPOCH +1,
         旧缓存整体失效重建——不靠记忆,靠文件头
预载   = catalog 发布前 preload_from_cache:命中直接进 Slot 缓存
         (Ready),首批结果即带图标;只填缺省,不覆盖 Pending/
         Ready 的内存项
GC     = 只对"当前 catalog 里的 key"做 stamp 失配删除;
         不认识的 key 一律保留——packaged 发现可能整体失败
         (E_ACCESSDENIED 历史包袱),白名单式清空会把好缓存误杀
不缓存 = 取不到版本号的 packaged 条目:宁可不缓存也不错缓存
写入   = tmp+rename,崩溃不留半个文件;读取任一校验失败
         (magic/版本/epoch/stamp/key/截断)按未命中处理并删文件
不变   = IconPipeline 对外的 Slot/Pending/Ready 语义、
         PresentationInvalidated 推送路径、present() 零 IO 全部不动;
         缓存只是 worker 提取前的一道前置查找 + 写穿
```

## 验证

```text
单测 = icon_cache:roundtrip / 各类失效(stale stamp、错 kind、
       坏 magic、旧 epoch、截断、key 校验)/ 残留 .tmp 不干扰
     = icon:preload_from_disk_cache(命中直接 Ready;Pending
       不被覆盖;stale stamp 未命中且文件被删)
E2E  = 真机:删 modules/app/cache/icons → 重启(重建缓存)→
       再重启 → 首屏图标即出,无逐枚弹入
```
