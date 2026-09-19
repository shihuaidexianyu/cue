> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 143. WebModule(触发词 `g`)+ 默认路径「打开链接」

两个诉求一次落地:**粘贴链接直达网页**与**调用搜索引擎搜索**。
前者是零按键流程(`Alt+Space` → `Ctrl+V` → `Enter`),走默认路径;
后者是独立模态,走触发词 `g`。本记录同时修订 §76 的
「Network search」条目。

## 决议

```text
§76 收窄 = 「network search」收窄为「在 launcher 内获取网络搜索
           结果」——仍然禁止;「把查询/URL 委派给系统浏览器」是
           合法形态。sakana 进程零网络 IO,浏览器才是搜索客户端
§82/§83 不动 = 无 prefix 仍是 App;「打开链接」行是 AppModule 自
           有 payload 的伪结果,模态隔离不破
搜索形态 = WebModule 固定两对组合:必应+Edge / Google+Chrome。
           `g <文本>` 出 ≤2 行(每行一对组合),↑↓ 选择;
           usage 把常用组合顶到第 0 行——没有「默认引擎」设置
单一职责 = `g` 输入一律按搜索词处理,即使形似 URL(规则唯一、
           可预测);`g`+空 → 无结果(无可枚举目录,不学 §126
           空查询列全部)
能力门控 = 查询时 Browser::exe_path() 探测(§126 休眠同款,
           微秒级、每次查询现探,缓存会错过中途安装);
           组合缺失不出行;双缺失 → 单行「未检测到 Edge 或
           Chrome」,激活时报该错(不静默空白)
usage    = 固定 key(web:search:bing / web:search:google),
           公式复制自 app/bookmark;行序 = usage 加分降序,
           稳定排序保底表序(必应在前)
图标     = 真浏览器图标(exe 提取,§117 bookmark 同法,load
           线程一次性提取 + PresentationInvalidated),未就绪
           Search 字形 0xE721 兜底(§135)
打开链接 = AppModule 伪结果行:url_normalize(输入) 通过则钉顶
           (ItemId(u64::MAX) 哨兵,catalog 序号永不冲突);
           空查询不出现;动作 = 打开(默认浏览器)/用 Edge 打开/
           用 Chrome 打开(探测到才出现)/复制链接;
           usage 固定 key open-url;行图标 Link 字形 0xE71B
url_normalize = scheme 白名单 http/https(大小写不敏感);
           无 scheme 需含 '.'(裸域名补 https://)+ IP 字面量
           与 localhost(:port 可用);尾部粘贴噪声修剪
           (引号/中文句号逗号/成对右括号等);含空格与其余
           scheme 一律拒绝
           ——绝不让任意 scheme 到达 ShellExecute
编码     = 搜索词 UTF-8 form-url 编码(unreserved 保留、
           空格→+);打开的 URL 不编码(ShellExecuteW 直吃
           Unicode,浏览器处理 IRI)。手写 ~20 行,不引 url crate
共享下沉 = bookmark 私有的 chromium::Browser 出现第三、四使用处
           (web / app / bookmark 自身两处)→ 整族下沉
           sakana-util-win::browser:Browser 枚举 + exe_path
           (env 候选 + is_file,不引注册表)+ open_url(exe 缺失
           退回默认浏览器,「宁可降级」沿用 §117)+ load_icons
           (调用方负责 COM 线程)
设置     = 除 §128 合成的 module.web.trigger 外零设置;
           之前的 search_engine Enum 方案作废(usage 置顶
           取代默认值)
```

## 明确不做

```text
默认路径的搜索兜底行(每个应用查询挂一行搜索,污染 §82,§81.D 答不过)
自定义引擎模板字符串(两对固定表;真需要时再议)
剪贴板监听 / 唤起自动回填(§76 clipboard manager 红线;用户手动 Ctrl+V,
  §115 已保证 Unicode 粘贴)
launcher 内获取搜索结果(§76 仍然有效)
浏览器历史搜索、多引擎关键字(`g`/`b` 单触发词单语义)
```

## 验证

```text
单测 = form-url 编码(ASCII/中文/空格/保留字符)、search 全组合
       (可用性门控/usage 重排/双缺失行/空查询/item id 稳定)、
       present/actions 形状、url_normalize 全案(scheme 变体/
       裸域名/IP:port/localhost/尾部标点/含空格/javascript: 拒绝)、
       AppModule 钉顶 + result_limit 预算
E2E  = 粘贴 https:// 链接 → 钉顶行 → Enter 默认浏览器打开;
       `g rust tutorial` 双行 → Enter 进常用组合浏览器;
       Tab 动作菜单(用 Edge/Chrome 打开、复制链接);
       常用组合随 usage 浮到第 0 行
```

---
