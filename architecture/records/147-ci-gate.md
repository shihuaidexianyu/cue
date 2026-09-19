> sakana 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 147. CI 门禁:fmt / clippy / test / check-arch

CLAUDE.md 记录在案的「仓库尚无 CI,check-arch 建议 pre-push 跑」
把全部架构不变量寄托在自觉上:一条 `use windows::` 进 sakana-core
就能击穿 §110 平台纯净度,且没人发现。补最小门禁
(`.github/workflows/ci.yml`,windows-latest + stable 工具链):

```text
push(main) / pull_request →
  1. cargo fmt --all -- --check        格式
  2. cargo clippy --all-targets -- -D warnings   lint 零警告
  3. cargo test --workspace            全部测试(含 #[ignore] 之外
     的真实 Win32 测试;#[ignore] 的手动项照旧不进 CI)
  4. scripts/check-arch.ps1            依赖方向 + 平台纯净度 +
     composition root(rg 缺席退回 git grep,§142)
```

设计取舍:

- **stable 工具链**,不锁版本:本仓库无 MSRV 承诺(桌面应用,
  源码构建),stable 漂移带来的新 lint 按 -D 当场修,不做
  toolchain pin 的假稳定。
- **-D warnings 从第一天就全绿**:基线验证于 rustc 1.98.1
  (fmt/clippy/test/check-arch 四项退出码 0),没有 grandfather。
- **不建 release 流水线**:分发仍走本地 scripts/package.ps1
  (Inno + 自签),CI 只守质量,不碰产物。

## 验证

```text
本地等价命令四连跑退出码全 0(1.98.1);workflow 于本 PR 首次
push 后在 Actions 页可见运行结果。
```
