//! cue-ui —— GPUI 界面(architecture.md §57、§108)。
//!
//! 固定网格布局:icon 槽位永远占固定宽度,`None` 即留空,文字起点永不移动。
//! cue-ui 不认识 Module,不认识 Win32;CoreEffect 的执行经由注入的
//! effect handler 交给编排层(cue binary,§112)。

use cue_core::{Core, CoreEffect, CoreEvent};
use cue_protocol::{IconImage, ResultAccessory, ResultIcon, ResultPresentation, SystemIconId};
use futures::StreamExt;
use gpui::*;
use gpui::prelude::FluentBuilder;
use std::collections::HashMap;
use std::sync::Arc;

/// §14:GPU 纹理按 `Arc` 指针缓存——module 对同一缓存图标复用同一
/// `Arc<[u8]>`,指针即缓存 key,同一张图只转换/上传一次。
type TextureCache = HashMap<usize, Arc<RenderImage>>;

/// §14 契约:协议侧是 RGBA8 直线 alpha;GPUI atlas 存 BGRA(见 gpui
/// 解码路径的逐像素 swap),上传时转换。契约违约(len != w*h*4)时
/// 放弃本张图标而非 panic(§63)。
fn raster_to_texture(icon: &IconImage) -> Option<Arc<RenderImage>> {
    let mut bgra = icon.rgba.to_vec();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(icon.width, icon.height, bgra)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(buffer)
    ])))
}

fn texture_key(icon: &IconImage) -> usize {
    Arc::as_ptr(&icon.rgba) as *const u8 as usize
}

/// Launcher 主视图:持有 Core,把 Core 状态渲染成固定网格。
pub struct LauncherView {
    core: Core,
    focus: FocusHandle,
    // ---- 渲染快照(Core 状态的呈现副本)----
    input: String,
    rows: Vec<ResultPresentation>,
    selected: Option<usize>,
    error: Option<String>,
    /// FocusInput 效果的视图侧标记:在下一帧 render 时落到窗口焦点上。
    want_focus: bool,
    /// §112:CoreEffect 的外部执行器(由编排层注入;FocusInput 同时走视图侧)。
    effect_handler: Option<Box<dyn FnMut(CoreEffect)>>,
    icon_textures: TextureCache,
}

