> sakana 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 149. IME 强制英文整体裁撤

§107 的产品决定「Launcher 输入框强制英文输入」(唤起时
`ActivateKeyboardLayout` 切 en-US + `ImmAssociateContext(hwnd, NULL)`
阻止 IME 挂靠,hide 前恢复记录的用户布局)**整体下线**:
`sakana-windows/src/ime.rs`、Show/Hide/Quit 三处效果接线与 §142
退出兜底里的恢复调用全部移除,`Win32_UI_Input_Ime` feature 随删。

## 为什么删

1. **成对契约天生脆弱**:布局记录必须在抢到前台之前、恢复必须在
   仍处前台时——失焦隐藏路径恢复"不保证生效"是记录在案的固有
   边界(§107),为它还挂过 `[ime]` 探针日志等数据。整段 Win32
   集成的全部复杂度,服务于"把一个开放式风险换成一段确定集成"
   的权衡;而该权衡的另一边(拼音已是设计内一等输入路径)今天
   依然成立。
2. **从外部操作 HWND 的 IMM 上下文与 GPUI 内部状态天生不同步**
   (§107 spike 当年就要验证这一点):每次 GPUI 升级这块集成都要
   重新验证,是平台耦合重灾区,macOS 移植清单上同样要重估。
3. **键盘主权交还用户**:不强制、不切换、不恢复——用户处于什么
   输入法状态,launcher 就用什么状态,与 Spotlight / Raycast 的
   行为一致。

## 行为影响(诚实记录)

```text
保留    英文模式下的全部体验:拼音全拼/首字母搜索(§27)、触发词
        b / / > g、英文直输——与 v0.6.1 完全一致
变化    中文 IME 激活时唤起:不再被强制切英文。此时
        a) 键入的字母进入组合而非直输,触发词与拼音搜索需要
           用户自己切回英文(与其它启动器一致);
        b) 组合上屏的汉字能否经 GPUI 按键路径进入搜索框
           未经验证(§107 当年正是为此回避),粘贴 Unicode
           仍是可靠路径(§115)
不变    粘贴直达 URL、设置页、动作菜单等一切其余功能
```

即:英文用户零感知;中文用户获得"不被打断"的 IME 状态,代价是
搜索时自己管理输入法(或用粘贴)。

## 用户侧影响

零迁移:本功能无设置行、无持久化状态(SAVED_LAYOUT 是进程内
静态量)。删除即消失。

## 验证

```text
回归 = fmt / clippy -D warnings / cargo test --workspace /
     check-arch 四门禁全绿;全仓 grep 无 ime::/ImmAssociateContext
     /ActivateKeyboardLayout 残留(hotkey.rs 的 VK_/MOD_ 常量除外,
     属 Win32_UI_Input_KeyboardAndMouse,热键仍用)
```
