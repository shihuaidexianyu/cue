> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 135. Segoe 字形图标;SystemIconId 退役

兜底图标此前是 emoji(UI 把 SystemIconId 映射成 🚀🔒😴…):彩色
emoji 与 Windows 原生图标语言不符,且"画什么字形"的决定漏进了
UI 层——协议携带语义枚举、UI 决定呈现,两处都越界。改为模块自己
渲染单色字形位图,协议只认 Raster。

## 决议

```text
渲染   = cue-util-win::glyph:Segoe Fluent Icons / Segoe MDL2
         Assets 的 PUA 字形(同码位表,按序回退),GDI 灰阶
         抗锯齿白字黑底画进 32bpp DIB,亮度通道 = 覆盖率 →
         straight alpha × 前景色(默认 0xE6E6E6 标题灰,
         深色底对比足够、不抢彩色图标)。不用 ClearType
         (会留彩色边缘);GetGlyphIndicesW 存在性检查,
         族内缺失换族,不画 .notdef 豆腐块
码位表 = 系统动作 7 枚(load 渲染存表,present 克隆 Arc——
         UI 按 Arc 指针缓存纹理):锁 E72E / 睡眠 E708(月)/
         休眠 E9CA(Frigid 温度计,"冻结到磁盘"隐喻,无官方
         休眠字形,一行可换)/ 注销 F3B1 / 重启 E72C /
         关机 E7E8 / 回收站 E74D。模块兜底:app ECAA
         (AppIconDefault)/ 文件夹 E8B7 / 文件 E8A5 /
         书签地球 E774
缓存   = cached_glyph:app/file/bookmark 三处兜底同形,
         按 §72 Rule of Three 下沉 util-win;同码位同色永远
         返回同一 Arc 缓冲;None(字体缺失)也缓存,不在
         present() 热路径重试
协议   = SystemIconId 与 ResultIcon::SystemIcon 删除,
         ResultIcon 只剩 Raster——协议不再携带"画什么"的
         语义枚举,字形决定回到模块(§3 的本来面貌)。
         取代 §126 决议的图标行,以及 §117/§118/§124 中的
         SystemIcon 兜底表述(兜底职责不变,载体换成字形
         位图);§14 契约同步修订
渲染失败 = 留空槽位(None),不影响激活;系统模块 load 时
           整表渲染,缺一枚仅 Warn
```

## 验证

```text
审计 = glyph_sheet(ignored 单测):候选码位渲成 contact
       sheet 落 PNG,肉眼选定上表(休眠候选两批,E9C8 空白、
       E7BA/E80A 是时钟,定 E9CA)
单测 = 全量绿;system 的 present 测试改为断言未 load 时空槽位
E2E  = 真机 `>` 列出 7 动作目视字形;商店应用 logo 未就绪行
       显示 ECAA 兜底
```

---

