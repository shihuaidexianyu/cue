> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 131. 启动序列:热键尽早注册 + 事件 backlog

热键就绪曾是启动序列的最后一环(进程入口 → GPUI 初始化 →
Core 读盘 → host window → 注册,实测 ~110 ms):进程启动后立刻
按下的热键无处投递,被静默吞掉——开机自启遇上登录磁盘风暴时
这个窗口还会拉得更长。Flow Launcher 之类的常驻 launcher 同样
把"看起来秒开"押在"进程早已驻留"上;把热键可用点压到进程
入口后几毫秒,才是对"启动速度"诚实的回答。

## 调研结论(对标 Flow Launcher)

```text
前提证伪 = Flow 的"秒开"不是启动快,而是进程常驻(与 CUE 相同):
冷启动   = ≥3 s——Program 插件每次启动重建 UWP+Win32 索引,
           各 ~2 s(Flow.Launcher issue #1731)
首查     = 启动后第一次查询冻结 3–8 s 是挂号已知问题(WPF
           窗口初始化 + 索引未就绪,discussion #586)
结论     = 常驻型 launcher 的"启动速度"本质是两点:进程多快
           进入"按键可用"状态、早按的键丢不丢。CUE 的常驻
           与异步 catalog 已覆盖后者的大头,短板只剩热键
           注册排在启动序列末环——本节重排补掉
```

## 决议

```text
重排     = host window 与热键都是纯 Win32、不依赖 GPUI:
           单实例检查之后立即创建 host window 并注册热键
           (进程入口 → 热键就绪 ≈ 3 ms),GPUI 初始化在后
backlog  = Core 就位前到达的 HostMsg(热键/第二实例唤起)
           由 handler 暂存,Core::new 后原序补发——Win32
           线程消息队列 + 应用内 backlog 两级接力,早按不丢
热键值   = 早期注册用 env 覆盖 ?? 默认 Alt+Space;Core 读了
           settings.tsv 后再次 apply(相同早退,不同则事务式
           换绑;自定义用户在 ~100 ms 瞬态内默认键暂可用)
Send 约束 = HostWindow::create 的 handler 去掉 + Send:
           WndProc 有线程亲和,handler 只会被创建线程调用,
           可持 Rc<RefCell<...>> 单线程状态
release  = strip = "symbols" + lto = "thin" + codegen-units = 1
           (exe 12 MB → 8.4 MB;链接时间换加载/扫描/启动)
```

## 验证

```text
E2E = 真机:host window 出现后立刻 PostMessage WM_HOTKEY
      (~33 ms,GPUI 未就绪)——Launcher 在 ~171 ms 正常可见
      (backlog 补发生效);完全就绪后第二次热键 toggle 隐藏
      (正常路径无回归)
探针 = [boot] hotkey ready 3–5 ms / gpui entered 117–132 ms /
      core ready 121–137 ms / window created 139–159 ms
```
