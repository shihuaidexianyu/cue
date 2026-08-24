> CUE 架构规格 · V1.x 实现记录。§ 编号全局唯一、跨文件稳定;文件地图与新增章节规则见根目录 architecture.md。

# 126. 系统动作模块(触发词 `>`)

固定枚举的系统动作——锁屏/睡眠/休眠/注销/重启/关机/清空回收站。
不是 shell runner:不接受任意命令(§76 禁区不动),动作表是
模块内静态数据。

## 决议

```text
动作表 = 7 项静态 ActionSpec(name/pinyin/initials/english/extras
         手校;7 个固定中文名不需要 pinyin 引擎——重启的
         chóng/zhòng 多音两条键都收)
匹配   = 完全相等 > 前缀 > 子串(120/100/60);usage 加分封顶
         40(次数 ≤25 + 7 天新近 15)= 匹配等级差,最多追平、
         由表序决胜——usage 重排同级匹配,压不过更强匹配
空查询 = 列出全部动作,usage 在前,未用过的保持表序
休眠   = load 时 GetPwrCapabilities 探测(S4 + 休眠文件),
         不可用则不出现
破坏性 = 分级:重启/关机走 InitiateSystemShutdownEx 30 秒宽限
         (原生倒计时、shutdown /a 可中止、不强制关应用——应用
         可提示保存/拒绝,是第二道保险);锁屏/睡眠/休眠/注销
         立即执行(可逆或正常会话流程);清空回收站无确认
         (SHERB_NOCONFIRMATION 等三旗标)
特权   = load 时 AdjustTokenPrivileges 启用 SE_SHUTDOWN_NAME
         (交互用户令牌自带、默认禁用);失败仅 Warn,执行时报
         具体错
图标   = SystemIconId 协议新增 7 个动作字形(UI 映射 emoji)
         ——协议 additive,无破坏性变更
明确不做 = 任意命令执行(shell runner 禁区)、模块内第二确认
           对话框(launcher 语义即确认;30 秒宽限是保险)、
           自定义动作配置(V1 固定表)、定时/延时动作
```

## 验证

```text
单测 = 拼音/首字母/英文/别名匹配、空查询全列 + usage 提前、
       usage 同级决胜不压强匹配、休眠能力过滤、item id 稳定、
       present/actions 形状
E2E  = 真机:`>` 列出全部动作(图标/副标题,截图)、`>gj`
       过滤到关机(不执行任何动作)
```

---