impl LauncherView {
    pub fn new(mut core: Core, cx: &mut Context<Self>) -> Self {
        let mut core_rx = core.take_event_receiver();
        let focus = cx.focus_handle();

        // §96:Core 事件泵。事件在 UI 线程消费,驱动状态机并重绘。
        // §96:Core 事件泵。事件在 UI 线程消费,驱动状态机并重绘。
        // 注意:必须是 async closure(lending AsyncFnOnce),`|..| async move {}`
        // 形式无法对 &mut AsyncApp 的 HRTB 泛化。
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Some(event) = core_rx.next().await {
                if this
                    .update(&mut *cx, |view, cx| view.on_core_event(event, cx))
                    .is_err()
                {
                    break; // 视图已销毁
                }
            }
        })
        .detach();

        Self {
            core,
            focus,
            input: String::new(),
            rows: Vec::new(),
            selected: None,
            error: None,
            want_focus: false,
            effect_handler: None,
            icon_textures: HashMap::new(),
        }
    }

    pub fn set_effect_handler(&mut self, handler: Box<dyn FnMut(CoreEffect)>) {
        self.effect_handler = Some(handler);
    }

    fn on_core_event(&mut self, event: CoreEvent, cx: &mut Context<Self>) {
        let changed = self.core.handle_event(event);
        self.after_core_change(changed, cx);
    }

    fn after_core_change(&mut self, changed: bool, cx: &mut Context<Self>) {
        if changed {
            self.refresh_snapshot();
        }
        for effect in self.core.take_effects() {
            if effect == CoreEffect::FocusInput {
                self.want_focus = true;
            }
            if let Some(handler) = self.effect_handler.as_mut() {
                handler(effect);
            }
        }
        cx.notify();
    }

    /// 快照:Core 状态 → 可直接渲染的行。present 只对当前结果调用(§105)。
    fn refresh_snapshot(&mut self) {
        let Some(session) = self.core.session() else {
            self.input.clear();
            self.rows.clear();
            self.selected = None;
            self.error = None;
            return;
        };
        self.input = session.raw_input.clone();
        self.selected = session.selected;
        self.error = session.error.as_ref().map(ToString::to_string);
        self.rows = session
            .results
            .iter()
            .filter_map(|item| self.core.present(item))
            .collect();
    }

    // ------------------------------------------------------------------
    // 键盘(§59):统一由 Core 处理,视图只翻译按键。
    // ------------------------------------------------------------------

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;

        match keystroke.key.as_str() {
            "escape" => self.core.close_session(),
            "enter" => self.core.activate_selected(),
            "up" => self.core.select_prev(),
            "down" => self.core.select_next(),
            "backspace" => self.core.backspace(),
            "v" if modifiers.control && !modifiers.alt => {
                // §115:允许粘贴 Unicode(IME 禁用只针对 composition)。
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.core.paste(&text);
                }
            }
            _ => {
                if !modifiers.control && !modifiers.alt {
                    if let Some(text) = keystroke.key_char.clone() {
                        self.core.push_text(&text);
                    }
                }
            }
        }
        self.after_core_change(true, cx);
    }

    // ------------------------------------------------------------------
    // 渲染(§108 固定网格)
    // ------------------------------------------------------------------

    fn render_input(&self) -> Div {
        let content: Div = if self.input.is_empty() {
            div()
                .text_color(rgb(0x6a6a75))
                .child("Type to search")
        } else {
            div().child(format!("{}▍", self.input))
        };
        div()
            .h(px(36.0))
            .flex()
            .items_center()
            .px(px(6.0))
            .text_lg()
            .child(content)
    }

    fn render_icon_slot(textures: &TextureCache, row: &ResultPresentation) -> Div {
        // icon 槽位:固定 32px,None 即留空,文字起点永不移动(§108)。
        let slot = div()
            .w(px(32.0))
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .justify_center();
        match &row.icon {
            None => slot,
            Some(ResultIcon::SystemIcon(id)) => slot.child(match id {
                SystemIconId::App => "🚀",
                SystemIconId::File => "📄",
                SystemIconId::Folder => "📁",
                SystemIconId::Generic => "▪",
            }),
            Some(ResultIcon::Raster(icon)) => match textures.get(&texture_key(icon)) {
                // 24px 显示尺寸;96px 源纹理由 GPUI 降采样(§14)。
                Some(texture) => slot.child(img(Arc::clone(texture)).w(px(24.0)).h(px(24.0))),
                None => slot,
            },
        }
    }

    fn render_row(row: &ResultPresentation, is_selected: bool, textures: &TextureCache) -> Div {
        let subtitle = row
            .subtitle
            .clone()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let accessory = row.accessory.as_ref().map(|a| match a {
            ResultAccessory::Text(t) | ResultAccessory::Shortcut(t) => t.to_string(),
        });

        let mut text_col = div()
            .flex_1()
            .flex()
            .flex_col()
            .justify_center()
            .overflow_hidden()
            .child(div().text_sm().child(row.title.to_string()));
        if !subtitle.is_empty() {
            text_col = text_col.child(
                div()
                    .text_xs()
                    .text_color(rgb(0x9a9aa3))
                    .child(subtitle),
            );
        }

        let mut container = div()
            .h(px(44.0))
            .flex()
            .items_center()
            .rounded_md()
            .px(px(4.0))
            .when(is_selected, |d| d.bg(rgb(0x2d4f67)))
            .child(Self::render_icon_slot(textures, row))
            .child(text_col);
        if let Some(accessory) = accessory {
            container = container.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(0x9a9aa3))
                    .child(accessory),
            );
        }
        container
    }
}

impl Render for LauncherView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.want_focus {
            self.want_focus = false;
            window.focus(&self.focus);
        }

        // 先把本帧出现的 Raster 图标全部转成纹理(§14:按 Arc 指针
        // 缓存,同一图标只上传一次)。字段级拆分借用,rows 只读、
        // textures 只写,互不冲突。
        {
            let rows = &self.rows;
            let textures = &mut self.icon_textures;
            for row in rows {
                if let Some(ResultIcon::Raster(icon)) = &row.icon {
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        textures.entry(texture_key(icon))
                    {
                        // 契约违约的图标不留空槽占位条目,下一帧重试。
                        if let Some(texture) = raster_to_texture(icon) {
                            e.insert(texture);
                        }
                    }
                }
            }
        }

        let mut list = div().flex().flex_col();
        for (i, row) in self.rows.iter().enumerate() {
            list = list.child(Self::render_row(
                row,
                self.selected == Some(i),
                &self.icon_textures,
            ));
        }

        let body: Div = if let Some(error) = &self.error {
            div()
                .py(px(12.0))
                .flex()
                .justify_center()
                .text_color(rgb(0xe06c75))
                .child(format!("Error: {error}"))
        } else if self.rows.is_empty() {
            // §58:没有结果时的空态。
            div()
                .py(px(12.0))
                .flex()
                .justify_center()
                .text_color(rgb(0x6a6a75))
                .child("No results")
        } else {
            list
        };

        div()
            .id("launcher")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .w_full()
            .h_full()
            .bg(rgb(0x1e1e24))
            .text_color(rgb(0xe6e6e6))
            .p(px(6.0))
            .flex()
            .flex_col()
            .child(self.render_input())
            .child(div().h(px(1.0)).w_full().bg(rgb(0x33333d)))
            .child(body)
    }
}
