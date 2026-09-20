> sakana 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 151. 构建产物从文件搜索中排除

用户报告(v0.7.1):搜 `/geek` 时,目标 `geek.exe` 之外混进两条
`d9geek2ejjyizsadif00c89z9.o`——自己 Rust 项目
`target\debug\incremental\` 下的增量编译中间产物,文件名 hash 里
恰好含 "geek" 被名字命中。问"能不能过滤掉"。

## 决策

两条线并行(前者管目录整体,后者管散落场景):

```text
1. 默认排除名单加两个目录片段:
     \target\    Cargo / Maven 构建产物(约定强度 ≈ node_modules)
     \obj\       MSBuild 中间产物(.NET)
   理由:与 node_modules 同类的"工具内脏"——用户工作文件几乎
   从不放进构建产物目录,但它们体量巨大(Rust incremental 单个
   项目可达数万文件),既淹没结果又白占索引(爬取时整棵剪枝)。

2. 噪声表 JUNK_EXTS 增加编译中间产物后缀:
     o / obj / rlib / rmeta   目标文件与 Rust 库元数据
     pdb                      调试符号
     ilk / exp                MSVC 链接中间物
     pyc                      Python 字节码
   理由:目录排除之外仍有散落场景(拷来的 .o、SDK 里的 .pdb)。
   这些后缀"用户不会有意打开"的共识强度与 tmp/log 同级 → +8。
   与排除名单不同,这是评分惩罚:显式搜到仍在结果里,只是沉底。

刻意不做(记录判断):
  dist / build / out / bin 不排——名字太通用,用户可能放真实
  资料,误伤面大于收益;
  不加"hash 型文件名"(无分隔字母数字串)识别规则——暂无人
  报告该场景,target 排除已覆盖主流;等真实需求再评估(§87)。
```

逃生口不变:查询含 `\` 时查询级过滤不生效,但被剪枝的
`target` / `obj` 子树够不到(与 node_modules 同待遇,§138 已
记录该语义弱化)。

## 存量升级

改默认名单必须让"没动过名单的老用户"跟上。升级判定此前只认
一个历史版本(§121 版);本次重构为**历史快照列表**
(`legacy_default_fragment_versions`,独立硬编码,不跟随当前
逻辑),现在含 §121 与 §125 两版;内容精确等于任一快照(用户
一个片段都没改)即重写为当前默认,改过即不触碰(§125 语义
不变)。以后每次改默认,把被替换的版本追加进列表。

## 行为影响

```text
新增    结果里不再出现自己项目 target / obj 下的构建产物
新增    散落的 .o/.rlib/.pdb 等中间产物沉底(显式搜索仍可达)
性能    爬取阶段整棵剪枝,索引规模与首爬时间随之下降
不变    匹配语义、排序键顺序、其余排除口径、逃生口
不变    用户手工编辑过的排除名单不被触碰
```

## 实现落点

```text
lib.rs     default_fragments 加 \target\ \obj\;
           v121/v125 快照函数 + legacy_default_fragment_versions;
           upgrade_seed_if_legacy 改为匹配任一快照
index.rs   JUNK_EXTS 9 → 17(编译中间产物后缀)
```

## 验证

```text
单元 = 编译后缀噪声值(.o/.rlib/.pdb/.pyc,hash 名另多段 +6
     共 14)、dir_pruned 对 target/obj 的判定、用户场景回归
     (搜 geek 只剩 geek.exe 与 geeknotes.txt)、历史两版快照
     均可升级且幂等、改过名单不触碰、播种含新片段
回归 = fmt / clippy -D warnings / cargo test --workspace /
     check-arch 四门禁全绿
```