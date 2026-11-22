//! The tez look ("Midnight Volt" on the design canvas): one palette, one type scale and the parts
//! every screen is built from. Pages only combine these, so the app stays consistent.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::rc::Rc;

pub const SANS: &str = if cfg!(windows) { "Segoe UI" } else if cfg!(target_os = "macos") { ".SystemUIFont" } else { "Inter" };
pub const MONO: &str = "JetBrains Mono";

// ------------------------------------------------------------------ palette

#[derive(Clone, Copy)]
pub struct Pal {
    pub dark: bool,
    pub bg: Hsla,
    pub side: Hsla,
    pub surface: Hsla,
    pub surface2: Hsla,
    pub surface3: Hsla,
    pub line: Hsla,
    pub line2: Hsla,
    pub text: Hsla,
    pub text2: Hsla,
    pub text3: Hsla,
    pub accent: Hsla,
    pub accent_hi: Hsla,
    pub accent_text: Hsla,
    pub accent_soft: Hsla,
    pub green: Hsla,
    pub green_soft: Hsla,
    pub amber: Hsla,
    pub amber_soft: Hsla,
    pub red: Hsla,
    pub red_soft: Hsla,
    pub blue: Hsla,
    pub blue_soft: Hsla,
    pub code: Hsla,
    /// Chart series in fixed order: PHP pools, site processes, services, web server.
    pub series: [Hsla; 4],
    pub card: Hsla,
    pub cardline: Hsla,
}

fn c(x: u32) -> Hsla { rgb(x).into() }
fn ca(x: u32) -> Hsla { rgba(x).into() }

impl Pal {
    pub fn dark() -> Pal {
        Pal {
            dark: true,
            // side = the window around the panel; bg/surface = the inset panel
            bg: c(0x212121), side: c(0x181818), surface: c(0x212121), surface2: c(0x2a2a2a), surface3: c(0x303030),
            line: c(0x2f2f2f), line2: c(0x3b3b3b),
            text: c(0xececec), text2: c(0xb4b4b4), text3: c(0x8f8f8f),
            accent: c(0xececec), accent_hi: c(0x6b6b6b), accent_text: c(0x9ec5fe), accent_soft: ca(0xffffff14),
            green: c(0x4ade80), green_soft: ca(0x4ade801f),
            amber: c(0xfbbf24), amber_soft: ca(0xfbbf241f),
            red: c(0xf87171), red_soft: ca(0xf871711f),
            blue: c(0x60a5fa), blue_soft: ca(0x60a5fa1f),
            code: c(0x181818),
            series: [c(0x3987e5), c(0xd95926), c(0x199e70), c(0xc98500)],
            card: c(0x262626), cardline: c(0x323232),
        }
    }
    pub fn light() -> Pal {
        Pal {
            dark: false,
            bg: c(0xffffff), side: c(0xf3f3f3), surface: c(0xffffff), surface2: c(0xefefef), surface3: c(0xe6e6e6),
            line: c(0xe9e9e9), line2: c(0xd9d9d9),
            text: c(0x0d0d0d), text2: c(0x3f3f3f), text3: c(0x6b6b6b),
            accent: c(0x0d0d0d), accent_hi: c(0x9b9b9b), accent_text: c(0x1d5fd6), accent_soft: ca(0x0000000d),
            green: c(0x15803d), green_soft: ca(0x15803d14),
            amber: c(0xa15c07), amber_soft: ca(0xd977061a),
            red: c(0xdc2626), red_soft: ca(0xdc262612),
            blue: c(0x2563eb), blue_soft: ca(0x2563eb12),
            code: c(0xf5f5f5),
            series: [c(0x2a78d6), c(0xeb6834), c(0x1baf7a), c(0xeda100)],
            card: c(0xfafafa), cardline: c(0xe6e6e6),
        }
    }
    pub fn tone(&self, t: Tone) -> (Hsla, Hsla) {
        match t {
            Tone::Ok => (self.green, self.green_soft),
            Tone::Warn => (self.amber, self.amber_soft),
            Tone::Err => (self.red, self.red_soft),
            Tone::Info => (self.blue, self.blue_soft),
            Tone::Accent => (self.accent_text, self.accent_soft),
            Tone::Neutral => (self.text2, self.surface3),
        }
    }
}

