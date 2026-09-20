> sakana 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 150. FileModule 三层评分排序

§138 定案的排序(文件名命中 > 路径命中,同级按 name_lower
字典序)**整体升级为评分制**;匹配语义(小写子串 AND + `ext:`
子集 + `\` 逃生口)一字不动。动机:字典序没有任何"用户更可能
要哪个文件"的语义——搜"报告"时 `报告.docx` 与 `报告 - 副本 (3).docx`
谁前谁后全看字符编码运气。

## 决策

```text
总分 = 匹配分 + usage_bonus − name_noise
排序键:name_hit(bool,沿用旧主键)> 总分降序 > name_lower
       字典序(确定性平局裁决)> path_lower
```

三层信号(借鉴调研:fzy/fzf 的"匹配质量打分"哲学、Listary 的
"频率 + 新近度"智能排序;Everything 的无智能排序是公认弱点,
v0.6.x 及以前的 sakana 同款):

### 1. 匹配分(每个 text token 对文件名 stem 取最佳命中)

```text
基础命中                       +10
位置:stem 开头                 +8   (前缀)
     词首边界                  +4   (左邻是分隔符或中英文切换点)
     其余                      0
精确(token == 整个 stem)       额外 +8 → 总 26
长度惩罚(仅 name_hit 档)      -(stem字符数 − 查询总字符数).min(20)
```

词首边界定义:左邻 ∈ {空白 _ - . ( ) [ ] + & #} 或 CJK↔ASCII
切换点(报告v2 → 报告|v2 两段)。位置分遍历 token 在 stem 的
所有出现取最佳,不是首个。

### 2. usage_bonus(§145,FileModule 此前唯一没接的模块)

`usage_bonus(ctx.usage, entry.path)`,item_key 已是全路径。
上限 +50 的量纲刻意设计:能拉开同匹配档内名次,压不过
"多命中一个 token"(+10)——常用文件在同档内浮起,不能
反超匹配明显更好的文件。

**两阶段截断**(性能:usage 查找带锁,命中集最坏数万条):
廉价分排序 → 取 top 2×limit → 小集合上查 usage 重排 →
truncate(limit)。已知边界:高频使用但匹配分跌出 top 2×limit
的文件救不回来——量纲设计使然,可接受。

### 3. name_noise(用户提出的"文件名复杂度",建索引时预计算)

```text
stem 以 ~$ 开头                      +8  Office 锁文件(名字还是截断的)
文件无扩展名                          +2  疑似数据/脚本碎片;目录豁免
扩展名 ∈ {tmp temp log bak old
  part crdownload partial download}  +8  打不开的临时/碎片
stem 尾部 (数字)                      +6  Windows 复制/下载防覆盖
stem 含副本标记(- copy/-copy/_copy/
  copy/- 副本/副本)                  +6
stem 含版本词(final/最终/修改/
  草稿/draft)                        +3
stem 以机器名开头
  (img_/dsc/mmexport/截图/屏幕截图/
  screenshot/未命名/新建)             +4
stem 含日期形(8 连数字 / dddd-dd-dd)  +3
分隔段数 >3                           +2/段,封顶 6
stem >40 字符                        +2
封顶 20,存 IndexEntry.noise: u8
```

定义锚点:区分**"系统/应用为防覆盖自动添加的"**与**"用户有意
命名的"**。刻意做成惩罚侧(垃圾后缀进噪声)而非奖励侧(常见
后缀白名单加分):白名单必然偏科(办公/开发/设计互相误伤),
"这个用户常开什么类型"的最佳代理是 usage_bonus(动态、
个性化);`~$告.docx` 带着合法 .docx 后缀,只有黑名单打得住。
`ext:` 显式过滤与噪声惩罚正交:搜 `报告 ext:docx` 时
`~$告.docx` 依然匹配、依然沉底。

## 行为影响

```text
变化    同名命中集内的顺序:精确名 > 前缀 > 词首 > 中间;
        副本/下载碎片/锁文件/版本堆砌沉底;用户打开过的文件浮起
不变    匹配语义(子串 AND、ext:、\ 逃生口)、name_hit 主键、
        结果条数、present/actions/激活链路、设置面(无新设置行)
不变    空查询仍返回空(usage 只能按键查)
```

误伤边界(接受并记录):真叫"最终版"的正式文档沉 3 分;排查时
搜 `.log` 的中间命中仍高匹配备先(单 token 精确 26 > 10−8);
目录名无扩展名不惩罚。

## 用户侧影响

零设置、零迁移;已有 usage 数据立即生效(激活时一直在上报)。

## 实现落点

```text
index.rs  IndexEntry.noise 字段(爬虫线程算);token_score /
          name_noise / split_stem_ext 纯函数;search_entries
          两阶段评分截断,签名加 usage: Option<&UsageReader>
lib.rs    FileModule 存 ctx.usage(load 时),query 传入;
          删"V1 不做 usage 重排"头注释
依赖      sakana-module-file → sakana-util-common(usage_bonus,
          §145 下沉的既有产物,第三次使用)
```

不引入 regex(手写小函数,与 app/bookmark matcher 同纪律);
不下沉 app/bookmark 的 matcher.rs(子序列 vs 子串位置,语义
不同,Rule of Three 未触发)。

## 事故与修复(v0.7.1,2026-09-20)

**v0.7.0 生产事故**:安装后文件搜索永久空白(不是"结果不对",
是"什么都不显示")。

根因:`ends_with_version` 对纯数字 stem(`20240901.jpg`、`123.txt`
这类相机/截图产物)执行 `s.len() - digits - 1`:`digits == s.len()`
时下溢 → debug 构建 panic("attempt to subtract with overflow"),
release 构建 wrap 成 `usize::MAX` 后切片越界 panic。索引线程是裸
`std::thread::spawn`,panic 即静默死亡(§132 日志无 panic hook,
release 无 stderr)→ 快照永不发布 → `wait_snapshot()` 永久
pending → 文件搜索永久空白。**用户目录里几乎必然存在纯数字
文件名**,所以是必现事故。

教训:噪声分计算在索引线程的**外部数据**(任意文件名)路径上,
行级边界必须穷举;v0.7.0 的测试只覆盖了"正常名字"(报告.docx
等),没有纯数字名。

修复三层:

```text
1. 根因    ends_with_version 增加 digits == s.len() 提前返回
2. 栅栏    病态名测试(纯数字/全符号/emoji/超长/多字节边界/
          单字符/无扩展名 等 37 个名字)+ 端到端 crawl 回归
          (临时目录放纯数字名,走真实首爬路径)
3. 兜底    索引线程顶层 catch_unwind(§150 增补):捕获后写
          Error 日志(索引线程处理外部数据,一个未预料的
          panic 不得静默),且首爬未完成时发布空快照——
          查询降级为"无结果"而非永久挂起。已有快照时保持
          冻结(旧内容好过清空)。
```

真实目录验证(修复后,debug 构建):
`cargo test -p sakana-module-file -- --ignored crawl_real_profile_smoke`
→ **440,891 条目,17.25 s,零 panic**(修复前该路径必死于
第一个纯数字文件)。

## 验证

```text
单元 = noise 逐模式打分、匹配分键序(精确>前缀>词首>中间)、
     usage 平局打破、垃圾后缀/锁文件沉底、目录豁免、
     病态名永不 panic(37 个)、端到端 crawl 含纯数字名、
     search_semantics 断言更新(字典序 → 评分序)
真实 = crawl_real_profile_smoke(#[ignore],440,891 条目零 panic)
回归 = fmt / clippy -D warnings / cargo test --workspace /
     check-arch 四门禁全绿
```
