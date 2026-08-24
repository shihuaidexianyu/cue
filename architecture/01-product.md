> CUE 架构规格分卷。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 1. 产品定义

Launcher 是一个：

> **通过统一输入界面，快速进入不同功能模态，并执行用户高频操作的轻量 Windows 工具。**

Launcher 本身不追求成为万能命令中心。

第一阶段的核心任务只有：

1. 快速启动应用
2. 快速打开文件 / 文件夹

未来可以增加：

3. Page / Browser 内容
4. 其他新的内置 Module

但新增功能必须满足明确的高频需求，不因“架构支持”而自动加入产品。

---

# 2. 产品核心交互

Launcher 默认通过：

```text
Alt + Space
```

唤醒。

默认进入：

```text
App Module
```

例如：

```text
Alt + Space
zed
Enter
```

启动：

```text
Zed
```

文件使用显式模态前缀：

```text
Alt + Space
/paper
Enter
```

打开：

```text
paper.pdf
```

当前建议：

```text
无前缀    App Module
/         File Module
```

未来：

```text
@         Page Module
```

但 V1 不要求实现 Page。

---

