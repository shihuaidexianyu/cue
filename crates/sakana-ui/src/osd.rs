//! 锁键状态 OSD(§140):第二个 GPUI 窗口的视图,纯渲染。
//!
//! 窗口的显示 / 隐藏 / 定位 / 自动收起计时全部在编排层(Win32,
//! 与 host 哲学一致)——本视图只持有"当前要画的状态"。依赖方向
//! 不变:sakana-ui 不认识 sakana-windows,事件以 protocol 的
//! `LockKey` 纯数据进来。
//!
//! 样式与 Launcher 同一块色板(§135 之后的统一深色);图标位用
//! 排版字标("Aa" / "123")而不是字体字形 gamble——Segoe Fluent
//! 图标字体没有可依赖的码点,等宽字标在任何字体下都成立。

use gpui::*;
use sakana_protocol::LockKey;
use sakana_protocol::logln;

pub struct OsdView {
    /// None = 没有要显示的内容(窗口此时也是隐藏的)。
    state: Option<(LockKey, bool)>,
}

impl OsdView {
    pub fn new() -> Self {
        Self { state: None }
    }

    /// 编排层在窗口显示前更新状态(先换内容再 SetWindowPos 显示,
    /// 用户永远看不到上一张卡片)。
    pub fn set_state(&mut self, key: LockKey, on: bool, cx: &mut Context<Self>) {
        // 每次内容更新都进日志:卡片显示什么 = 这里最后一次 set 的
        // 参数。盖卡/闪卡类问题不用猜,日志里时序一目了然。
        logln!("[osd] view <- {key:?} on={on}");
        self.state = Some((key, on));
        cx.notify();
    }
}

impl Default for OsdView {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for OsdView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let Some((key, on)) = self.state else {
            return div();
        };
        let (monogram, title) = match key {
            LockKey::Caps => ("Aa", "大写锁定"),
            LockKey::Num => ("123", "数字键盘"),
        };
        // 开 = 值蓝(与设置页开关一致);关 = 警示黄(值得注意但不是错误)。
        let (state_text, state_color) = if on {
            ("已开启", rgb(0x61afef))
        } else {
            ("已关闭", rgb(0xe5c07b))
        };

        div()
            .w_full()
            .h_full()
            .bg(rgb(0x1e1e24))
            .border_1()
            .border_color(rgb(0x3d3d49))
            .rounded(px(10.0))
            .text_color(rgb(0xe6e6e6))
            .flex()
            .items_center()
            .px(px(12.0))
            .child(
                div()
                    .w(px(36.0))
                    .h(px(36.0))
                    .flex_none()
                    .rounded(px(8.0))
                    .bg(rgb(0x2a2a33))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_base()
                    .text_color(state_color)
                    .child(monogram),
            )
            .child(div().w(px(10.0)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(div().text_sm().whitespace_nowrap().child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(state_color)
                            .whitespace_nowrap()
                            .child(state_text),
                    ),
            )
    }
}
