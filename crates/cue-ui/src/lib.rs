//! cue-ui —— GPUI 界面(architecture.md §57、§108)。
//!
//! 固定网格布局:icon 槽位永远占固定宽度,`None` 即留空,文字起点永不移动。
//! cue-ui 不认识 Module,不认识 Win32;CoreEffect 的执行经由注入的
//! effect handler 交给编排层(cue binary,§112)。

use cue_core::{
    ActionMenuModel, ActionMenuRow, Core, CoreEffect, CoreEvent, SettingsModel, SettingsRow,
    KEY_HOTKEY,
};
use cue_protocol::{
    Hotkey, IconImage, Key as ProtoKey, Modifiers as ProtoModifiers, ResultAccessory, ResultIcon,
    ResultPresentation, SettingKind, SettingValue, SystemIconId,
};
use futures::StreamExt;
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::collections::HashMap;
use std::sync::Arc;

/// §14:GPU 纹理按 `Arc` 指针缓存——module 对同一缓存图标复用同一
/// `Arc<[u8]>`,指针即缓存 key,同一张图只转换/上传一次。
type TextureCache = HashMap<usize, Arc<RenderImage>>;

/// 视口内可见结果行数:窗口 420px - 内边距 12 - 输入行 36 - 分隔线 1,
/// ÷ 行高 44px。结果可多于可见行数(result_limit=20),超出的行由
/// 选择驱动的滚动窗口覆盖(键盘 launcher 不需要真滚动条)。
const VISIBLE_ROWS: usize = 8;

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

