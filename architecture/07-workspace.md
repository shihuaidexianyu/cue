> CUE 架构规格分卷。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 67. Future Compatibility

V1 不设计插件 API。

但保持一个边界：

```text
Core
↓
Module trait
↓
Built-in modules
```

将来如果需要第三方 Module：

```text
可以重新设计实现方式。
```

不要求 V1 Trait 成为永久 ABI。

这一点非常重要：

> Rust trait 是当前内部架构接口，不是第三方 ABI contract。

---

# 68. 推荐 Workspace

v0.2 修订：crate 统一 `cue-` 前缀。原名 `core` / `windows` 分别与 Rust 内置 `core` 和官方 `windows` crate（Win32 binding）撞名——本地 path dependency 与 registry 包同名会造成引用与别名混乱。Rust import 名相应为 `cue_core` / `cue_protocol` / `cue_ui` / `cue_windows` / `cue_module_app`。

```text
cue/
│
├── Cargo.toml
│
└── crates/
    │
    ├── cue/                    # binary,composition root
    │   └── main.rs
    │
    ├── cue-core/
    │   ├── session.rs
    │   ├── registry.rs
    │   ├── routing.rs
    │   ├── result.rs
    │   ├── settings.rs
    │   └── usage.rs
    │
    ├── cue-protocol/
    │   ├── module.rs
    │   ├── launcher_module.rs
    │   ├── presentation.rs
    │   ├── action.rs
    │   ├── outcome.rs
    │   └── settings.rs
    │
    ├── cue-ui/
    │   ├── app.rs
    │   ├── launcher_window.rs
    │   ├── input.rs
    │   ├── result_list.rs
    │   ├── result_row.rs
    │   └── settings.rs
    │
    ├── cue-windows/
    │   ├── hotkey.rs
    │   ├── activation.rs
    │   ├── shell.rs
    │   ├── startup.rs
    │   ├── single_instance.rs
    │   └── monitor.rs
    │
    ├── cue-module-app/
    │   ├── lib.rs
    │   ├── discovery.rs
    │   ├── shortcut.rs
    │   ├── packaged.rs
    │   ├── dedup.rs
    │   ├── search.rs
    │   ├── ranking.rs
    │   ├── icon.rs
    │   └── launch.rs
    │
    ├── cue-module-file/        # ~~V1.x，暂不创建（§31）~~ 已落地（§118）
    │   ├── lib.rs
    │   ├── everything.rs
    │   ├── search.rs
    │   ├── presentation.rs
    │   └── open.rs
    │
    ├── cue-module-bookmark/    # V1.x 已落地（§117）
    ├── cue-module-system/      # V1.x 已落地（§126）
    └── cue-util-win/           # 模块间共享 Win32 助手（§72–73 下沉）
```

---

# 69. 不创建的 crate

当前明确不要：

```text
plugin-sdk
plugin-runtime
module-sandbox
wasm-host
search-framework
universal-index
universal-ranking
capability-runtime
dynamic-loader
```

---

# 70. Composition Root

只有最终 executable 知道有哪些具体 Module。

例如：

```rust
fn compose_app() -> Launcher {
    let mut registry = ModuleRegistry::new();

    registry.register(
        Box::new(AppModule::new())
    );

    // V1.x 的 Bookmark / File / System 模块同样只在此注册

    Launcher::new(registry)
}
```

`cue-core` crate 不应：

```rust
use cue_module_app::AppModule;
```

具体模块只由顶层 composition root 引入。

---

# 71. 依赖方向

正确：

```text
cue (binary)
├── cue-core
├── cue-ui
├── cue-windows
├── cue-module-app
├── cue-module-bookmark (V1.x)
├── cue-module-file (V1.x)
└── cue-module-system (V1.x)

cue-module-app
├── cue-protocol
└── cue-util-win

cue-module-bookmark / cue-module-file
├── cue-protocol
└── cue-util-win

cue-module-system
└── cue-protocol        # 目前不需要 cue-util-win,按需再加

cue-util-win
└── cue-protocol

cue-core
└── cue-protocol

cue-ui
├── cue-core
└── cue-protocol
```

错误：

```text
cue-core
→ cue-module-app
```

Core 不允许依赖具体 Module。

---

# 72. 什么时候抽公共库

采用：

> Rule of Three

第一次：

```text
直接写
```

第二次：

```text
允许重复
```

第三次：

```text
确认语义真的相同以后再抽
```

例如：

```text
AppModule
PageModule
CommandModule
```

都需要：

```text
pinyin_initials()
```

才考虑：

```text
text-utils
```

---

# 73. 抽取方向

共享实现只能：

```text
向下沉淀
```

例如：

```text
App ───┐
       ├── text-utils
Page ──┘
```

不要：

```text
App
Page
 ↓
Core 增加拼音 API
```

Core 不应成为杂物库。

---

