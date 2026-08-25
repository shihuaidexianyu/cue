# CUE — Product & Architecture Specification v0.2

## 0. 文档状态

**目标平台:** Windows
**技术栈:** Rust + GPUI + Windows API
**当前阶段:** V1 完成 + V1.x 模块(书签 / 文件 / 系统动作 / 动作菜单)持续迭代
**核心原则:** 克制、低延迟、模块化、不过度抽象

所有 Module 在当前版本中均为**可信内置 Rust Module,并静态编译进入 Launcher**。

明确不讨论:

- 第三方插件
- 动态模块加载
- DLL ABI
- WASM
- Module sandbox
- 插件市场
- 第三方权限系统

## 0.1 文档组织(2026-08-24 起拆分)

正文按主题拆为 `architecture/` 目录下的多个分卷,本文件是总索引。

**§ 编号是全文档的稳定地址,全局唯一、跨文件有效、只增不减:**

- 引用一律写 `§NN`(代码注释、CLAUDE.md、提交信息同样适用),不带文件名;
- 拆分、移动、修订章节内容时**编号不变**;
- 新增章节取当前最大编号 + 1,放入对应分卷(V1.x 实现记录一律新文件);
- 废除的章节保留编号与标题,正文标注废止原因与替代 §。

## 0.2 文件地图

| § 范围 | 文件 | 内容 |
|---|---|---|
| §1–§2 | [architecture/01-product.md](architecture/01-product.md) | 产品定义、核心交互 |
| §3–§12 | [architecture/02-core.md](architecture/02-core.md) | 架构原则(§3)、顶层架构、Core 职责、Module trait、ModuleItem、Result 禁区 |
| §13–§24 | [architecture/03-presentation.md](architecture/03-presentation.md) | 结果呈现(icon/badge/row)、Action 模型、ModuleOutcome、Core↔Module 交互 |
| §25–§34 | [architecture/04-modules.md](architecture/04-modules.md) | 模糊搜索位置、禁 Universal Search、App 发现/去重、File 决策、PageModule |
| §35–§48 | [architecture/05-settings-storage.md](architecture/05-settings-storage.md) | 设置架构(schema/事务/UI)、模块存储三桶 |
| §49–§66 | [architecture/06-runtime.md](architecture/06-runtime.md) | ModuleContext、Usage、热键、窗口行为、GPUI UI、键盘、错误、日志、模块启停 |
| §67–§73 | [architecture/07-workspace.md](architecture/07-workspace.md) | crate 布局、禁区 crate、Composition Root、依赖方向、Rule of Three |
| §74–§88 | [architecture/08-scope-contract.md](architecture/08-scope-contract.md) | V1 范围、非功能需求、最终边界、**§86 最终 Contract**、判断规则、实现顺序 |
| §89–§106 | [architecture/09-async.md](architecture/09-async.md) | V1 成功标准、设计哲学、异步任务模型(QueryTicket 北极星) |
| §107–§116 | [architecture/10-v1-landing.md](architecture/10-v1-landing.md) | V1 落地决策:IME、Row 布局、Module 事件、跨平台、CoreEffect、单实例、性能契约、UX 不变量、托盘 |
| §117–§132 | [architecture/records/](architecture/records/) | V1.x 实现记录,**每章一文件**,文件名即 § 编号 |

### records/ 速查

| § | 文件 | 主题 |
|---|---|---|
| §117 | [117-bookmark.md](architecture/records/117-bookmark.md) | BookmarkModule(Chromium 书签) |
| §118 | [118-file-module.md](architecture/records/118-file-module.md) | FileModule 实现记录(Everything) |
| §119 | [119-action-menu.md](architecture/records/119-action-menu.md) | 次级动作菜单(Tab) |
| §120 | [120-noise-exclusion.md](architecture/records/120-noise-exclusion.md) | FileModule 噪声目录排除 |
| §121 | [121-editable-exclusion-list.md](architecture/records/121-editable-exclusion-list.md) | 可编辑名单(String 设置) |
| §122 | [122-exclusion-list-file.md](architecture/records/122-exclusion-list-file.md) | 名单外置:模块数据文件(Path 设置) |
| §123 | [123-toml-config.md](architecture/records/123-toml-config.md) | 手编辑配置文件一律 TOML |
| §124 | [124-file-icons.md](architecture/records/124-file-icons.md) | 文件结果真实图标(异步提取) |
| §125 | [125-exclusion-generalized.md](architecture/records/125-exclusion-generalized.md) | 排除名单通用化(`\AppData\` 锚定) |
| §126 | [126-system-module.md](architecture/records/126-system-module.md) | 系统动作模块(触发词 `>`) |
| §127 | [127-dnd-mode.md](architecture/records/127-dnd-mode.md) | 免打扰模式(全屏不唤起) |
| §128 | [128-custom-triggers.md](architecture/records/128-custom-triggers.md) | 触发词自定义 |
| §129 | [129-settings-page.md](architecture/records/129-settings-page.md) | 设置页呈现(单行列表 + 详情条) |
| §130 | [130-dpi-wakeup.md](architecture/records/130-dpi-wakeup.md) | 跨 DPI 唤起双重缩放复盘 |
| §131 | [131-startup-hotkey.md](architecture/records/131-startup-hotkey.md) | 启动序列:热键尽早注册 + 事件 backlog |
| §132 | [132-logging.md](architecture/records/132-logging.md) | 诊断日志(全局 sink、有界单文件、写线程) |

## 0.3 阅读顺序

1. 新读者:§1–§3(产品定义与第一原则)→ §86(最终 Contract)→ §87(判断规则)。
2. 改 Core/协议:02、03、09;改设置或存储:05;加模块:04 + 06 + records/ 里同类模块的记录。
3. 历史决策按 § 号查 records/ 与 10-v1-landing;性能基线在 §114(10-v1-landing)与各记录的「验证」小节。
