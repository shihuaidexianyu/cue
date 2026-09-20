> sakana 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 152. 工具缓存目录扩充 + 排除匹配编译为分段查表

用户报告(v0.7.2 后):"能不能把这些类似的目录都屏蔽掉,我觉得网上
应该有清单"——§151 只收了 target/obj,日常还会撞上 .next/.cache/
.terraform 这类工具内脏。要求按业界清单一次性收全,且不许把性能
拖出预算。

## 决策

两条线并行:

```text
1. 默认名单 15 → 43 通用片段(+ 9 条 USERPROFILE 展开,合计 52):
     依赖与虚拟环境   node_modules bower_components __pycache__
                     .venv venv virtualenv .yarn .pnpm-store
     构建产物         target obj .next .nuxt .svelte-kit .angular
                     .output .turbo .parcel-cache .vite .docusaurus
                     .terraform .serverless
     工具缓存/测试产物 .cache .sass-cache .pytest_cache .mypy_cache
                     .ruff_cache .tox .nox .hypothesis .nyc_output
                     .gradle .expo
     IDE 配置         .idea .vs
   收录判据(§151 延续;名单分组即判据标题,seed 写进 TOML 注释):
     - 带点的工具目录几乎零误伤——没人给资料夹起名 .next / .cache;
     - 语言专属强约定目录(node_modules / __pycache__ / target / obj);
     - 参考 VS Code 官方 search.exclude 默认与主流 gitignore 模板。
   刻意不排(判断记录):build / dist / out / bin / vendor / env /
   coverage / packages / lib——通用英文词,用户可能放真实资料,
   误伤面大于收益。.gradle 等原在 USERPROFILE 展开组,提升为通用
   段名(项目内同名目录同属工具内脏)。

2. 排除匹配编译为分段查表(性能):
   动机:片段翻倍后,逐片段 contains 让查询级过滤成本随片段数
   线性膨胀,触碰 §78 预算。
   做法(ExcludeIndex;名单变更时重建一次,查询只克隆 Arc):
     两侧 \ 的单段片段      → 段名 HashSet,按 path.split('\\') 查
                             段,成本 O(段数) 而非 O(片段数)
     盘符前缀(如 C:\Windows\)→ 前缀列表 starts_with
     非锚定片段(用户自定义)→ 子串扫描兜底(保持旧语义)
   末段语义:`\` 结尾的目录路径全部段参与;文件路径的末段是文件名,
   不参与——名为 target/.cache 的文件不是工具目录。整体与旧子串
   锚定判定严格等价,只把成本口径从 O(片段数) 换成 O(段数)。
```

逃生口语义不变:查询含 `\` 时查询级过滤整体不生效,被剪枝子树
够不到(§138 已记录的弱化不变)。

## 存量升级

沿用 §151 机制:新增 v151 快照(v0.7.2 的播种内容与顺序逐字比对
确认),`legacy_default_fragment_versions` 现含 §121 / §125 / §151
三版;未改过名单的用户重写为新默认,改过即不触碰,幂等。

## 行为影响

```text
新增     .next/.cache/.terraform 等 28 个工具目录整棵不进索引
不变     查询级过滤语义与旧子串锚定判定等价(末段是文件名时不判),
         通用词不排、逃生口语义、用户自改名单不被触碰、排序语义
性能     片段 25 → 52 翻倍,合成负载查询成本持平(见验证)
```

## 实现落点

```text
lib.rs     DEFAULT_FRAGMENT_GROUPS 分组常量(6 组 43 片段)+
           HOME_FRAGMENTS;ExcludeIndex{seg_names,prefixes,free} 与
           matches;ExcludeState.patterns(名单变更同步重建);
           refreshed_fragments 返回编译模式;
           seed_exclude_file 按组写注释标题;
           v151_default_fragments 快照 + legacy 数组扩到 3 版
index.rs   dir_pruned / crawl / search_entries 改吃 &ExcludeIndex;
           性能栅栏 wide_query_latency_under_fragment_growth(#[ignore])
```

## 验证

```text
单元 = 段匹配/前缀/子串三类片段语义、目标目录剪枝回归
     (.next/.terraform/venv/.cache)、通用词不误伤(build/vendor)、
     seed 分组标题与新片段、历史三版快照升级幂等、改过名单不触碰、
     refreshed_fragments 重建编译模式
性能 = release 合成负载(20 万路径,宽泛查询命中全部):
       名单 25 → 52 片段,整查询 33.3 → 34.1 ms 持平(跨轮次
       33–43 ms 波动,两版无显著差异);
       纯匹配对照(同一 52 片段,20 万路径):旧子串语义
       77.2 ms/pass → 编译索引 34.4 ms/pass(≈2.2×);
       注解:§78 的 P95 < 15 ms 是生产口径(§114 实测);本栅栏是
       名单增长的相对浪涌看守,防"片段线性膨胀"回归。
     真实首爬冒烟(%USERPROFILE%,release):1 根 → 269,288 条目、
     18 s、无 panic(新名单剪枝口径下真实规模的健全性检查)
回归 = fmt / clippy -D warnings / cargo test --workspace /
     check-arch 四门禁全绿
```