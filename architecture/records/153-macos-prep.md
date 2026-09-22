> sakana 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 153. macOS 预备:双平台编译门禁 + 平台绑定纯净度扩展

用户无 macOS 设备,决定按零成本路径启动 macOS 预备:CI 即 Mac,
真机验证攒批。本记录只建门禁,不写平台代码——§110"第二平台动工
时再抽象"的动工信号,以门禁形式先行。

## 决策

```text
1. check-arch 平台纯净度从 windows 系扩为双平台绑定族(§111 对称化):
     源码模式     + std::os::unix、objc2、cocoa、core_foundation、
                   core_graphics、core_text、use objc、objc::
     Cargo.toml   锚定行首 crate 名 + [.\s=-]:windows(-sys)、
                   objc(2 家族)、cocoa、core-foundation(-sys)、
                   core-graphics、core-text、metal
     Allowed()    前缀匹配覆盖家族成员(objc2-app-kit、
                   core-foundation-sys 等),不逐一枚举
   守卫范围从 core/protocol 扩到 util-common——共享 util 平台中立
   (§72–73)首次机器化;ui 不进守卫:GPUI 后端允许合法持有
   cfg(target_os) 渲染差异代码(§111)。raw extern FFI 不查,
   与 windows 侧同水位——护栏是 tripwire,不是证明。
   模式只用字面交替(git grep -E POSIX 回退,无 \b)。
2. CI 新增 macos job(macos-latest):对平台中性层
   core/protocol/util-common/ui 跑同一套四门禁。验证集是显式
   -p 白名单——加入名单是适配推进的显式动作,不是自动获得;
   宿主层与 Windows 耦合模块待各自平台化后再加(§110 隔离原则)。
   check-arch 在 macOS 用 pwsh 跑(runner 默认 shell 是 bash)。
3. Cargo 零改动:gpui 0.2.2 的 windows-manifest 是空 feature(=[]),
   macOS 上无副作用;其 macOS 后端(cocoa/metal/objc2)本就是
   Zed 的主平台。
4. 分发/真机策略(判断记录):黑苹果/VM 不采纳(GPUI 依赖 Metal,
   VM 无 Metal,且绕 EULA);真机验证攒批走云 Mac 按小时租;
   正式公证(Apple Developer 账号)只在公开分发时考虑。
```

## 行为影响

```text
新增   macos runner 上平台中性层的编译/测试/守卫门禁
新增   core/protocol/util-common 的 macOS 绑定 crate 禁入
不变   app 行为零变化(无 Rust 源码改动);Windows 门禁原样
```

## 实现落点

```text
scripts/check-arch.ps1   平台纯净模式与 Allowed() 扩展 + 回归断言;
                          BOM 失而复得:编辑器剥 BOM → PS 5.1 按
                          ANSI 解中文注释吞换行 → 语法错乱(项目
                          教训第二次,入库前需校验头三字节)
.github/workflows/ci.yml macos job + 两个 job 的 arch guard 显式
                          shell: pwsh
```

## 验证

```text
本地 = check-arch 双引擎(PS 5.1 + pwsh7)通过;合成违规负向测试
     三条(源码模式命中 macOS 绑定、TOML 模式命中依赖声明、正常
     清单零误伤);fmt
CI   = windows job 原样全绿(1m37s);macos job 首跑全绿(4m06s):
     clippy 2m02s,73 项测试 0 失败(core 13+48 / protocol 6 /
     util-common 3 / ui 3),arch guard pwsh 通过
发现   gpui 0.2.2 在 macos-latest(aarch64)原生编译通过——§110
     "ui 原样可移植"的编译级验证完成;渲染/IME 运行时行为仍待
     P0 真机 spike
```
