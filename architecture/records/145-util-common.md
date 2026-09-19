> sakana 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 145. sakana-util-common:usage_bonus 第三次复制下沉

Rule of Three(§72–73)的第一次依规执行:app 的 usage 加分公式
(`min(count,20)*2;24h 内 +10,7d 内 +5`)被 bookmark(自称"第二次
使用")、web(自称"第三次使用")逐字复制。第三次出现 = 下沉阈值
已到,且三个消费方逐字相同、无分叉需求——按 §73 纪律下沉到新建的
**平台中立** util crate `sakana-util-common`,与 `sakana-util-win`
(Win32 侧)平行:

```text
crates/sakana-util-common
  依赖  = sakana-protocol(仅复用 UsageReader / ActionId 类型)
  内容  = pub fn usage_bonus(usage: Option<&UsageReader>, item_key: &str) -> i32
  消费  = sakana-module-app / -bookmark / -web
  非消费 = sakana-module-system(刻意):§126 的封顶设计
          (上限 40 = 匹配等级差)形状不同,不强行参数化统一
```

不放进 sakana-protocol:排名公式是模块侧共享实现,不是 Core ↔
Module 契约(协议层只承载数据契约,§71 的延伸)。check-arch.ps1
的 `Allowed` 白名单随之扩一行:`sakana-util-common → sakana-protocol`
only,`sakana-module-*` 的合法依赖加入 util-common,并补三条
回归断言(方向 / 协议依赖 / 模块可达)。

pinyin_index(app/bookmark 两处复制)**维持重复**:Rule of Three
是次数规则,两处未到阈值,bookmark 头注释的"第三处未出现"判断
依然成立(FileModule 最终没有拼音需求)。

## 验证

```text
单测 = util-common 三例:无 usage → 0;分档金样(now=16 / cap=50 /
     two_days=15 / eight_days=10);future last_used 只剩频率项
回归 = app/bookmark/web 既有 usage 排序测试原样通过(走共享路径)
     check-arch.ps1 含 util-common 规则后全绿
```
