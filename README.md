<div align="center">
  <img src="assets/cue.svg" alt="CUE" width="96" height="96">
  <h1>CUE</h1>
  <p><b>轻量 Windows 启动器</b> —— Alt+Space 唤起,输入即搜,Enter 启动</p>
  <p>Rust + GPUI + Win32 · 单文件约 12 MB · 常驻内存约 63 MB</p>
</div>


## 功能

- **应用搜索**:开始菜单 + 商店应用(UWP/MSIX)全量索引;中文应用按拼音全拼 / 首字母搜(`yx` → 邮箱)
- **书签搜索**:`b` + 空格进入书签模式(词边界触发,输 `baidu` 不会误入),搜 Edge / Chrome 书签——含 Chrome **账号同步书签**;行内标注来源浏览器,回车**在哪个浏览器收藏,就在哪个浏览器打开**
- **文件搜索**:`/` 进入文件模式,经本机 [Everything](https://www.voidtools.com/) 1.4 秒搜全盘文件与文件夹(原生语法 `ext:`、路径过滤等原样透传);默认排除系统目录与"工具内脏"(Windows、Program Files、ProgramData、任意用户的 AppData、node_modules、.git、包缓存等)——结果里只有你的工作文件;名单是 `%LOCALAPPDATA%\CUE\modules\file\data\excluded-paths.toml`(TOML 数组,每行一个片段),设置页选中「排除名单文件」回车即用默认编辑器打开,保存后下一次查询生效,「排除噪声路径」总开关可一键整体关停过滤;输入含 `\` 的显式路径时不过滤;Everything 未运行时行内提示、不影响其余功能
- **真实文件图标**:文件结果按内容取图标——exe / lnk 等取文件内嵌图标(每个文件一枚),其余按扩展名共享类型图标;后台线程异步提取,就绪后自动重画,不阻塞输入;文件夹与无扩展名文件用通用图标
- **系统动作**:`>` 进入系统动作——锁屏 / 睡眠 / 休眠 / 注销 / 重启 / 关机 / 清空回收站,拼音、首字母、英文都能搜(`gj` → 关机);重启 / 关机给 30 秒原生倒计时(`shutdown /a` 可取消,应用可拒绝),其余立即执行;机器未启用休眠时"休眠"自动隐藏
- **动作菜单**:结果上按 `Tab` 展开次级动作——应用:以管理员身份运行 / 打开所在位置;文件:打开所在文件夹 / 复制路径;书签:复制链接(`↑↓` 选择,`Enter` 执行,`Esc` 返回)
- **越用越顺手**:空输入显示最常用的应用与书签;频率 + 最近使用加权排序
- **不打扰**:平时只有托盘图标;单实例;失焦自动隐藏;**游戏模式**——前台全屏(游戏、全屏视频)时热键静默失效,不打断沉浸(可在设置中关闭)
- **可设置**(托盘右键 → 设置):唤起热键、失焦隐藏、开机自启、游戏模式、**各模块触发词**(`b`/`/`/`>` 可改成你喜欢的词)——事务式生效,改失败不留半成品

## 安装

从 [Releases](https://github.com/shihuaidexianyu/cue/releases) 下载 `CUE-Setup-x.y.z.exe`,双击安装:每用户安装、**免管理员**、简体中文向导,卸载走系统"应用和功能"(设置与使用统计 `%LOCALAPPDATA%\CUE\` 默认保留)。

文件搜索(`/` 模式)依赖本机已安装并运行的 [Everything 1.4](https://www.voidtools.com/);不装也能用,只是 `/` 模式会提示不可用。

### 从源码构建

```powershell
cargo build --release          # 产出 target\release\cue.exe
scripts\package.ps1            # 编译安装包 dist\CUE-Setup-x.y.z.exe(需 Inno Setup 6)
```

## 使用

| 操作 | 按键 |
|---|---|
| 唤起 / 隐藏 | `Alt + Space`(可在设置中修改) |
| 搜应用 | 直接输入(英文 / 拼音) |
| 搜书签 | `b` + 空格,再输入关键词 |
| 搜文件 | `/`,再输入关键词(需本机运行 Everything 1.4) |
| 系统动作 | `>`,再输入动作名(拼音 / 英文) |
| 选择 | `↑` `↓` |
| 启动(主动作) | `Enter` |
| 动作菜单(次级动作) | `Tab` 打开,`↑↓` 选择,`Enter` 执行,`Esc`/`Tab` 返回 |
| 隐藏 | `Esc` |
| 显示 / 设置 / 退出 | 托盘图标右键 |

设置与使用统计保存在 `%LOCALAPPDATA%\CUE\`(`settings.tsv` / `usage.tsv`),卸载时默认保留。

## 性能

Release 构建,Windows 11 实测(架构规格 §114):

| 指标 | 预算 | 实测 |
|---|---|---|
| 冷启动(进程入口 → 热键可用) | < 500 ms | 113–125 ms |
| 唤起延迟(热键 → 可输入) | < 100 ms | 92–98 ms(E2E 含注入开销);渲染预热后首唤 22 ms、稳态 15 ms |
| 应用搜索(输入 → 结果) | P50 < 5 ms / P95 < 15 ms | P50 0.38 ms / P95 0.52 ms |
| 常驻内存(空闲 60 s) | < 100 MB | 63.1 MB |

## 架构

一条铁律:**Core 管功能怎么跑,Module 管功能怎么干**。Core 是薄宿主(会话、输入路由、查询生命周期、设置、usage),不含任何业务语义;应用、书签等业务全在模块里,经 owned opaque `ModuleItem` 交给 Core,互不感知。

```text
crates/
├── cue                    编排器:host/UI 事件 → Core 状态 → 平台效果
├── cue-core               宿主运行时(无平台代码,无业务语义)
├── cue-protocol           模块协议(ModuleItem / 结果展示 / 激活结果)
├── cue-ui                 GPUI 界面
├── cue-windows            Win32 宿主(热键 / 托盘 / 单实例 / 窗口)
├── cue-util-win           模块共享 Win32 助手(COM / 图标 / ShellExecute / 剪贴板)
├── cue-module-app         应用搜索(默认模块,无触发词)
├── cue-module-bookmark    书签搜索(触发词 b)
├── cue-module-file        文件搜索(触发词 /,Everything IPC)
└── cue-module-system      系统动作(触发词 >,固定枚举)
```

完整产品 & 架构规格见 [architecture.md](architecture.md)(中文,正文 § 编号为权威引用)。

## 开发

```powershell
cargo test                          # 全部测试
cargo test -p cue-module-app        # 单个 crate
cargo clippy --all-targets          # lint
scripts\icon.ps1                    # 重生成品牌图标(assets/cue.svg → cue.ico)
```

注意:运行中的 `cue.exe` 会锁定二进制,重新构建前先 `Stop-Process -Name cue`。
