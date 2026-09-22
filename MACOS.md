# sakana macOS 适配:结论与路线

> 2026-09-22 · 依据本日兼容性全量分析与 §153 门禁落地(CI 双平台全绿)。
> 后续动工的入口文档;规格事实以 architecture/ 为准,门禁决策见 §153。

## 一、结论

1. **架构承诺兑现(§110/§111)**:core / protocol / util-common / ui 四个 crate 源码零平台引用,CI 双平台验证可编译、测试全绿(macos job 73 项测试 0 失败)。macOS 适配不是架构重写,而是宿主层映射 + 模块语义换血。
2. **gpui 0.2.2 在 aarch64-darwin 原生编译通过**(clippy 2m02s 实测);`windows-manifest` 是空 feature,macOS 无副作用,Cargo 无门控需求。
3. **债务主体**:宿主层(sakana-windows 九项能力)、五个模块的 Windows 业务语义、util-win。composition root 装配序需按 macOS 事件模型重写,但 Box 闭包注入接口风格可原样复用(维持 §110 对 HostPlatform trait 的拒绝)。
4. **数据迁移零负担**:设置/usage/索引/排除名单按平台全新播种,无存量升级问题。
5. **真正需要真机的只有一小层**:渲染/IME/热键手感、权限弹窗、打包发版——攒批租云 Mac 验证即可,不必常备设备。

## 二、风险要点

| 等级 | 风险 | 要点 |
|---|---|---|
| 高 | FileModule 换血 | 路径方言(反斜杠/盘符锚定)+ watcher RDCW→FSEvents;§142/§146/§150/§152 四代不变量重验 |
| 高 | composition root 重写 | 装配序承载 §131/§142 事故教训,macOS 事件模型不同,不能照抄 |
| 中高 | GPUI macOS 运行时行为 | 编译级已验证;纹理格式/key_char/IME/焦点语义待 P0 真机 spike |
| 中 | 权限点 | DND 全屏探针需 Screen Recording;system 动作触发 TCC 自动化弹窗——一律动作触发时请求,不在启动时预要 |
| 低 | 常规映射 | 剪贴板/托盘/自启/单实例/浏览器探测,标准 NS API 对等实现 |

## 三、能力映射速查

热键 Carbon RegisterEventHotKey(免权限)· 托盘 NSStatusItem · 单实例 NSDistributedNotificationCenter · 自启 SMAppService / LaunchAgents · 打开与定位 NSWorkspace / `open -R` · 文件监听 FSEvents(`notify` crate 接现有 200ms 合批/溢出重爬/退避重生模型)· 图标 .icns 解析 · 剪贴板 NSPasteboard · IME 无需解挂(macOS 后端成熟,§149 止血不移植)。

## 四、后续计划(零成本路径)

设备循环:Windows 本地写码 → CI macos job 四门禁 → 运行时差异记入"待真机验证清单" → 攒批租云 Mac(几元/小时)验证;决定长期投入再购二手 Mac mini。黑苹果/VM 不采纳(GPUI 依赖 Metal,VM 无 Metal)。

- **P0 真机 spike**(租一次即可):GPUI 最小窗口 / 中文 IME / 热键手感;产出 = 行为差异清单 + §154 记录
- **P1 宿主骨架**:sakana-macos crate(热键/托盘/单实例/自启/退出序),Alt+Space 唤起空壳;同步扩 check-arch.ps1 Allowed 名单与 CI 白名单
- **P2 低垂果实**:Web / Bookmark(改路径 + .app 探测)→ App(LaunchServices / Info.plist / icns / NSWorkspace;提权动作按 §126 能力缺失模式隐藏)
- **P3 File**:分隔符/盘符 cfg 化收敛到单点;FSEvents watcher;macOS 默认排除名单 = 37 个工具约定片段 + $HOME 展开组照搬 + 系统组换 macOS 版(/System、/Library、~/.Trash 等)
- **P4 System + 打磨**:系统动作(休眠按 §126 隐藏)+ TCC 引导 + DND 探针决策;打包 .app + DMG + codesign(公证需 Apple Developer 账号,公开分发再买)

CI 白名单扩容 = 每阶段显式动作(新 crate 默认 fail-closed,必须同步 check-arch);§78 性能预算在 macOS 按 §114 口径重测。

## 五、档案

- 门禁决策与实现:[architecture/records/153-macos-prep.md](architecture/records/153-macos-prep.md)(§153)
- 索引同步:architecture.md 速查表 · CLAUDE.md §110–111 / §147 相关段落