pub struct PalHolder(pub Pal);
impl Global for PalHolder {}

pub fn pal(cx: &App) -> Pal { cx.try_global::<PalHolder>().map(|p| p.0).unwrap_or_else(Pal::dark) }

#[derive(Clone, Copy, PartialEq)]
pub enum Tone { Ok, Warn, Err, Info, Accent, Neutral }

/// The tone for a process state from the daemon.
pub fn state_tone(state: &str) -> Tone {
    match state {
        "running" => Tone::Ok,
        "starting" | "backoff" => Tone::Warn,
        "failed" => Tone::Err,
        _ => Tone::Neutral,
    }
}

pub fn state_label(state: &str) -> &'static str {
    match state {
        "running" => "Running",
        "starting" => "Starting",
        "backoff" => "Restarting",
        "failed" => "Failed",
        "stopped" => "Stopped",
        _ => "Stopped",
    }
}

// ------------------------------------------------------------------ text and icons

pub fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg().path(SharedString::from(format!("icons/{name}.svg"))).size(px(size)).flex_none().text_color(color)
}

pub fn t(s: impl Into<SharedString>) -> Div { div().child(s.into()) }

pub fn muted(p: &Pal, s: impl Into<SharedString>) -> Div { div().text_size(px(14.)).text_color(p.text3).child(s.into()) }

pub fn sub(p: &Pal, s: impl Into<SharedString>) -> Div { div().text_size(px(15.5)).text_color(p.text2).child(s.into()) }

pub fn mono(p: &Pal, s: impl Into<SharedString>) -> Div {
    div().font_family(MONO).text_size(px(12.5)).text_color(p.text2).child(s.into())
}

pub fn h1(p: &Pal, s: impl Into<SharedString>) -> Div {
    div().text_size(px(30.)).font_weight(FontWeight::NORMAL).text_color(p.text).child(s.into())
}

pub fn h2(p: &Pal, s: impl Into<SharedString>) -> Div {
    div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(s.into())
}

pub fn strong(p: &Pal, s: impl Into<SharedString>) -> Div {
    div().font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(s.into())
}

pub fn label_caps(p: &Pal, s: impl Into<SharedString>) -> Div {
    div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(p.text3).child(s.into())
}

// ------------------------------------------------------------------ buttons

#[derive(Clone, Copy, PartialEq)]
pub enum Kind { Primary, Secondary, Ghost, Danger }

pub fn button(id: impl Into<ElementId>, p: &Pal, kind: Kind, small: bool, icon_name: Option<&str>, label: Option<&str>) -> Stateful<Div> {
    let clear = gpui_kit::transparent_black();
    let (bg, fg, border, hover) = match kind {
        Kind::Primary => (p.accent, p.bg, clear, p.text2),
        Kind::Secondary => (p.surface, p.text, p.line2, p.surface2),
        Kind::Ghost => (clear, p.text2, clear, p.surface2),
        Kind::Danger => (p.surface, p.red, p.line2, p.red_soft),
    };
    let h = if small { 30. } else { 36. };
    let mut d = div().id(id).flex().flex_row().flex_none().items_center().justify_center().gap(px(7.)).h(px(h))
        .rounded_full().border_1().border_color(border).bg(bg).text_color(fg)
        .text_size(px(if small { 13.5 } else { 14.5 })).font_weight(FontWeight::MEDIUM).whitespace_nowrap()
        .cursor_pointer().hover(move |s| s.bg(hover));
    d = if label.is_some() { d.px(px(if small { 12. } else { 16. })) } else { d.w(px(h)) };
    if let Some(i) = icon_name { d = d.child(icon(i, 14., fg)); }
    if let Some(l) = label { d = d.child(SharedString::from(l.to_string())); }
    d
}

