//! 锁键状态提示(§140;§144 起裁为纯观察)的配置:纯数据、OS-neutral
//! (§110),Core 负责持久化,host(sakana-windows)负责执行。
//!
//! §144 裁撤:手势(轻点切输入法 / 长按切锁定)、NumLock 三模式与
//! 全部按键拦截/注入一并删除。原因:Windows 在进 LL 钩子**之前**就
//! 翻转 toggle 态,吞键挡不住翻转;被吞的键又不投递,本进程的状态
//! 读数与前台真态永久脱钩;注入翻转的 make/repeat 语义还随按键的
//! 物理按住状态变化。每一层修补都引入下一层不一致,不可靠的部分
//! 不做,只留可靠的部分:锁定状态变化时的 OSD 卡片。

/// 状态上报用的锁键标识(纯数据;host 消息与 OSD 文案都按它区分)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockKey {
    Caps,
    Num,
}

/// 锁键提示服务全量配置(每次 commit 后整体推给 host worker)。
/// 两个布尔行,无校验面,不需要 from_settings。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockKeysConfig {
    /// 总开关:false = 服务停用,不弹卡片(账本仍随按键维护,
    /// 重开时不会补报旧账)。
    pub enabled: bool,
    /// 状态变化时是否弹 OSD 卡片。
    pub osd: bool,
}

impl Default for LockKeysConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            osd: true,
        }
    }
}