/// 设置页热键捕获:GPUI keystroke → 协议 Hotkey(§53 OS-neutral 描述)。
/// 纯修饰键或不可映射键名返回 None(视图继续等待下一次按键)。
fn capture_candidate(ks: &Keystroke) -> Option<Hotkey> {
    let key = ProtoKey::parse(ks.key.as_str())?;
    let m = &ks.modifiers;
    Some(Hotkey {
        modifiers: ProtoModifiers {
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
            super_key: m.platform,
        },
        key,
    })
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
    /// §41 设置页的热键捕获态(视图本地):true 时下一次按键组合
    /// 成为 core.hotkey 候选。
    capturing_hotkey: bool,
    /// §114 测量探针(§79):文本输入的时刻;下一次结果行非空时
    /// 打印 input→rows 时延(InputChanged → ResultState 提交的视图侧
    /// 上界,含事件泵与 present)。
    perf_input_at: Option<std::time::Instant>,
    /// 结果滚动窗口起点:渲染只取 [scroll_offset, +VISIBLE_ROWS) 切片,
    /// 选中项变化时在 refresh_snapshot 里滚入视口。
    scroll_offset: usize,
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
            capturing_hotkey: false,
            perf_input_at: None,
            scroll_offset: 0,
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
            // §114 探针:结果行首次非空即输入→结果可见的上界。
            if let Some(t0) = self.perf_input_at.take_if(|_| !self.rows.is_empty()) {
                eprintln!("[perf] input->rows in {:?}", t0.elapsed());
            }
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
            self.scroll_offset = 0;
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
        // 选中项滚入视口(新非空结果选中第 0 行,自然归零)。
        match self.selected {
            Some(sel) if sel < self.scroll_offset => self.scroll_offset = sel,
            Some(sel) if sel >= self.scroll_offset + VISIBLE_ROWS => {
                self.scroll_offset = sel + 1 - VISIBLE_ROWS;
            }
            _ => {}
        }
        self.scroll_offset = self
            .scroll_offset
            .min(self.rows.len().saturating_sub(VISIBLE_ROWS));
    }

    // ------------------------------------------------------------------
    // 键盘(§59):统一由 Core 处理,视图只翻译按键。
    // ------------------------------------------------------------------

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        // §41:设置页与搜索页是两套键盘语义,Core 状态决定路由。
        if self.core.in_settings() {
            self.on_settings_key_down(event);
            self.after_core_change(true, cx);
            return;
        }
        // §18:动作菜单打开时是第三套语义(模态)。
        if self.core.in_action_menu() {
            self.on_action_menu_key_down(event);
            self.after_core_change(true, cx);
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;

        match keystroke.key.as_str() {
            "escape" => self.core.close_session(),
            "enter" => self.core.activate_selected(),
            "tab" => self.core.open_action_menu(),
            "up" => self.core.select_prev(),
            "down" => self.core.select_next(),
            "backspace" => {
                self.perf_input_at = Some(std::time::Instant::now());
                self.core.backspace();
            }
            "v" if modifiers.control && !modifiers.alt => {
                // §115:允许粘贴 Unicode(IME 禁用只针对 composition)。
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.perf_input_at = Some(std::time::Instant::now());
                    self.core.paste(&text);
                }
            }
            // GPUI 把 VK_SPACE 列为 immutable 命名键("space"),不生成
            // key_char——必须在 fallback 之前显式插入,否则空格被吞
            // ("b github" 变成 "bgithub",触发词永远吃不到词边界)。
            "space" if !modifiers.control && !modifiers.alt => {
                self.perf_input_at = Some(std::time::Instant::now());
                self.core.push_text(" ");
            }
            _ => {
                if !modifiers.control && !modifiers.alt {
                    if let Some(text) = keystroke.key_char.clone() {
                        self.perf_input_at = Some(std::time::Instant::now());
                        self.core.push_text(&text);
                    }
                }
            }
        }
        self.after_core_change(true, cx);
    }

    // ------------------------------------------------------------------
    // 设置页键盘(§41):↑↓ 选择,Enter/Space 修改,Esc 返回。
    // 热键行进入捕获态:下一次组合键即候选,事务结果由 Core 模型回显。
    // ------------------------------------------------------------------

    fn on_settings_key_down(&mut self, event: &KeyDownEvent) {
        let ks = &event.keystroke;
        if self.capturing_hotkey {
            self.capturing_hotkey = false;
            if ks.key.as_str() == "escape" {
                return; // 取消捕获
            }
            if let Some(hotkey) = capture_candidate(ks) {
                // 失败(如无修饰键、注册冲突):Core 保留旧值并把错误
                // 放进模型(§42),无需视图侧处理。
                let _ = self
                    .core
                    .apply_setting(KEY_HOTKEY, SettingValue::Hotkey(hotkey));
            } else {
                // 纯修饰键/不可映射键:继续等待下一次按键。
                self.capturing_hotkey = true;
            }
            return;
        }
        match ks.key.as_str() {
            "escape" => self.core.dismiss_settings(),
            "up" => self.core.settings_select_prev(),
            "down" => self.core.settings_select_next(),
            "enter" | "space" => self.settings_activate_selected(),
            _ => {}
        }
    }

    fn settings_activate_selected(&mut self) {
        let Some(model) = self.core.settings_model() else {
            return;
        };
        let Some(row) = model.rows.get(model.selected) else {
            return;
        };
        match (row.kind, &row.value) {
            (SettingKind::Bool, SettingValue::Bool(b)) => {
                let key = row.key.to_string();
                let _ = self.core.apply_setting(&key, SettingValue::Bool(!b));
            }
            (SettingKind::Hotkey, _) => {
                self.capturing_hotkey = true;
            }
            // V1 没有 Integer/String/Enum/Path 类设置;出现后按 §39 再加编辑 UI。
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // 动作菜单键盘(§18):↑↓ 选择,Enter 执行,Esc/Tab 返回。
    // 模态:其余按键关掉菜单并被吞掉(不落进搜索输入)。
    // ------------------------------------------------------------------

    fn on_action_menu_key_down(&mut self, event: &KeyDownEvent) {
        match event.keystroke.key.as_str() {
            "escape" | "tab" => self.core.close_action_menu(),
            "up" => self.core.action_menu_select_prev(),
            "down" => self.core.action_menu_select_next(),
            "enter" => self.core.activate_action_menu_selection(),
            _ => self.core.close_action_menu(),
        }
    }

    // ------------------------------------------------------------------
    // 渲染(§108 固定网格)
    // ------------------------------------------------------------------

    fn render_input(&self) -> Div {
        let content: Div = if self.input.is_empty() {
            div().text_color(rgb(0x6a6a75)).child("Type to search")
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

    // ------------------------------------------------------------------
    // 设置页渲染(§41):模型来自 Core,视图只做布局。
    // ------------------------------------------------------------------

    fn render_settings(&self, model: &SettingsModel) -> Div {
        let mut list = div().flex().flex_col();
        for (i, row) in model.rows.iter().enumerate() {
            list = list.child(self.render_settings_row(row, i == model.selected));
        }

        let mut body = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .px(px(6.0))
                    .text_lg()
                    .child("设置"),
            )
            .child(div().h(px(1.0)).w_full().bg(rgb(0x33333d)))
            .child(list);

        if model.restart_required {
            body = body.child(
                div()
                    .px(px(6.0))
                    .py(px(4.0))
                    .text_xs()
                    .text_color(rgb(0xe5c07b))
                    .child("部分设置将在重启 CUE 后生效"),
            );
        }
        if let Some(error) = &model.error {
            body = body.child(
                div()
                    .px(px(6.0))
                    .py(px(4.0))
                    .text_xs()
                    .text_color(rgb(0xe06c75))
                    .child(format!("设置未生效:{error}")),
            );
        }
        body.child(
            div()
                .px(px(6.0))
                .py(px(4.0))
                .text_xs()
                .text_color(rgb(0x6a6a75))
                .child("↑↓ 选择 · Enter 修改 · Esc 返回"),
        )
    }

    fn render_settings_row(&self, row: &SettingsRow, is_selected: bool) -> Div {
        let value_text = match &row.value {
            SettingValue::Bool(b) => {
                if *b {
                    "开".to_string()
                } else {
                    "关".to_string()
                }
            }
            SettingValue::Hotkey(h) => {
                if self.capturing_hotkey && is_selected {
                    "按下新组合键…(Esc 取消)".to_string()
                } else {
                    h.to_string()
                }
            }
            SettingValue::Integer(i) => i.to_string(),
            SettingValue::String(s) | SettingValue::Enum(s) => s.clone(),
            SettingValue::Path(p) => p.display().to_string(),
        };

        let mut label_col = div()
            .flex_1()
            .flex()
            .flex_col()
            .justify_center()
            .overflow_hidden()
            .child(div().text_sm().child(row.label.to_string()));
        if let Some(desc) = &row.description {
            label_col = label_col.child(
                div()
                    .text_xs()
                    .text_color(rgb(0x9a9aa3))
                    .child(desc.to_string()),
            );
        }

        div()
            .h(px(44.0))
            .flex()
            .items_center()
            .rounded_md()
            .px(px(6.0))
            .when(is_selected, |d| d.bg(rgb(0x2d4f67)))
            .child(label_col)
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(0x61afef))
                    .child(value_text),
            )
    }

    // ------------------------------------------------------------------
    // 动作菜单渲染(§18):整体替换结果区(同设置页模式),
    // 头部标注菜单归属的选中项。
    // ------------------------------------------------------------------

    fn render_action_menu(&self, model: &ActionMenuModel) -> Div {
        let mut list = div().flex().flex_col();
        for (i, row) in model.rows.iter().enumerate() {
            list = list.child(Self::render_action_row(row, i == model.selected));
        }
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .px(px(6.0))
                    .text_xs()
                    .text_color(rgb(0x9a9aa3))
                    .child(format!("动作 · {}", model.item_title)),
            )
            .child(div().h(px(1.0)).w_full().bg(rgb(0x33333d)))
            .child(list)
            .child(
                div()
                    .px(px(6.0))
                    .py(px(4.0))
                    .text_xs()
                    .text_color(rgb(0x6a6a75))
                    .child("↑↓ 选择 · Enter 执行 · Esc/Tab 返回"),
            )
    }

    fn render_action_row(row: &ActionMenuRow, is_selected: bool) -> Div {
        let mut container = div()
            .h(px(36.0))
            .flex()
            .items_center()
            .rounded_md()
            .px(px(6.0))
            .when(is_selected, |d| d.bg(rgb(0x2d4f67)))
            .child(div().flex_1().text_sm().child(row.label.to_string()));
        if let Some(shortcut) = &row.shortcut {
            container = container.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(0x9a9aa3))
                    .child(shortcut.clone()),
            );
        }
        container
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
            text_col = text_col.child(div().text_xs().text_color(rgb(0x9a9aa3)).child(subtitle));
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
        for (i, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(VISIBLE_ROWS)
        {
            list = list.child(Self::render_row(
                row,
                self.selected == Some(i),
                &self.icon_textures,
            ));
        }

        let in_settings = self.core.in_settings();
        let body: Div = if let Some(model) = self.core.settings_model() {
            // §41:设置模式整体替换搜索区(输入行也不再显示,见下方 chrome)。
            self.render_settings(&model)
        } else if let Some(menu) = self.core.action_menu_model() {
            // §18:动作菜单整体替换结果区(输入行保留,可见查询上下文)。
            self.render_action_menu(&menu)
        } else {
            let mut col = div().flex().flex_col();
            if let Some(error) = &self.error {
                // §115:激活失败 session 保持打开——错误做成横幅叠加在
                // 结果列表上方,用户仍可 ↑↓ 选择其他项重试。
                col = col.child(
                    div()
                        .px(px(6.0))
                        .py(px(4.0))
                        .text_xs()
                        .text_color(rgb(0xe06c75))
                        .child(format!("Error: {error}")),
                );
            }
            if self.rows.is_empty() {
                // §58:没有结果时的空态。
                col = col.child(
                    div()
                        .py(px(12.0))
                        .flex()
                        .justify_center()
                        .text_color(rgb(0x6a6a75))
                        .child("No results"),
                );
            } else {
                col = col.child(list);
            }
            col
        };

        let mut chrome = div()
            .id("launcher")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .w_full()
            .h_full()
            .bg(rgb(0x1e1e24))
            .text_color(rgb(0xe6e6e6))
            .p(px(6.0))
            .flex()
            .flex_col();
        if !in_settings {
            chrome = chrome
                .child(self.render_input())
                .child(div().h(px(1.0)).w_full().bg(rgb(0x33333d)));
        }
        chrome.child(body)
    }
}