// ------------------------------------------------------------------ status

pub fn dot(color: Hsla) -> Div { div().size(px(7.)).rounded_full().bg(color).flex_none() }

pub fn pill(p: &Pal, tone: Tone, label: impl Into<SharedString>, with_dot: bool) -> Div {
    let (fg, bg) = p.tone(tone);
    div().flex().flex_row().flex_none().items_center().gap(px(6.)).h(px(22.)).px(px(8.)).rounded_full().bg(bg)
        .text_color(fg).text_size(px(12.)).font_weight(FontWeight::MEDIUM).whitespace_nowrap()
        .when(with_dot, |d| d.child(dot(fg)))
        .child(label.into())
}

pub fn chip(p: &Pal, label: impl Into<SharedString>) -> Div {
    div().flex().flex_none().items_center().h(px(22.)).px(px(7.)).rounded(px(6.)).border_1().border_color(p.line)
        .bg(p.surface2).font_family(MONO).text_size(px(11.5)).text_color(p.text2).whitespace_nowrap().child(label.into())
}

/// A rounded square holding a letter or an icon, tinted by tone.
pub fn tile(p: &Pal, tone: Tone, size: f32, letter: Option<&str>, icon_name: Option<&str>) -> Div {
    let (fg, bg) = p.tone(tone);
    let mut d = div().size(px(size)).flex_none().rounded(px(size * 0.27)).bg(bg).flex().items_center().justify_center()
        .text_color(fg).font_weight(FontWeight::SEMIBOLD).text_size(px(size * 0.42));
    if let Some(l) = letter { d = d.child(SharedString::from(l.to_uppercase())); }
    if let Some(i) = icon_name { d = d.child(icon(i, size * 0.48, fg)); }
    d
}

/// The tone a site's tile gets, so each site keeps its own colour.
pub fn site_tone(name: &str) -> Tone {
    let h: u32 = name.bytes().map(|b| b as u32).sum();
    [Tone::Accent, Tone::Info, Tone::Ok, Tone::Warn][(h % 4) as usize]
}

// ------------------------------------------------------------------ surfaces

/// A section: no box, a title with a hairline under it, rows separated by hairlines.
pub fn card(_p: &Pal) -> Div { div().flex().flex_col() }

pub fn card_head(p: &Pal, title: &str) -> Div {
    div().flex().flex_row().flex_wrap().items_center().gap(px(12.)).px(px(18.)).pb(px(14.)).pt(px(4.)).border_b_1().border_color(p.line)
        .child(h2(p, title.to_string()))
}

/// Children separated by hairlines.
pub fn rows(p: &Pal, items: Vec<AnyElement>) -> Div {
    let line = p.line;
    div().flex().flex_col().children(items.into_iter().enumerate().map(move |(i, e)| {
        div().when(i > 0, |d| d.border_t_1().border_color(line)).child(e)
    }))
}

/// A setting: label and help on the left, the control on the right.
pub fn field(p: &Pal, label: &str, help: Option<String>, mono_label: bool, control: impl IntoElement) -> Div {
    div().flex().flex_row().flex_wrap().items_center().gap(px(20.)).px(px(18.)).py(px(15.))
        .child(div().flex_1().min_w(px(220.)).flex().flex_col().gap(px(2.))
            .child(div().text_size(px(if mono_label { 13.5 } else { 15. })).font_weight(FontWeight::MEDIUM).text_color(p.text)
                .when(mono_label, |d| d.font_family(MONO)).child(label.to_string()))
            .when_some(help, |d, h| d.child(muted(p, h))))
        .child(div().w(px(320.)).flex().flex_row().justify_end().child(control))
}

pub fn switch(id: impl Into<ElementId>, p: &Pal, on: bool) -> Stateful<Div> {
    div().id(id).w(px(40.)).h(px(24.)).flex_none().rounded_full().p(px(3.)).flex().flex_row().items_center().cursor_pointer()
        .bg(if on { p.green } else { p.line2 })
        .when(on, |d| d.justify_end())
        .child(div().size(px(18.)).rounded_full().bg(white()).shadow_sm())
}

