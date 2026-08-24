> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 117. BookmarkModule（Chromium 书签,V1.x)

浏览器书签搜索。设计参照 Flow Launcher 的 BrowserBookmark 插件,
按 CUE 架构约束裁剪。

## 范围与触发词

```text
module id  = bookmark
trigger    = b（词边界规则见 §5.2:b<空白> 或裸 b 才命中,不吞 baidu)
范围       = Chromium 系(Edge / Chrome),含多 profile
明确不做   = Firefox(places.sqlite 带锁,需拷贝解锁 + SQL——
             不值得为它引入 rusqlite 依赖;本机亦无 Firefox 生态)
```

## 数据源与刷新

```text
数据源 = <User Data>/<profile>/{Bookmarks, AccountBookmarks}
         (JSON,无锁,浏览器运行中可读;Bookmarks = 本地书签,
         AccountBookmarks = 账号书签——登录 Google 账号后 Chrome
         只写后者、本地 Bookmarks 停更,两者同构、可并存,都收)
发现   = 枚举 User Data 下所有 profile 目录的这两个文件
解析   = 递归 roots;凡带 children 数组的对象即容器(folder /
         bookmark_bar / other / synced / workspace / custom_root
         一视同仁),type == "url" 才是书签;坏 JSON / 缺字段跳过(§63)
刷新   = 每次 query 在模块后台 future 里重跑发现 + stat,
         (路径, mtime, 长度) 指纹变了才重解析——无 watcher(§56 精神),
         热路径零 IO;JSON 亚毫秒级,全量重解析不做增量
```

"Default" profile 不标注;其余 profile 名进副标题(Flow 同款)。

## 搜索 / 排序 / 展示

```text
搜索键 = [title(原始大小写), pinyin_full, pinyin_initials, domain]
排序   = score desc,再 title_lower(§27/§28 同款,usage_bonus 同公式)
空查询 = usage Top Bookmarks(§115 一致:无 usage 不显示"推荐")
行展示 = title + 副标题(domain,非 Default profile 追加 "· <profile>")
       + accessory(浏览器名 Edge/Chrome)
图标   = 来源浏览器 exe 图标(load 时后台线程一次性提取,≤2 张);
         提取失败 / 浏览器未装 → SystemIcon::Generic。
         不做网站 favicon(Flow 默认关:需读 Favicons sqlite +
         拷贝解锁,复杂度与依赖不值)
```

## 打开与 usage

```text
打开     = 从哪来回哪开:来源浏览器 exe + URL 参数启动
         (exe 缺失时退回系统默认浏览器,宁可降级不让激活失败)
item_key = {browser}:{url}(§51;不同来源是不同启动动作,分开计数)
session  = Close;失败保持打开并报错(§115)
```

## 依赖决策记录

新增 workspace 依赖 `serde_json`(Chromium Bookmarks 是 JSON;
settings/usage 的手写 TSV 不适合这个量级与嵌套结构)。
matcher / pinyin_index / usage_bonus / 图标提取 复制自
cue-module-app——Rule of Three 第二次使用(§72);第三个消费者
(FileModule)落地后,图标/COM/shell_execute 已下沉 cue-util-win。

---

