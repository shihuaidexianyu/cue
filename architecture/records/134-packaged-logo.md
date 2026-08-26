> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 134. 商店应用真实图标(GetLogo 管线)

packaged 应用(UWP/MSIX)此前只有协议兜底图标——商店应用占日常
启动的相当比例,空白格子不可用。Flow Launcher 的做法是手解
AppxManifest.xml 找 Square44x44Logo 再按像素尺寸挑位图(忽略全部
qualifier);我们改走官方 WinRT API,qualifier(scale/contrast/
targetsize 资源间接)由系统正确处理。

## 决议

```text
提取   = AppListEntry.DisplayInfo.GetLogo(96×96) →
         OpenReadAsync 读流 → image crate 嗅探解码
         (PNG/BMP/JPEG)→ 非 96 则 Lanczos3 重采样到 96
         —— IconImage 契约(96px RGBA8 straight alpha)不变
留边   = UWP small logo 为开始菜单磁贴设计,自带一圈透明
         留边,直接进 96px 画布行内视觉小一圈。fill_bbox:
         内容包围盒 < 80% 画布时双线性放大到 80%(封顶 2×,
         防极小内容拉爆);满幅 logo 不动
索引交付 = 发现线程在发布 catalog 前把 AUMID → AppListEntry
         索引填进 IconPipeline(set_packaged_index):query
         只能看到已发布条目,worker 取 logo 时索引必然就绪
         ——零等待、不重复枚举(§99 资源自限的同款思路)
线程   = 图标 worker 持 ComGuard;GetLogo/OpenReadAsync 的
         .join() 阻塞都在 worker,UI 线程零 IO(§124 同款)
明确不做 = 手解 AppxManifest(Flow 方案):qualifier 处理
           不全会挑错图;manifest 路径解析、MRT 资源引用
           (ms-resource:)全是坑,官方 API 一行覆盖
```

## 验证

```text
审计 = packaged_logo_audit(ignored 单测):真机全量 packaged
       条目 29/29 提取成功,前 24 枚落 PNG 肉眼核对(计算器/
       照片/终端/Xbox/邮件均为官方图标,留边补偿后充满度正常)
单测 = icon_audit 扩展覆盖 §133 新源
E2E  = 真机搜商店应用(如"计算器")显示真实图标
```

---

