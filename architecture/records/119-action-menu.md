> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 119. 次级动作菜单(Action Menu,V1.x)

§18 Action Model 的 UI 落地;§75 三项(Run as administrator /
Open containing folder / secondary actions UI)随此解封。

## 交互

```text
打开   = Tab(有选中项且模块给出非空动作集时)
菜单键 = ↑↓ 选择 · Enter 执行选中动作 · Esc/Tab 关闭
模态   = 其余按键:关菜单并吞掉(不落进搜索输入)
Esc 分层 = 菜单开着 → 关菜单(结果列表原样恢复);菜单关着 → 关会话
头部   = "动作 · <选中项标题>"(打开时 present 快照)
不做   = 每动作快捷键(§59 维持"后续再定义")、菜单内搜索、鼠标
```

## Core(§5 状态机的第三个子状态,同 §41 设置页模式)

```text
SessionState.action_menu = Option<{ actions 快照, selected, item_title }>
open   = Tab 时对选中项同步调 Module::actions(廉价,一次性快照)
激活   = activate_selected 泛化为 activate_with(ActionId);
         菜单 Enter 先关菜单再以选中 ActionId 激活——失败时
         用户看到结果列表 + 错误横幅(§115)
失效   = 输入变化(§102)或新结果提交 → 关菜单
         (动作快照属于旧选中项,不打到新结果上)
usage  = 按各自动作 ActionId 记账(§50 键本含 ActionId;
         "复制路径" 不污染 PRIMARY 的启动频率)
UI 模型 = action_menu_model()(label + 预格式化 shortcut + selected);
         UI 只渲染,键盘路由按 in_action_menu() 分流
```

## 平台原语(cue-util-win 新住户)

```text
shell_execute_elevated = ShellExecuteEx lpVerb="runas"(UAC;
                        用户取消 = ERROR_CANCELLED,按普通失败展示)
reveal_in_explorer     = explorer.exe /select,"<path>"(文件/夹通用;
                        不引 COM/PIDL——一行动词,Failure 面最小)
clipboard::set_text    = OpenClipboard → EmptyClipboard →
                        SetClipboardData(CF_UNICODETEXT) 单次写入
                        ——是 §18 的 Copy path/link,不是 §76 的
                        clipboard manager(无历史、无监听)
```

## 模块动作集(§18;label 中文,顺序即菜单顺序)

```text
App      = 打开 / 以管理员身份运行 / 打开所在位置
           (后两个仅 Win32 目标:packaged 由系统代理激活,
           没有可提权/可定位的 exe,actions() 不声明)
File     = 打开 / 打开所在文件夹 / 复制路径
Bookmark = 打开 / 复制链接
未知 ActionId = ActivationFailed(明确报错,不静默降级,§63)
```

## 验证

```text
单测 = Core 5 例(打开/导航钳制/ActionId 路由/PRIMARY 不变/§102 关菜单)、
       三模块动作集与顺序、packaged 拒绝次级动作、
       剪贴板真机回环(CF_UNICODETEXT 写入再读回)
E2E  = 真机:Tab 出菜单(截图:文件 3 行/书签 2 行,头部归属正确),
       ↓↓ Enter 复制路径 → 剪贴板 = C:\Windows\explorer.exe,
       会话关闭;Esc 分层(先关菜单回结果页,再 Esc 关会话);
       usage.tsv 落 (file, action_id=2, 全路径)
```

---