pub fn banner(p: &Pal, tone: Tone, icon_name: &str, title: &str, body: &str) -> Div {
    let (fg, bg) = p.tone(tone);
    div().flex().flex_row().flex_wrap().items_center().gap(px(12.)).pl(px(16.)).pr(px(14.)).py(px(12.))
        .rounded(px(12.)).border_1().border_color(p.line).bg(p.surface)
        .child(div().size(px(30.)).flex_none().rounded(px(8.)).bg(bg).flex().items_center().justify_center().child(icon(icon_name, 16., fg)))
        .child(div().flex_1().min_w(px(240.)).flex().flex_col().gap(px(2.))
            .child(div().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).text_color(p.text).child(title.to_string()))
            .child(div().text_size(px(13.)).text_color(p.text3).child(body.to_string())))
}

/// A log or code block; colours tez markers, errors, warnings and finished jobs.
pub fn code_block(p: &Pal, text: &str, max_lines: usize) -> Div {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    let pp = *p;
    div().bg(p.code).border_1().border_color(p.line).rounded(px(10.)).px(px(14.)).py(px(12.)).font_family(MONO)
        .text_size(px(12.)).line_height(px(19.)).text_color(p.text2).overflow_hidden().flex().flex_col()
        .when(lines.is_empty(), |d| d.child(div().text_color(pp.text3).child("Nothing here yet.")))
        .children(lines[start..].iter().map(move |l| {
            let low = l.to_ascii_lowercase();
            let color = if l.starts_with("====") { pp.text3 }
                else if low.contains("error") || low.contains("exception") || low.contains("fatal") || low.contains("failed") { pp.red }
                else if low.contains("warn") { pp.amber }
                else if low.contains(" done") || low.contains("success") || l.contains("✓") { pp.green }
                else { pp.text2 };
            div().text_color(color).child(if l.is_empty() { " ".to_string() } else { l.to_string() })
        }))
}

/// Segmented control; returns the row, `on_pick` gets the chosen value.
pub fn seg(id: &str, p: &Pal, options: &[(String, String)], current: &str, on_pick: Rc<dyn Fn(String, &mut App)>) -> Div {
    seg_with(id, p, options, current, &[], on_pick)
}

/// A segmented control whose `disabled` values show but cannot be picked.
pub fn seg_with(id: &str, p: &Pal, options: &[(String, String)], current: &str, disabled: &[String], on_pick: Rc<dyn Fn(String, &mut App)>) -> Div {
    let pp = *p;
    div().flex().flex_row().flex_none().p(px(2.)).rounded(px(9.)).bg(p.surface2).border_1().border_color(p.line).gap(px(2.))
        .children(options.iter().map(|(value, label)| {
            let on = value == current;
            let off = disabled.contains(value);
            let v = value.clone();
            let pick = on_pick.clone();
            div().id(SharedString::from(format!("{id}-{value}"))).h(px(28.)).px(px(12.)).rounded(px(7.)).flex().items_center()
                .text_size(px(12.5)).font_weight(FontWeight::MEDIUM)
                .text_color(if on { pp.text } else if off { pp.text3 } else { pp.text2 })
                .when(on, |d| d.bg(pp.surface).shadow_sm())
                .when(!off, |d| d.cursor_pointer().hover(move |s| s.text_color(pp.text)).on_click(move |_, _, cx| pick(v.clone(), cx)))
                .child(label.clone())
        }))
}

