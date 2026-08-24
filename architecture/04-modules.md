> CUE 架构规格分卷。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 26. 不建立 Universal Search Framework

V1 禁止提前设计：

```text
SearchDocument
UniversalRanker
CandidateProvider
UniversalIndex
SearchStrategy
RankingStrategy
```

除非真实实现中已经证明这些抽象有明确价值。

---

# 27. AppModule 搜索模型

AppModule 初始化时发现应用，并建立自己的搜索结构。

例如：

```text
永劫无间
```

可以预处理：

```text
永劫无间
yongjiewujian
yjwj
naraka
```

这些都属于：

```text
AppModule internal state
```

Core 不知道。

用户：

```text
yjwj
```

AppModule：

```text
fuzzy / exact
↓
永劫无间
```

---

# 28. AppModule Ranking

建议初始版本：

\[
Score =
StringMatch
+
UsageBonus
+
RecencyBonus
+
AliasBonus
\]

但具体公式属于 AppModule。

Core 不参与。

---

# 29. App Discovery

V1 推荐：

```text
User Start Menu
Common Start Menu
UWP / MSIX
```

后续：

```text
App Paths
PATH
custom sources
```

v0.2 确认 UWP/MSIX 实现路线（具体 API 路径）：

```text
PackageManager
→ FindPackagesForUserWithPackageTypes(...)
→ Package.GetAppListEntriesAsync()
→ AppListEntry
    ├─ AppUserModelId
    └─ DisplayInfo（名称、图标资源）

启动：
IApplicationActivationManager::ActivateApplication(AUMID)
```

实现注记（V1 落地）：实际调用是 `FindPackages()`——无参即当前
用户的全部包，语义等价且免去拼用户 SID；类型过滤对结果无实质
影响（没有 AppListEntry 的包在下一步自然跳过），简单路径胜出。

注意：**Package ≠ App**。一个 package 可含 0..n 个 application，枚举单位是 AppListEntry，不是 Package，也不是自己解析每个 manifest。

不采用：

```text
shell:AppsFolder 枚举
```

理由：AppsFolder 枚举省实现，但返回系统组件等脏数据，过滤规则脆弱；AppListEntry 语义精确。

不要一开始扫描系统所有 `.exe`。

---

# 30. App 去重

原则：

> 不判断两个入口是不是“同一个软件”。

只判断：

> 是否具有相同 launch semantics。

### Packaged Apps

优先：

```text
AUMID
```

### Win32

优先：

```text
canonical executable
+
normalized arguments
```

宁可重复，也不要 aggressive dedup。

---

# 31. FileModule

> **v0.1 修订：FileModule 整体延后出 V1。**
> ~~原因：对 Everything 第三方依赖的策略未最终确认。~~
> **v0.2 定案（已落地，实现记录见 §118）：依赖本机已安装并运行的
> Everything 1.4——Flow Launcher 同款策略。不自建索引(§76 不变)、
> 不随包分发 Everything、不链 Everything.dll;直连 WM_COPYDATA IPC。
> Everything 未运行 → 模块报 Unavailable,行内错误文案,优雅降级。**

FileModule 内部负责：

```text
query Everything
convert results
ranking
presentation
open
secondary actions
```

Core 不知道 Everything 的存在。

---

# 32. FileModule Result

业务对象可能：

```rust
struct FileEntry {
    path: PathBuf,
    is_dir: bool,
    size: Option<u64>,
    modified_at: Option<SystemTime>,
}
```

但只保存在 Module 内部。

Core 只持有：

```text
ItemId
```

---

# 33. 文件和文件夹

FileModule 同一个模态返回：

```text
files
folders
```

不单独设计：

```text
FolderModule
```

除非后续真实需求证明需要。

---

# 34. PageModule

不属于 V1 必须范围。

未来可以包含：

```text
browser history
bookmarks
tabs
known pages
```

但对 Core 来说仍然只是一个 LauncherModule。

---

# 35. Settings Architecture

Settings 分两类：

```text
Core Settings
Module Settings
```

---

