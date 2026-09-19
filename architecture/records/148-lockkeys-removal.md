> sakana 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 148. 锁键服务整体裁撤(含 OSD 窗口)

§140 从 WinCaps(MIT)移植的锁键服务,§144 已把手势/拦截/注入
裁到只剩"纯观察的锁定状态 OSD"。本记录把剩下的也删了:**服务
整体下线**——WH_KEYBOARD_LL 钩子、按下沿账本、OSD 卡片窗口、
`core.lockkeys.*` 设置行、`NotifyLockKeys` 回调、`HostMsg::
SessionReset` 复位链路全部移除。

## 为什么删

1. **与产品定位无关**:锁键提示不在 launcher 的高频操作路径上
   (§1:统一输入界面执行高频操作),它是 WinCaps 移植带来的
   附带功能——从手势裁到纯观察再裁到零,轨迹本身就说明它不
   属于这个产品。
2. **常驻成本与功能不成比例**:WH_KEYBOARD_LL 是系统级低级钩子,
   全系统每一次按键都要经过我们的回调——为一张"CapsLock 开/关"
   卡片在整机的按键路径上支付常驻开销。
3. **平台耦合重**:第二个 GPUI 窗口(PopUp + Transparent)、
   排除法 hwnd 发现、`WM_POWERBROADCAST`/`WM_WTSSESSION` 的
   SessionReset 复位,都是这项功能独有的宿主面;macOS 移植评估
   里它也是首个"价值重估"对象。

## 裁撤清单

```text
删除整文件  sakana-windows/src/lockkeys.rs(LL 钩子 worker + 账本)
            sakana-protocol/src/lockkeys.rs(LockKey / LockKeysConfig)
            sakana-ui/src/osd.rs(OsdView,第二个 GPUI 窗口)
摘除接线    main.rs:worker 启动、OsdMsg 泵、OSD 窗口与排除法
            hwnd 发现、ShowLauncher 时收起 OSD、notify_lockkeys
            host.rs:WM_SAKANA_LOCKKEY、WM_POWERBROADCAST 分支、
            HostMsg::{LockKeyChanged, SessionReset}
            core:core.lockkeys.* 两行、LOCKKEYS_PREFIX、
            notify_lockkeys 字段与初始/commit 通知
死代码      window::find_window_hwnd_excluding、monitor::
            place_centered_on_active_monitor(均因 OSD 而生)
保留        WM_WTSSESSION 锁屏 → FocusLost(§115 不变量中与
            锁键无关的另一半);display-change guard(launcher
            仍在用);IME 强制英文(§107,输入框服务)
```

## 用户侧影响

settings.tsv 里的 `core.lockkeys.*` 行(§144 遗留四行 + enabled/
osd 两行):无对应 spec 的行加载时忽略,下次整体重写自愈消失
——与 §144 裁手势行、§127 更名 game_mode 同款机制,零迁移。

## 验证

```text
单测 = 陈旧锁键行被忽略、其余行照常加载(改写 §144 同名测试;
     用 enabled/osd/hold_ms 三行做坏料)
回归 = fmt / clippy -D warnings / cargo test --workspace /
     check-arch 四门禁全绿;全仓 grep 无 lockkeys/OSD 残留
```