pub fn tabs(id: &str, p: &Pal, options: &[(String, String)], current: &str, on_pick: Rc<dyn Fn(String, &mut App)>) -> Div {
    let pp = *p;
    div().flex().flex_row().gap(px(4.)).border_b_1().border_color(p.line)
        .children(options.iter().map(|(value, label)| {
            let on = value == current;
            let v = value.clone();
            let pick = on_pick.clone();
            div().id(SharedString::from(format!("{id}-{value}"))).h(px(38.)).px(px(12.)).flex().items_center()
                .text_size(px(13.5)).font_weight(FontWeight::MEDIUM).cursor_pointer().border_b_2()
                .border_color(if on { pp.accent_hi } else { gpui_kit::transparent_black() })
                .text_color(if on { pp.text } else { pp.text2 })
                .hover(move |s| s.text_color(pp.text))
                .child(label.clone())
                .on_click(move |_, _, cx| pick(v.clone(), cx))
        }))
}

// ------------------------------------------------------------------ inputs

struct InputHolder {
    input: Entity<InputState>,
    /// The value last pushed in from outside, so edits are not overwritten until the source changes.
    last: String,
    _sub: Subscription,
}

/// A text input kept across renders by `key`. `on_change` runs on every edit (typed changes
/// are queued and saved after a pause by the caller).
pub fn text_input(window: &mut Window, cx: &mut App, key: &str, value: &str, placeholder: &str, masked: bool, on_change: Option<Rc<dyn Fn(String, &mut App)>>) -> Entity<InputState> {
    let initial = value.to_string();
    let ph = placeholder.to_string();
    let holder = window.use_keyed_state(SharedString::from(format!("input:{key}")), cx, move |window, cx| {
        let start = initial.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(start).placeholder(ph).masked(masked));
        let sub = cx.subscribe(&input, move |_, input, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                if let Some(f) = &on_change { let v = input.read(cx).value().to_string(); f(v, cx); }
            }
        });
        InputHolder { input, last: initial, _sub: sub }
    });
    let v = value.to_string();
    holder.update(cx, |h, cx| {
        if h.last != v {
            h.last = v.clone();
            if h.input.read(cx).value() != v.as_str() {
                h.input.update(cx, |i, cx| i.set_value(v.clone(), window, cx));
            }
        }
    });
    holder.read(cx).input.clone()
}

pub fn input_el(state: &Entity<InputState>, mono_font: bool) -> Input {
    let i = Input::new(state);
    if mono_font { i.font_family(MONO) } else { i }
}

// ------------------------------------------------------------------ helpers

pub fn mb(b: u64) -> String {
    if b >= 1 << 30 { format!("{:.1} GB", b as f64 / (1u64 << 30) as f64) } else { format!("{} MB", b >> 20) }
}

pub fn ago(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} m", secs / 60),
        3600..=86399 => format!("{} h {} m", secs / 3600, (secs % 3600) / 60),
        _ => format!("{} d", secs / 86400),
    }
}

/// A card: one thing you control. Header (who, status, switch top-right), cost strip, quiet actions.
pub fn cardbox(p: &Pal) -> Div { div().flex().flex_col().bg(p.card).border_1().border_color(p.cardline).rounded(px(16.)).overflow_hidden() }

/// A quiet text button for a card's footer.
pub fn text_btn(id: impl Into<ElementId>, p: &Pal, label: &str) -> Stateful<Div> {
    let pp = *p;
    div().id(id).px(px(10.)).py(px(6.)).rounded(px(8.)).text_size(px(14.)).text_color(p.text2).cursor_pointer()
        .hover(move |s| s.bg(pp.surface2).text_color(pp.text)).child(label.to_string())
}

/// The card's cost strip: equal cells with a value over a label.
pub fn stat_strip(p: &Pal, cells: Vec<(String, &'static str, Option<Hsla>)>) -> Div {
    let pp = *p;
    div().flex().flex_row().border_t_1().border_b_1().border_color(p.cardline)
        .children(cells.into_iter().enumerate().map(move |(i, (v, l, c))| div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(2.)).px(px(16.)).py(px(12.))
            .when(i > 0, |d| d.border_l_1().border_color(pp.cardline))
            .child(div().text_size(px(18.)).text_color(c.unwrap_or(pp.text)).truncate().child(v))
            .child(div().text_size(px(12.5)).text_color(pp.text3).child(l))))
}