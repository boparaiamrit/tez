//! The window: title bar (menus, back and forward, window buttons), the icon rail, the list
//! column and the inset content panel. Also the Home dashboard, a site's Overview and the
//! Settings sheet, as specified on the design canvas ("What each page answers").

use crate::pages;
use crate::ui::*;
use crate::{api, busy_on, do_remove, flip_proc, flip_service, flip_site, go, go_back, go_forward, job, local, notify, open_remove, refresh_now, restart_all, send, set_local, set_setting, set_theme, tez, txt, Busy, Page, Tez};
use std::collections::BTreeMap;
use std::time::Duration;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{json, Value};
use std::cell::Cell;
use std::rc::Rc;

type Pick = Rc<dyn Fn(String, &mut App)>;

fn sid(s: impl Into<String>) -> ElementId { ElementId::Name(SharedString::from(s.into())) }
fn arr(v: &Value) -> Vec<Value> { v.as_array().cloned().unwrap_or_default() }
fn cap(s: &str) -> String { let mut c = s.chars(); match c.next() { Some(f) => f.to_uppercase().collect::<String>() + c.as_str(), None => String::new() } }
fn ms(v: f64) -> String { if v >= 1000. { format!("{:.1} s", v / 1000.) } else { format!("{} ms", v.round() as i64) } }

#[derive(Clone, Copy, PartialEq)]
enum Section { Sites, Services, Php, Debug }

fn section(page: &Page) -> Section {
    match page {
        Page::Services => Section::Services,
        Page::Php | Page::Node => Section::Php,
        Page::Dumps | Page::Mail | Page::Logs => Section::Debug,
        _ => Section::Sites,
    }
}

// ------------------------------------------------------------------ the frame

/// What a right-click on a sidebar row offers.
fn ctx_items(kind: &str, cx: &App) -> Vec<MenuItem> {
    let t = tez(cx);
    let (what, name) = kind.split_once(':').unwrap_or((kind, ""));
    let name = name.to_string();
    match what {
        "site" => {
            let s = arr(&t.status["sites"]).into_iter().find(|s| txt(&s["name"]) == name).unwrap_or(Value::Null);
            let (url, path) = (txt(&s["url"]), txt(&s["path"]));
            let up = s["up"].as_bool().unwrap_or(false);
            let (u1, u2, p1, p2, p3, n1, n2) = (url.clone(), url.clone(), path.clone(), path.clone(), path.clone(), name.clone(), name.clone());
            vec![
                mi("Open in browser", move |_, _| api::open_url(&u1)),
                mi("Open folder", move |_, _| api::open_path(&p1)),
                mi("Open terminal here", move |_, _| api::open_terminal(&p2)),
                mi("Open in editor", move |_, _| api::open_editor(&p3)),
                mi("Copy address", move |_, cx| { cx.write_to_clipboard(ClipboardItem::new_string(u2.clone())); notify(cx, "Address copied".to_string()); }),
                MenuItem { sep_before: true, ..mi(if up { "Stop its processes" } else { "Start its processes" }, move |_, cx| flip_site(cx, &n1, !up)) },
                MenuItem { sep_before: true, ..mi("Remove site…", move |_, cx| open_remove(cx, n2.clone())) },
            ]
        }
        "svc" => {
            let on = arr(&t.status["procs"]).iter().any(|x| txt(&x["name"]) == name && x["wanted"] != false);
            let title = arr(&t.services["services"]).iter().find(|s| txt(&s["name"]) == name).map(|s| txt(&s["title"])).filter(|x| !x.is_empty()).unwrap_or_else(|| name.clone());
            let (n1, n2, n3, n4) = (name.clone(), name.clone(), name.clone(), name.clone());
            vec![
                mi("Details", move |_, cx| { cx.global_mut::<Tez>().svc_sel = n1.clone(); go(cx, Page::Services); }),
                mi(if on { "Stop" } else { "Start" }, move |_, cx| flip_service(cx, &n2, &title, !on)),
                mi("Restart", move |_, cx| send(cx, format!("/api/proc/{n3}/restart"), json!({}), "Restarting")),
                MenuItem { sep_before: true, ..mi("Show its log", move |_, cx| { cx.global_mut::<Tez>().log_sel = n4.clone(); go(cx, Page::Logs); }) },
            ]
        }
        "php" => {
            let ini = std::path::PathBuf::from(txt(&t.status["home"])).join("php").join(&name).join("php.ini").to_string_lossy().to_string();
            let (m1, m2, m3) = (name.clone(), name.clone(), name.clone());
            vec![
                mi("Details", move |_, cx| { cx.global_mut::<Tez>().php_sel = m1.clone(); go(cx, Page::Php); }),
                mi("Make default", move |_, cx| set_setting(cx, "default_php", json!(m2.clone()), "Default PHP changed")),
                mi("Open php.ini", move |_, _| api::open_editor(&ini)),
                MenuItem { sep_before: true, ..mi("Restart workers", move |_, cx| send(cx, format!("/api/proc/php-{m3}/restart"), json!({}), "Restarting PHP")) },
            ]
        }
        _ => vec![],
    }
}

/// The right-click menu, at the pointer.
fn ctx_menu(p: &Pal, at: Point<Pixels>, kind: &str, cx: &App) -> AnyElement {
    let items = ctx_items(kind, cx);
    let pp = *p;
    let menu = div().occlude().min_w(px(220.)).p(px(4.)).rounded(px(8.)).bg(p.surface2).border_1().border_color(p.line2).shadow_lg().flex().flex_col()
        .on_mouse_down_out(|_, _, cx| { cx.global_mut::<Tez>().ctx = None; cx.refresh_windows(); })
        .children(items.into_iter().enumerate().map(move |(i, it)| {
            let run = it.run.clone();
            div().flex().flex_col()
                .when(it.sep_before && i > 0, |d| d.child(div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(pp.line2)))
                .child(div().id(sid(format!("cm-{i}"))).flex().flex_row().items_center().h(px(30.)).px(px(10.)).rounded(px(5.)).text_size(px(13.))
                    .hover(move |s| s.bg(pp.surface3))
                    .child(div().flex_1().child(it.label.clone()))
                    .on_click(move |_, window, cx| { cx.global_mut::<Tez>().ctx = None; run(window, cx); cx.refresh_windows(); }))
        }));
    deferred(anchored().position(at).snap_to_window().child(menu)).with_priority(3).into_any_element()
}

/// Remove a site: from tez only, or with its folder to the Recycle Bin, and optionally its database.
/// Anything that cannot be undone from tez (the folder, the database) needs the name typed back.
fn remove_dialog(p: &Pal, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = tez(cx);
    let name = t.del_site.clone().unwrap_or_default();
    let pv = t.del_preview.clone();
    let (files, db, busy) = (t.del_files, t.del_db, t.del_busy);
    let pp = *p;
    let loaded = !pv.is_null();
    let err = txt(&pv["error"]);
    let path = txt(&pv["path"]);
    let linked = pv["kind"] == "linked";
    let trash = txt(&pv["trash"]);
    let trash_ok = !trash.is_empty();
    let bin = if trash_ok { trash.clone() } else if cfg!(target_os = "macos") || cfg!(target_os = "linux") { "Trash".into() } else { "Recycle Bin".into() };
    let procs: Vec<String> = arr(&pv["processes"]).iter().map(txt).collect();
    let dbv = pv["database"].clone();
    let db_ok = dbv["local"].as_bool() == Some(true) && dbv["exists"].as_bool() == Some(true);
    let db_name = txt(&dbv["name"]);
    let size = pv["size_bytes"].as_u64().unwrap_or(0);
    let needs_name = files || db;
    let typed = local(cx, "del-confirm");
    let typed_ok = typed.trim() == name;
    let can = loaded && err.is_empty() && !busy && (!needs_name || typed_ok);

    let option = |id: &'static str, on: bool, enabled: bool, title: String, body: String, pick: bool| {
        div().id(id).flex().flex_row().items_start().gap(px(10.)).p(px(12.)).rounded(px(8.)).border_1()
            .border_color(if on { pp.accent_hi } else { pp.line2 }).bg(if on { pp.accent_soft } else { pp.surface })
            .when(!enabled, |d| d.opacity(0.5))
            .child(div().mt(px(2.)).size(px(16.)).flex_none().rounded_full().border_1().border_color(if on { pp.text } else { pp.line2 }).flex().items_center().justify_center()
                .when(on, |d| d.child(div().size(px(8.)).rounded_full().bg(pp.text))))
            .child(div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(2.))
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .child(div().text_size(px(13.)).text_color(pp.text3).child(body)))
            .when(enabled, |d| d.on_click(move |_, _, cx| { let t = cx.global_mut::<Tez>(); t.del_files = pick; cx.refresh_windows(); }))
    };

    let mut body = div().flex().flex_col().gap(px(12.));
    if !loaded {
        body = body.child(skeleton_rows(p, 3, 44.));
    } else if !err.is_empty() {
        body = body.child(banner(p, Tone::Err, "triangle-alert", "tez could not look at this site", &err));
    } else {
        let keep_text = if linked { format!("Unlink it and stop its processes. The folder stays at {path}.") } else { format!("Stop serving it and stop its processes. The folder stays at {path}; Settings > General can serve it again.") };
        let amount = if size < (1 << 20) { "Less than 1 MB".to_string() } else { format!("{}{}", mb(size), if pv["size_partial"].as_bool() == Some(true) { " or more" } else { "" }) };
        let bin_text = if trash_ok { format!("{amount} goes to the {bin}, where you can restore it. Nothing is deleted outright.") }
            else { format!("Not possible here: {}", txt(&pv["trash_blocked"])) };
        body = body
            .when(!procs.is_empty(), |d| d.child(div().text_size(px(13.)).text_color(pp.text2).child(format!("Stops {} first.", procs.join(", ")))))
            .when(pv["git_dirty"].as_bool() == Some(true), |d| d.child(banner(&pp, Tone::Warn, "git-branch", "Unsaved git work", &format!("{}. It goes wherever the folder goes.", txt(&pv["git_detail"])))))
            .child(option("rm-keep", !files, true, if linked { "Unlink from tez".into() } else { "Remove from tez".into() }, keep_text, false))
            .child(option("rm-files", files, trash_ok, format!("Also move the folder to the {bin}"), bin_text, true));
        if db_ok {
            body = body.child(div().id("rm-db").flex().flex_row().items_center().gap(px(10.)).px(px(2.))
                .child(div().size(px(16.)).flex_none().rounded(px(4.)).border_1().border_color(if db { pp.red } else { pp.line2 }).bg(if db { pp.red } else { pp.surface }).flex().items_center().justify_center()
                    .when(db, |d| d.child(icon("check", 12., pp.bg))))
                .child(div().flex_1().min_w(px(0.)).text_size(px(13.5)).child(format!("Also drop the database {db_name} and {db_name}_testing. This cannot be undone.")))
                .on_click(|_, _, cx| { let t = cx.global_mut::<Tez>(); t.del_db = !t.del_db; cx.refresh_windows(); }));
        } else if !txt(&dbv["note"]).is_empty() {
            body = body.child(div().text_size(px(13.)).text_color(pp.text3).child(format!("Database {db_name}: {}.", txt(&dbv["note"]))));
        }
        if needs_name {
            let input = text_input(window, cx, "del-confirm", &typed, "Site name", false, Some(Rc::new(|v, cx| set_local(cx, "del-confirm", v))));
            body = body.child(div().flex().flex_col().gap(px(6.))
                .child(div().text_size(px(13.)).text_color(pp.text2).child(format!("Type {name} to confirm")))
                .child(input_el(&input, true)));
        }
    }
    let action = if busy { "Removing…".to_string() } else if files { format!("Move to {bin}") } else if linked { "Unlink".into() } else { "Remove".into() };
    div().absolute().top_0().left_0().size_full().occlude().flex().items_center().justify_center()
        .child(div().absolute().top_0().left_0().size_full().bg(rgba(0x0000008c)))
        .child(div().id("rm-dialog").occlude().w(px(540.)).max_h(relative(0.9)).p(px(20.)).flex().flex_col().gap(px(14.)).bg(pp.bg).border_1().border_color(pp.line2).rounded(px(12.)).shadow_lg()
            .child(div().flex().flex_col().gap(px(2.))
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(format!("Remove {name}")))
                .when(!path.is_empty(), |d| d.child(div().font_family(MONO).text_size(px(12.)).text_color(pp.text3).truncate().child(path.clone()))))
            .child(body)
            .child(div().pt(px(4.)).flex().flex_row().justify_end().gap(px(8.))
                .child(button("rm-cancel", &pp, Kind::Secondary, false, None, Some("Cancel")).on_click(|_, _, cx| { cx.global_mut::<Tez>().del_site = None; cx.refresh_windows(); }))
                .child(button("rm-go", &pp, Kind::Danger, false, if files { Some("trash-2") } else { None }, Some(&action))
                    .when(!can, |d| d.opacity(0.45))
                    .when(can, |d| d.on_click(|_, _, cx| do_remove(cx))))))
        .into_any_element()
}

/// Shown the first time the window is closed: tez keeps running, and where to find it.
fn close_note(p: &Pal) -> AnyElement {
    let place = if cfg!(target_os = "macos") { "the menu bar" } else { "the tray, by the clock" };
    div().absolute().top_0().left_0().size_full().occlude().flex().items_center().justify_center()
        .child(div().absolute().top_0().left_0().size_full().bg(rgba(0x0000008c)))
        .child(div().id("close-note").occlude().w(px(420.)).p(px(20.)).flex().flex_col().gap(px(10.)).bg(p.bg).border_1().border_color(p.line2).rounded(px(12.)).shadow_lg()
            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child("tez keeps running"))
            .child(div().text_size(px(13.5)).text_color(p.text2).child(format!("Your sites and services keep working. tez stays in {place}: click its icon to open this window again, or right-click it and choose Quit.")))
            .child(div().pt(px(6.)).flex().flex_row().justify_end().gap(px(8.))
                .child(button("close-quit", p, Kind::Secondary, true, None, Some("Quit tez app")).on_click(|_, window, cx| crate::close_window(window, cx, true)))
                .child(button("close-ok", p, Kind::Primary, true, None, Some("Close window")).on_click(|_, window, cx| crate::close_window(window, cx, false)))))
        .into_any_element()
}

pub fn root(window: &mut Window, cx: &mut App) -> AnyElement {
    let p = pal(cx);
    let t = tez(cx);
    let side_hidden = t.side_hidden;
    let settings_open = t.settings_open;
    let jump = t.jump;
    let close_hint = t.close_hint;
    let removing = t.del_site.is_some();
    let ctx = t.ctx.clone();
    let toast_el = toasts(&p, cx);
    let title = titlebar(&p, cx);
    let rail_el = rail(&p, cx);
    let side = if side_hidden { None } else { Some(side_list(&p, cx)) };
    let panel_el = panel(&p, window, cx);
    let sheet = if settings_open { Some(settings_sheet(&p, window, cx)) } else { None };
    let jump_el = if jump { Some(pages::jump_overlay(&p, window, cx)) } else { None };
    let close_note_el = if close_hint { Some(close_note(&p)) } else { None };
    let remove_el = if removing { Some(remove_dialog(&p, window, cx)) } else { None };
    let ctx_el = ctx.map(|(at, kind)| ctx_menu(&p, at, &kind, cx));
    div().size_full().relative().flex().flex_col().bg(if window_material() { p.glass } else { p.side }).text_color(p.text).font_family(sans()).text_size(px(BODY))
        .child(title)
        .child(div().flex_1().min_h(px(0.)).flex().flex_row().child(rail_el).when_some(side, |d, s| d.child(s)).child(panel_el))
        .when_some(sheet, |d, s| d.child(s))
        .when_some(jump_el, |d, j| d.child(j))
        .when_some(close_note_el, |d, c| d.child(c))
        .when_some(remove_el, |d, c| d.child(c))
        .when_some(ctx_el, |d, c| d.child(c))
        .when_some(toast_el, |d, t| d.child(t))
        .into_any_element()
}

/// Bottom right: a line with a spinner for each change still going, then what just finished.
/// Quick changes skip the progress line. Clicking the result dismisses it.
fn toasts(p: &Pal, cx: &App) -> Option<Div> {
    let t = tez(cx);
    let mut items: Vec<AnyElement> = Vec::new();
    let mut going: Vec<(&String, &Busy)> = t.busy.iter().filter(|(_, b)| b.since.elapsed() > Duration::from_millis(300)).collect();
    going.sort_by_key(|(_, b)| b.since);
    for (k, b) in going {
        items.push(toast_box(p, format!("tb-{k}"), spinner(SharedString::from(format!("ts-{k}")), 16., p.accent_text).into_any_element(), format!("{}…", b.doing)).into_any_element());
    }
    if let Some(m) = &t.message {
        let (fg, _) = p.tone(m.tone);
        let lead = match m.tone {
            Tone::Ok => icon("circle-check", 16., fg),
            Tone::Err => icon("circle-alert", 16., fg),
            Tone::Warn => icon("triangle-alert", 16., fg),
            _ => icon("info", 16., p.text3),
        };
        items.push(toast_box(p, format!("tm-{}", m.id), lead.into_any_element(), m.text.clone())
            .cursor_pointer().on_click(|_, _, cx| { cx.global_mut::<Tez>().message = None; cx.refresh_windows(); }).into_any_element());
    }
    if items.is_empty() { return None; }
    Some(div().absolute().bottom(px(20.)).right(px(20.)).flex().flex_col().items_end().gap(px(8.)).children(items))
}

fn toast_box(p: &Pal, id: String, lead: AnyElement, text: String) -> Stateful<Div> {
    let body = div().flex().flex_row().items_center().gap(px(10.)).min_w(px(260.)).max_w(px(420.)).pl(px(12.)).pr(px(16.)).py(px(10.))
        .rounded(px(10.)).bg(p.surface).border_1().border_color(p.line2).shadow_lg().text_size(px(13.5)).text_color(p.text)
        .child(lead).child(div().flex_1().min_w(px(0.)).child(text));
    // Rise and fade in once, when it first appears.
    div().id(sid(id.clone())).child(body.with_animation(SharedString::from(format!("{id}-in")), Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint()),
        |d, t| d.opacity(t).mt(px(8. * (1. - t)))))
}

fn icon_btn(id: impl Into<ElementId>, p: &Pal, name: &str, dim: bool, round: bool) -> Stateful<Div> {
    let pp = *p;
    div().id(id).size(px(34.)).flex_none().flex().items_center().justify_center()
        .rounded(if round { px(17.) } else { px(9.) }).when(round, |d| d.bg(pp.surface2))
        .hover(move |s| s.bg(pp.surface3))
        .child(icon(name, 18., if dim { p.text3 } else { p.text2 }))
}

fn titlebar(p: &Pal, cx: &App) -> Div {
    let t = tez(cx);
    let can_back = !t.back.is_empty();
    let can_fwd = !t.fwd.is_empty();
    let open = t.menu_open.clone();
    let pp = *p;
    let mut bar = div().h(px(44.)).flex_none().flex().flex_row().items_center().gap(px(4.)).pl(px(if cfg!(target_os = "macos") { 82. } else { 10. }))
        .child(icon_btn("tb-side", p, "panel-left", false, false).on_click(|_, _, cx| { let t = cx.global_mut::<Tez>(); t.side_hidden = !t.side_hidden; cx.refresh_windows(); }))
        .child(icon_btn("tb-back", p, "chevron-left", !can_back, false).on_click(|_, _, cx| go_back(cx)))
        .child(icon_btn("tb-fwd", p, "chevron-right", !can_fwd, false).on_click(|_, _, cx| go_forward(cx)));
    if !cfg!(target_os = "macos") {
        bar = bar.child(div().w(px(10.)));
        for (key, label) in [("file", "File"), ("edit", "Edit"), ("view", "View"), ("help", "Help")] {
            let is_open = open.as_deref() == Some(key);
            let k = key.to_string();
            let mut m = div().flex().flex_col()
                .child(div().id(sid(format!("menu-{key}"))).px(px(10.)).py(px(4.)).rounded(px(6.)).text_size(px(14.))
                    .text_color(if is_open { pp.text } else { pp.text2 }).when(is_open, |d| d.bg(pp.surface2))
                    .hover(move |s| s.bg(pp.surface2).text_color(pp.text))
                    .child(label)
                    .on_click(move |_, _, cx| { let t = cx.global_mut::<Tez>(); t.menu_open = if t.menu_open.as_deref() == Some(k.as_str()) { None } else { Some(k.clone()) }; cx.refresh_windows(); }));
            if is_open { m = m.child(deferred(anchored().snap_to_window().child(menu_popover(p, key, cx))).with_priority(2)); }
            bar = bar.child(m);
        }
    }
    // Only the empty stretch drags the window; buttons and menus keep their clicks.
    bar = bar.child(div().id("tb-drag").flex_1().h_full().window_control_area(WindowControlArea::Drag)
        .when(!cfg!(windows), |d| d.on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())));
    if cfg!(windows) {
        let wc = |id: &str, ic: &str, area: WindowControlArea, close: bool| {
            div().id(sid(id.to_string())).w(px(46.)).h_full().flex().items_center().justify_center().window_control_area(area)
                .hover(move |s| if close { s.bg(rgb(0xc42b1c)) } else { s.bg(pp.surface2) })
                .child(icon(ic, 15., pp.text2))
        };
        bar = bar.child(wc("wc-min", "minus", WindowControlArea::Min, false))
            .child(wc("wc-max", "square", WindowControlArea::Max, false))
            .child(wc("wc-close", "x", WindowControlArea::Close, true));
    }
    bar
}

struct MenuItem { label: String, hint: Option<String>, check: bool, sep_before: bool, run: Rc<dyn Fn(&mut Window, &mut App)> }

fn mi(label: &str, run: impl Fn(&mut Window, &mut App) + 'static) -> MenuItem { MenuItem { label: label.into(), hint: None, check: false, sep_before: false, run: Rc::new(run) } }

fn menu_popover(p: &Pal, key: &str, cx: &App) -> Div {
    let t = tez(cx);
    let online = t.online;
    let theme = if t.theme.is_empty() { "system".to_string() } else { t.theme.clone() };
    let parked = arr(&t.settings["parked"]).first().map(txt).unwrap_or_default();
    let mut items: Vec<MenuItem> = match key {
        "file" => vec![
            MenuItem { hint: Some(keys("Mod+N")), ..mi("New site", |_, cx| go(cx, Page::NewSite)) },
            mi("Import a project", |_, cx| { cx.global_mut::<Tez>().new_source = "import".into(); go(cx, Page::NewSite) }),
            mi("Open the Sites folder", move |_, _| api::open_path(&parked)),
            MenuItem { sep_before: true, ..mi(if online { "Restart the web server and PHP" } else { "Start tez" }, move |_, cx| if online { restart_all(cx) } else { api::start_daemon(); notify(cx, "Starting tez") }) },
            mi("Stop tez", |_, cx| send(cx, "/api/shutdown".into(), json!({}), "Stopping tez")),
            MenuItem { sep_before: true, ..mi("Close window", |window, _| window.remove_window()) },
            MenuItem { hint: Some(keys("Mod+Q")), ..mi("Quit tez", |_, cx| cx.quit()) },
        ],
        "edit" => vec![
            MenuItem { hint: Some(keys("Mod+K")), ..mi("Search or jump to", |_, cx| { cx.global_mut::<Tez>().jump = true; cx.refresh_windows(); }) },
            MenuItem { hint: Some(keys("Mod+,")), ..mi("Settings", |_, cx| { cx.global_mut::<Tez>().settings_open = true; cx.refresh_windows(); }) },
        ],
        "view" => {
            let mut v = vec![
                MenuItem { hint: Some(keys("Mod+1")), ..mi("Home", |_, cx| go(cx, Page::Overview)) },
                MenuItem { hint: Some(keys("Mod+2")), ..mi("Sites", |_, cx| go(cx, Page::Sites)) },
                MenuItem { hint: Some(keys("Mod+3")), ..mi("Services", |_, cx| go(cx, Page::Services)) },
                MenuItem { hint: Some(keys("Mod+4")), ..mi("PHP", |_, cx| go(cx, Page::Php)) },
                mi("Node.js", |_, cx| go(cx, Page::Node)),
                MenuItem { hint: Some(keys("Mod+5")), ..mi("Dumps", |_, cx| go(cx, Page::Dumps)) },
                mi("Mail", |_, cx| go(cx, Page::Mail)),
                mi("Logs", |_, cx| go(cx, Page::Logs)),
                MenuItem { sep_before: true, hint: Some(keys("Mod+B")), ..mi("Show or hide the sidebar", |_, cx| { let t = cx.global_mut::<Tez>(); t.side_hidden = !t.side_hidden; cx.refresh_windows(); }) },
            ];
            for (i, (val, label)) in [("system", "Match the system"), ("dark", "Dark"), ("light", "Light")].into_iter().enumerate() {
                let v2 = val.to_string();
                v.push(MenuItem { check: theme == val, sep_before: i == 0, ..mi(label, move |_, cx| set_theme(cx, v2.clone())) });
            }
            v
        }
        "status" => vec![
            mi(if online { "Restart the web server and PHP" } else { "Start tez" }, move |_, cx| if online { restart_all(cx) } else { api::start_daemon(); notify(cx, "Starting tez") }),
            mi("Stop tez", |_, cx| send(cx, "/api/shutdown".into(), json!({}), "Stopping tez")),
            MenuItem { sep_before: true, ..mi("Web dashboard", |_, _| api::open_url(&format!("http://127.0.0.1:{}", api::port()))) },
            mi("Settings", |_, cx| { cx.global_mut::<Tez>().settings_open = true; cx.refresh_windows(); }),
            MenuItem { sep_before: true, ..mi("Quit tez", |_, cx| cx.quit()) },
        ],
        _ => vec![
            mi("Documentation", |_, _| api::open_url("https://github.com/boparaiamrit/tez#readme")),
            mi("Releases", |_, _| api::open_url("https://github.com/boparaiamrit/tez/releases")),
            mi("Web dashboard", |_, _| api::open_url(&format!("http://127.0.0.1:{}", api::port()))),
        ],
    };
    if key == "help" {
        let version = txt(&t.status["version"]);
        items.push(MenuItem { sep_before: true, hint: if version.is_empty() { None } else { Some(version) }, ..mi("About tez", |_, cx| notify(cx, "tez: fast native Laravel development")) });
    }
    let pp = *p;
    let k = key.to_string();
    div().occlude().mt(px(if key == "status" { 4. } else { 36. })).min_w(px(240.)).p(px(4.)).rounded(px(8.)).bg(p.surface2).border_1().border_color(p.line2).shadow_lg().flex().flex_col()
        .on_mouse_down_out(move |_, _, cx| { let t = cx.global_mut::<Tez>(); if t.menu_open.as_deref() == Some(k.as_str()) { t.menu_open = None; cx.refresh_windows(); } })
        .children(items.into_iter().enumerate().map(move |(i, it)| {
            let run = it.run.clone();
            div().flex().flex_col()
                .when(it.sep_before && i > 0, |d| d.child(div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(pp.line2)))
                .child(div().id(sid(format!("mi-{i}"))).flex().flex_row().items_center().gap(px(12.)).h(px(30.)).px(px(10.)).rounded(px(5.))
                    .text_size(px(13.)).hover(move |s| s.bg(pp.surface3))
                    .child(div().flex_1().child(it.label.clone()))
                    .when_some(it.hint.clone(), |d, h| d.child(div().text_size(px(13.)).text_color(pp.text3).child(h)))
                    .when(it.check, |d| d.child(icon("check", 15., pp.text)))
                    .on_click(move |_, window, cx| { cx.global_mut::<Tez>().menu_open = None; run(window, cx); cx.refresh_windows(); }))
        }))
}

fn rail(p: &Pal, cx: &App) -> Div {
    let t = tez(cx);
    let sec = section(&t.page);
    let home = t.page == Page::Overview;
    let new_dumps = t.dumps.len().saturating_sub(t.dumps_seen) > 0;
    let online = t.online;
    let status_open = t.menu_open.as_deref() == Some("status");
    let pp = *p;
    let item = move |id: &str, ic: &str, on: bool, badge: bool, target: Page| {
        div().id(sid(format!("rail-{id}"))).size(px(42.)).rounded(px(12.)).flex().items_center().justify_center().relative()
            .when(on, |d| d.bg(pp.rail_on).shadow_sm()).when(!on, |d| d.hover(move |s| s.bg(pp.rail_hover)))
            .child(icon(ic, 21., if on { pp.text } else { pp.text3 }))
            .when(badge, |d| d.child(div().absolute().top(px(8.)).right(px(8.)).size(px(8.)).rounded_full().bg(pp.text)))
            .on_click(move |_, _, cx| go(cx, target.clone()))
    };
    let mut status = div().flex().flex_col()
        .child(div().id("rail-status").size(px(42.)).rounded(px(12.)).flex().items_center().justify_center()
            .hover(move |s| s.bg(pp.rail_hover))
            .child(div().size(px(20.)).rounded_full().border_2().border_color(if online { pp.green } else { pp.text3 }).flex().items_center().justify_center()
                .child(div().size(px(8.)).rounded_full().bg(if online { pp.green } else { pp.text3 })))
            .on_click(|_, _, cx| { let t = cx.global_mut::<Tez>(); t.menu_open = if t.menu_open.as_deref() == Some("status") { None } else { Some("status".into()) }; cx.refresh_windows(); }));
    if status_open { status = status.child(deferred(anchored().snap_to_window().child(menu_popover(p, "status", cx))).with_priority(2)); }
    // One step deeper than the sidebar, with a hairline, so the two columns read apart.
    div().w(px(60.)).flex_none().flex().flex_col().items_center().gap(px(6.)).pt(px(8.)).pb(px(12.))
        .bg(p.rail).border_r_1().border_color(p.rail_line)
        .child(item("home", "house", home, false, Page::Overview))
        .child(item("sites", "globe", !home && sec == Section::Sites, false, Page::Sites))
        .child(item("services", "database", sec == Section::Services, false, Page::Services))
        .child(item("php", "code-xml", sec == Section::Php, false, Page::Php))
        .child(item("debug", "bug", sec == Section::Debug, new_dumps, Page::Dumps))
        .child(div().flex_1())
        .child(div().id("rail-settings").size(px(42.)).rounded(px(12.)).flex().items_center().justify_center()
            .hover(move |s| s.bg(pp.rail_hover)).child(icon("settings", 21., p.text3))
            .on_click(|_, _, cx| { cx.global_mut::<Tez>().settings_open = true; cx.refresh_windows(); }))
        .child(status)
}

fn side_item(id: String, p: &Pal, ic: Option<&str>, label: String, end: Option<String>, on: bool, sub: bool, target: Page) -> Stateful<Div> {
    let pp = *p;
    div().id(sid(id)).flex().flex_row().items_center().gap(px(10.)).h(px(if sub { 28. } else { 32. })).pl(px(if sub { 38. } else { 10. })).pr(px(10.))
        .rounded(px(6.)).text_size(px(if sub { 13. } else { BODY })).text_color(if sub { pp.text2 } else { pp.text })
        .when(on, |d| d.bg(pp.surface3)).hover(move |s| s.bg(pp.surface2))
        .when_some(ic.map(|s| s.to_string()), |d, i| d.child(icon(&i, 16., pp.text2)))
        .child(div().flex_1().truncate().child(label))
        .when_some(end, |d, e| d.child(div().text_size(px(13.)).text_color(pp.text3).child(e)))
        .on_click(move |_, _, cx| go(cx, target.clone()))
}

fn side_sec(p: &Pal, label: &str) -> Div { div().px(px(10.)).pt(px(14.)).pb(px(4.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text3).child(label.to_string()) }

fn side_list(p: &Pal, cx: &App) -> Div {
    let t = tez(cx);
    let sec = section(&t.page);
    let page = t.page.clone();
    let procs = arr(&t.status["procs"]);
    let sm = t.site_metrics["sites"].clone();
    let title = match sec { Section::Sites => "Sites", Section::Services => "Services", Section::Php => "PHP and Node.js", Section::Debug => "Debug" };
    let mut list = div().id("side-scroll").flex_1().min_h(px(0.)).overflow_y_scroll().flex().flex_col().gap(px(1.));
    match sec {
        Section::Sites => {
            list = list.child(side_item("si-new".into(), p, Some("square-pen"), "New site".into(), None, page == Page::NewSite, false, Page::NewSite));
            let sites = arr(&t.status["sites"]);
            let (running, rest): (Vec<Value>, Vec<Value>) = sites.into_iter().partition(|s| procs.iter().any(|x| txt(&x["group"]) == format!("site:{}", txt(&s["name"])) && x["state"] == "running"));
            let perf = |name: &str| -> Option<String> {
                let m = &sm[name];
                if m.is_null() { return None; }
                let rpm = m["rpm"].as_f64().unwrap_or(0.);
                if rpm <= 0.0 { return None; }
                m["p95_ms"].as_f64().map(ms)
            };
            if !running.is_empty() {
                list = list.child(side_sec(p, "Running"));
                for s in &running {
                    let name = txt(&s["name"]);
                    let ctx_kind = format!("site:{name}");
                    list = list.child(side_item(format!("si-{name}"), p, Some("folder"), name.clone(), perf(&name).or(Some(format!("PHP {}", txt(&s["php"])))), page == Page::Site(name.clone()), false, Page::Site(name.clone()))
                        .on_mouse_down(MouseButton::Right, move |ev, _, cx| { cx.global_mut::<Tez>().ctx = Some((ev.position, ctx_kind.clone())); cx.refresh_windows(); }));
                    for x in procs.iter().filter(|x| txt(&x["group"]) == format!("site:{name}")) {
                        let short = txt(&x["name"]).rsplit('/').next().unwrap_or_default().to_string();
                        let end = if x["state"] == "running" { x["memory"].as_u64().map(mb) } else { Some(state_label(&txt(&x["state"])).to_lowercase()) };
                        list = list.child(side_item(format!("si-{name}-{short}"), p, None, cap(&short), end, false, true, Page::Site(name.clone())));
                    }
                }
            }
            if !rest.is_empty() {
                list = list.child(side_sec(p, if running.is_empty() { "Sites" } else { "Other sites" }));
                for s in &rest {
                    let name = txt(&s["name"]);
                    let ctx_kind = format!("site:{name}");
                    list = list.child(side_item(format!("si-{name}"), p, Some("folder"), name.clone(), perf(&name).or(Some(format!("PHP {}", txt(&s["php"])))), page == Page::Site(name.clone()), false, Page::Site(name.clone()))
                        .on_mouse_down(MouseButton::Right, move |ev, _, cx| { cx.global_mut::<Tez>().ctx = Some((ev.position, ctx_kind.clone())); cx.refresh_windows(); }));
                }
            }
        }
        Section::Services => {
            let svcs = arr(&t.services["services"]);
            let sel = t.svc_sel.clone();
            let pp = *p;
            let row = |s: &Value, installed: bool| {
                let name = txt(&s["name"]);
                let st = procs.iter().find(|x| txt(&x["name"]) == name).map(|x| txt(&x["state"])).unwrap_or_default();
                let changing = t.busy.contains_key(&format!("svc:{name}")) || st == "stopping";
                let end = if installed { s["port"].as_u64().map(|x| format!(":{x}")) } else { Some("add".into()) };
                let n2 = name.clone();
                let ctx_kind = format!("svc:{name}");
                div().id(sid(format!("sv-{name}"))).flex().flex_row().items_center().gap(px(10.)).h(px(32.)).px(px(10.)).rounded(px(6.))
                    .when(installed, |d| d.on_mouse_down(MouseButton::Right, move |ev, _, cx| { cx.global_mut::<Tez>().ctx = Some((ev.position, ctx_kind.clone())); cx.refresh_windows(); }))
                    .when(name == sel, |d| d.bg(pp.surface3)).hover(move |s| s.bg(pp.surface2))
                    .child(if changing { spinner(sid(format!("svsp-{name}")), 12., pp.amber).into_any_element() } else if installed { dot(if st == "running" { pp.green } else { pp.text3 }).into_any_element() } else { icon("plus", 15., pp.text3).into_any_element() })
                    .child(div().flex_1().truncate().text_color(if installed { pp.text } else { pp.text2 }).child(txt(&s["title"])))
                    .when_some(end, |d, e| d.child(div().text_size(px(13.)).text_color(pp.text3).child(e)))
                    .on_click(move |_, _, cx| { cx.global_mut::<Tez>().svc_sel = n2.clone(); go(cx, Page::Services); })
            };
            let installed: Vec<&Value> = svcs.iter().filter(|s| s["installed"] == true).collect();
            if !installed.is_empty() { list = list.child(side_sec(p, "Installed")); }
            for s in installed { list = list.child(row(s, true)); }
            let more: Vec<&Value> = svcs.iter().filter(|s| s["installed"] != true && s["available"] != false).collect();
            if !more.is_empty() { list = list.child(side_sec(p, "Add a service")); }
            for s in more { list = list.child(row(s, false)); }
        }
        Section::Php => {
            let sel = t.php_sel.clone();
            let pp = *p;
            list = list.child(side_sec(p, "PHP"));
            for v in arr(&t.php["versions"]) {
                let minor = txt(&v["minor"]);
                let users = arr(&v["sites"]).len();
                let m2 = minor.clone();
                let on = minor == sel || (sel.is_empty() && v["default"] == true);
                let ctx_kind = format!("php:{minor}");
                list = list.child(div().id(sid(format!("pv-{minor}"))).flex().flex_row().items_center().gap(px(10.)).h(px(32.)).px(px(10.)).rounded(px(6.))
                    .when(on, |d| d.bg(pp.surface3)).hover(move |s| s.bg(pp.surface2))
                    .on_mouse_down(MouseButton::Right, move |ev, _, cx| { cx.global_mut::<Tez>().ctx = Some((ev.position, ctx_kind.clone())); cx.refresh_windows(); })
                    .child(icon("code-xml", 16., pp.text2))
                    .child(div().flex_1().child(format!("PHP {}", txt(&v["version"]))))
                    .child(div().text_size(px(13.)).text_color(pp.text3).child(if v["default"] == true { "default".to_string() } else if users > 0 { format!("{users} site{}", if users == 1 { "" } else { "s" }) } else { String::new() }))
                    .on_click(move |_, _, cx| { cx.global_mut::<Tez>().php_sel = m2.clone(); go(cx, Page::Php); }));
            }
            node_side(&mut list, p, t, &page);
        }
        Section::Debug => {
            let nd = t.dumps.len().saturating_sub(t.dumps_seen);
            let unread = t.mail["unread"].as_u64().unwrap_or(0);
            list = list.child(side_item("dg-dumps".into(), p, Some("bug"), "Dumps".into(), if nd > 0 { Some(format!("{nd} new")) } else { None }, page == Page::Dumps, false, Page::Dumps))
                .child(side_item("dg-mail".into(), p, Some("mail"), "Mail".into(), if unread > 0 { Some(format!("{unread} unread")) } else { None }, page == Page::Mail, false, Page::Mail))
                .child(side_item("dg-logs".into(), p, Some("scroll-text"), "Logs".into(), None, page == Page::Logs, false, Page::Logs));
        }
    }
    div().w(px(256.)).flex_none().flex().flex_col().pl(px(4.)).pr(px(10.)).pb(px(12.))
        .child(div().flex().flex_row().items_center().gap(px(6.)).pl(px(10.)).pr(px(4.)).pb(px(8.))
            .child(div().flex_1().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(icon_btn("side-search", p, "search", false, false).on_click(|_, _, cx| { cx.global_mut::<Tez>().jump = true; cx.refresh_windows(); })))
        .child(list)
}

fn panel(p: &Pal, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = tez(cx);
    let page = t.page.clone();
    let online = t.online && !t.preview_offline;
    let pp = *p;
    // The switch at the top of the panel, and the buttons on its right.
    let (seg_el, right): (Option<Div>, Vec<AnyElement>) = match &page {
        Page::Overview => {
            let r = if t.range.is_empty() { "1h".to_string() } else { t.range.clone() };
            (Some(pill_seg("range", p, &[("15m", "15 min"), ("1h", "1 hour"), ("24h", "24 hours")], &r, Rc::new(|v, cx| { cx.global_mut::<Tez>().range = v; refresh_now(cx, false); cx.refresh_windows(); }))),
             vec![icon_btn("pt-restart", p, "refresh-cw", false, true).on_click(|_, _, cx| restart_all(cx)).into_any_element()])
        }
        Page::Site(name) => {
            let tab = if t.site_tab.is_empty() || t.site_tab == "processes" { "overview".to_string() } else { t.site_tab.clone() };
            let s = arr(&t.status["sites"]).into_iter().find(|s| txt(&s["name"]) == *name).unwrap_or(Value::Null);
            let (url, path, path2) = (txt(&s["url"]), txt(&s["path"]), txt(&s["path"]));
            let rm_name = name.clone();
            (Some(pill_seg("sitetab", p, &[("overview", "Overview"), ("environment", "Environment"), ("domains", "Domains"), ("config", "Config"), ("logs", "Logs")], &tab, Rc::new(|v, cx| { cx.global_mut::<Tez>().site_tab = v; refresh_now(cx, false); cx.refresh_windows(); }))),
             vec![
                icon_btn("pt-open", p, "arrow-up-right", false, true).on_click(move |_, _, _| api::open_url(&url)).into_any_element(),
                icon_btn("pt-term", p, "terminal", false, false).on_click(move |_, _, _| api::open_terminal(&path)).into_any_element(),
                icon_btn("pt-code", p, "code-xml", false, false).on_click(move |_, _, _| api::open_editor(&path2)).into_any_element(),
                icon_btn("pt-remove", p, "trash-2", false, false).on_click(move |_, _, cx| open_remove(cx, rm_name.clone())).into_any_element(),
             ])
        }
        _ => (None, vec![]),
    };
    let body: AnyElement = if !online {
        pages::offline(p, cx)
    } else {
        match page {
            Page::Overview => home(p, cx),
            Page::Sites => { let n = arr(&tez(cx).status["sites"]).len(); div().flex().flex_col().gap(px(22.)).child(div().flex().flex_col().gap(px(8.)).child(h1(p, "Sites")).child(sub(p, format!("{n} site{} from your Sites folders, each at name.test and name.localhost. Right-click one for more.", if n == 1 { "" } else { "s" })))).child(site_cards(p, cx, false)).into_any_element() }
            Page::Site(name) => site_view(p, &name, window, cx),
            Page::NewSite => pages::new_site(p, window, cx),
            Page::Services => pages::services_page(p, window, cx),
            Page::Php => pages::php_page(p, window, cx),
            Page::Node => pages::node_page(p, cx),
            Page::Dumps => pages::dumps_page(p, cx),
            Page::Mail => pages::mail_page(p, cx),
            Page::Logs => pages::logs_page(p, cx),
            Page::Settings => { cx.global_mut::<Tez>().settings_open = true; home(p, cx) }
        }
    };
    div().flex_1().min_w(px(0.)).mr(px(8.)).mb(px(8.)).rounded(px(10.)).bg(pp.bg).border_1().border_color(pp.line).flex().flex_col().overflow_hidden()
        .child(div().h(px(48.)).flex_none().relative().flex().items_center().justify_center()
            .when_some(seg_el, |d, s| d.child(s))
            .child(div().absolute().right(px(12.)).top(px(8.)).flex().flex_row().gap(px(6.)).children(right)))
        .child(div().id(sid(format!("panel-scroll-{:?}-{}", tez(cx).page, tez(cx).side_hidden))).flex_1().min_h(px(0.)).overflow_y_scroll().track_scroll(&tez(cx).panel_scroll)
            .child(div().w_full().px(px(32.)).pt(px(8.)).pb(px(40.)).flex().flex_col().items_start()
                .child(div().w_full().max_w(px(1120.)).flex().flex_col().gap(px(22.)).child(body))))
        .into_any_element()
}

/// The rounded switch at the top of the panel.
fn pill_seg(id: &str, p: &Pal, options: &[(&str, &str)], current: &str, on_pick: Pick) -> Div {
    let pp = *p;
    div().flex().flex_row().p(px(2.)).gap(px(2.)).rounded(px(8.)).bg(p.surface2).border_1().border_color(p.line)
        .children(options.iter().map(|(v, l)| {
            let on = *v == current;
            let pick = on_pick.clone();
            let vv = v.to_string();
            div().id(sid(format!("{id}-{v}"))).h(px(28.)).px(px(14.)).rounded(px(6.)).flex().items_center()
                .text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(if on { pp.text } else { pp.text2 })
                .when(on, |d| d.bg(pp.surface3).shadow_sm())
                .child(l.to_string())
                .on_click(move |_, _, cx| pick(vv.clone(), cx))
        }))
}

// ------------------------------------------------------------------ charts

/// Stacked areas (memory by group): soft bands with a clean line on top of each, round gridlines.
fn stacked_chart(p: &Pal, rows: Vec<[f64; 4]>, scale: f64, ticks: Vec<f64>, hover: Option<f32>, cell: Rc<Cell<Option<Bounds<Pixels>>>>) -> impl IntoElement {
    let pp = *p;
    let c2 = cell.clone();
    let n = rows.len();
    canvas(move |b, _, _| b, move |b, _, window, _| {
        c2.set(Some(b));
        let (x0, y0, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
        for t in &ticks {
            let y = y0 + h - (*t / scale) as f32 * h;
            window.paint_quad(fill(Bounds::new(point(px(x0), px(y)), size(px(w), px(1.))), pp.line));
        }
        window.paint_quad(fill(Bounds::new(point(px(x0), px(y0 + h - 1.)), size(px(w), px(1.))), pp.line2));
        if n < 2 { return; }
        let xs: Vec<f32> = (0..n).map(|i| x0 + w * i as f32 / (n - 1) as f32).collect();
        let mut below = vec![y0 + h; n];
        for k in 0..4 {
            let top: Vec<f32> = (0..n).map(|i| below[i] - (rows[i][k] / scale) as f32 * h).collect();
            let mut pb = PathBuilder::fill();
            pb.move_to(point(px(xs[0]), px(top[0])));
            for i in 1..n { pb.line_to(point(px(xs[i]), px(top[i]))); }
            for i in (0..n).rev() { pb.line_to(point(px(xs[i]), px(below[i]))); }
            pb.close();
            if let Ok(path) = pb.build() { window.paint_path(path, pp.series[k].opacity(0.28)); }
            let mut sb = PathBuilder::stroke(px(1.5));
            sb.move_to(point(px(xs[0]), px(top[0])));
            for i in 1..n { sb.line_to(point(px(xs[i]), px(top[i]))); }
            if let Ok(path) = sb.build() { window.paint_path(path, pp.series[k]); }
            below = top;
        }
        if let Some(f) = hover {
            let x = x0 + w * f;
            window.paint_quad(fill(Bounds::new(point(px(x), px(y0)), size(px(1.), px(h))), pp.text3));
        }
    }).size_full()
}
/// Lines (response time p50 and p95), values in ms.
fn line_chart(p: &Pal, lines: Vec<(Vec<f64>, Hsla)>, max: f64) -> impl IntoElement {
    let pp = *p;
    let max = max.max(1.);
    canvas(move |b, _, _| b, move |b, _, window, _| {
        let (x0, y0, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
        window.paint_quad(fill(Bounds::new(point(px(x0), px(y0 + h * 0.5)), size(px(w), px(1.))), pp.line));
        window.paint_quad(fill(Bounds::new(point(px(x0), px(y0 + h - 1.)), size(px(w), px(1.))), pp.line2));
        for (vals, color) in &lines {
            let n = vals.len();
            if n < 2 { continue; }
            let mut pb = PathBuilder::stroke(px(2.));
            for (i, v) in vals.iter().enumerate() {
                let pt = point(px(x0 + w * i as f32 / (n - 1) as f32), px(y0 + h - (*v / max) as f32 * h));
                if i == 0 { pb.move_to(pt) } else { pb.line_to(pt) }
            }
            if let Ok(path) = pb.build() { window.paint_path(path, *color); }
        }
    }).size_full()
}

fn sparkline(p: &Pal, vals: Vec<f64>) -> impl IntoElement {
    let color = p.text3;
    let max = vals.iter().copied().fold(0., f64::max).max(1.);
    let min = vals.iter().copied().fold(f64::MAX, f64::min).min(max);
    canvas(move |b, _, _| b, move |b, _, window, _| {
        let n = vals.len();
        if n < 2 { return; }
        let (x0, y0, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
        let span = (max - min).max(max * 0.05).max(1.);
        let mut pb = PathBuilder::stroke(px(1.5));
        for (i, v) in vals.iter().enumerate() {
            let pt = point(px(x0 + w * i as f32 / (n - 1) as f32), px(y0 + h - ((*v - min) / span) as f32 * (h - 2.) - 1.));
            if i == 0 { pb.move_to(pt) } else { pb.line_to(pt) }
        }
        if let Ok(path) = pb.build() { window.paint_path(path, color); }
    }).w(px(64.)).h(px(18.))
}

// ------------------------------------------------------------------ home

fn group_title(p: &Pal, title: &str, note: Option<String>) -> Div {
    div().flex().flex_row().items_baseline().gap(px(12.)).px(px(18.)).pb(px(14.)).border_b_1().border_color(p.line)
        .child(h2(p, title.to_string()))
        .when_some(note, |d, n| d.child(div().text_size(px(14.)).text_color(p.text3).child(n)))
}

fn attention_row(p: &Pal, title: &str, body: &str, tone: Tone, action: Option<AnyElement>) -> AnyElement {
    let (fg, _) = p.tone(tone);
    div().flex().flex_row().items_center().gap(px(16.)).px(px(18.)).py(px(15.)).border_b_1().border_color(p.line)
        .child(dot(fg))
        .child(div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(2.)).child(strong(p, title.to_string())).child(muted(p, body.to_string())))
        .when_some(action, |d, a| d.child(a))
        .into_any_element()
}

fn pool_name(name: &str) -> String {
    // php-8.4-3 -> "PHP 8.4 pool"; acmeshop/horizon -> "Horizon · acmeshop"
    if let Some(rest) = name.strip_prefix("php-") { if let Some((v, _)) = rest.rsplit_once('-') { return format!("PHP {v} pool"); } }
    if let Some((site, proc_)) = name.split_once('/') { return format!("{} · {site}", cap(proc_)); }
    match name { "web" => "Web server".into(), "postgres" => "PostgreSQL".into(), "redis" => "Redis".into(), "mailpit" => "Mailpit".into(), "meilisearch" => "Meilisearch".into(), "rustfs" => "RustFS".into(), n => cap(n) }
}

fn home(p: &Pal, cx: &App) -> AnyElement {
    let t = tez(cx);
    let procs = arr(&t.status["procs"]);
    let m = t.metrics.clone();
    let machine = m["machine"].clone();
    let running = procs.iter().filter(|x| x["state"] == "running").count();
    let tez_mem: u64 = procs.iter().filter_map(|x| x["memory"].as_u64()).sum();
    let cpu: f64 = procs.iter().filter_map(|x| x["cpu"].as_f64()).sum();
    let has_cpu = procs.iter().any(|x| x["cpu"].is_number());
    let failing: Vec<Value> = procs.iter().filter(|x| x["state"] == "failed" || x["state"] == "backoff").cloned().collect();
    let notes: Vec<String> = arr(&t.status["notes"]).iter().map(txt).collect();
    let trusted = t.settings["ca_trusted"].as_bool().unwrap_or(true);
    let wsl = machine["wsl"].as_u64().unwrap_or(0);
    let disk_free = machine["disk_free"].as_u64().unwrap_or(t.settings["free_disk"].as_u64().unwrap_or(0));
    let cache = machine["cache"].as_u64().unwrap_or(0);
    let pp = *p;

    let mut out = div().flex().flex_col().gap(px(32.))
        .child(div().flex().flex_col().gap(px(8.)).child(h1(p, "This computer"))
            .child(sub(p, if has_cpu { format!("tez runs {running} processes using {} of memory and {cpu:.0}% of the CPU", mb(tez_mem)) } else { format!("tez runs {running} processes using {} of memory", mb(tez_mem)) })));

    // Needs attention: only real problems.
    let mut att: Vec<AnyElement> = Vec::new();
    for x in &failing {
        let name = txt(&x["name"]);
        let (n1, n2) = (name.clone(), name.clone());
        att.push(attention_row(p, &format!("{} keeps stopping", pool_name(&name)), &format!("{} restarts. The log says why.", x["restarts"].as_u64().unwrap_or(0)), Tone::Err,
            Some(div().flex().flex_row().gap(px(8.))
                .child(button(sid(format!("att-log-{name}")), p, Kind::Secondary, true, None, Some("Show log")).on_click(move |_, _, cx| { cx.global_mut::<Tez>().log_sel = n1.clone(); go(cx, Page::Logs); }))
                .child(button(sid(format!("att-rs-{name}")), p, Kind::Ghost, true, None, Some("Try again")).on_click(move |_, _, cx| send(cx, format!("/api/proc/{n2}/restart"), json!({}), "Restarting")))
                .into_any_element())));
    }
    if wsl > 0 {
        let armed = local(cx, "wsl-confirm") == "1";
        att.push(attention_row(p, &format!("WSL is still holding {}", mb(wsl)), "Nothing in tez needs it. Shutting it down stops everything running inside WSL and frees port 80.", Tone::Warn,
            Some(button("att-wsl", p, if armed { Kind::Danger } else { Kind::Secondary }, true, None, Some(if armed { "Click again to shut down WSL" } else { "Shut down WSL" }))
                .on_click(move |_, _, cx| {
                    if local(cx, "wsl-confirm") == "1" { set_local(cx, "wsl-confirm", String::new()); send(cx, "/api/wsl/shutdown".into(), json!({}), "Shutting down WSL"); }
                    else { set_local(cx, "wsl-confirm", "1".into()); }
                }).into_any_element())));
    }
    for n in &notes {
        let body = { let mut c = n.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() };
        att.push(attention_row(p, if n.contains("port 80") { "Plain HTTP is not on port 80" } else { "Heads up" }, &body, Tone::Warn, None));
    }
    if !trusted {
        att.push(attention_row(p, "Local HTTPS is not trusted", "Browsers warn until you trust tez's certificate. One admin prompt.", Tone::Warn,
            Some(button("att-trust", p, Kind::Primary, true, None, Some("Trust")).on_click(|_, _, cx| job(cx, &["trust"], "Trust local HTTPS", "Asking for permission")).into_any_element())));
    }
    if disk_free > 0 && disk_free < 20 * (1 << 30) {
        att.push(attention_row(p, &format!("Only {} free on the tez drive", mb(disk_free)), "tez stops downloads before 2 GB; Windows slows down under 20 GB.", Tone::Warn,
            if cache > 0 { Some(button("att-cache", p, Kind::Secondary, true, None, Some(&format!("Clear {} cache", mb(cache)))).on_click(|_, _, cx| send(cx, "/api/cache/clear".into(), json!({}), "Cleared the download cache")).into_any_element()) } else { None }));
    }
    if !att.is_empty() {
        out = out.child(cardbox(p).border_color(pp.amber.opacity(0.35)).children(att));
    }

    // This computer: CPU and RAM over time for the whole machine, with tez's share inside.
    let series = arr(&m["series"]);
    let first = series.iter().position(|s| s["mem_used"].as_f64().unwrap_or(0.) > 0.).unwrap_or(series.len());
    let series: Vec<Value> = series[first..].to_vec();
    let sys_cpu: Vec<f64> = series.iter().map(|s| s["sys_cpu"].as_f64().unwrap_or(0.)).collect();
    let tez_cpu: Vec<f64> = series.iter().map(|s| s["cpu"].as_f64().unwrap_or(0.)).collect();
    let used: Vec<f64> = series.iter().map(|s| s["mem_used"].as_f64().unwrap_or(0.)).collect();
    let tez_used: Vec<f64> = series.iter().map(|s| ["php", "sites", "services", "web"].iter().map(|k| s[*k].as_f64().unwrap_or(0.)).sum()).collect();
    let range = if t.range.is_empty() { "1h".to_string() } else { t.range.clone() };
    let range_label = match range.as_str() { "15m" => "last 15 minutes", "24h" => "last 24 hours", _ => "last hour" };
    let total = machine["total"].as_u64().unwrap_or(0);
    let avail = machine["available"].as_u64().unwrap_or(0);
    let used_now = total.saturating_sub(avail);
    let cpu_now = machine["cpu"].as_f64().unwrap_or(0.);
    let cores = machine["cpus"].as_u64().unwrap_or(0);
    let since = series.first().and_then(|s| s["t"].as_u64()).map(|t0| format!("{} ago", ago(m["now"].as_u64().unwrap_or(t0).saturating_sub(t0)))).unwrap_or_default();
    let legend2 = |a: &str, ca: Hsla, b: &str, cb: Hsla| div().flex().flex_row().gap(px(18.)).px(px(18.)).pt(px(4.)).text_size(px(13.5)).text_color(pp.text2)
        .child(div().flex().flex_row().items_center().gap(px(8.)).child(div().w(px(14.)).h(px(2.)).bg(ca)).child(a.to_string()))
        .child(div().flex().flex_row().items_center().gap(px(8.)).child(div().w(px(14.)).h(px(2.)).bg(cb)).child(b.to_string()));
    let chart = |lines: Vec<(Vec<f64>, Hsla)>, max: f64, ticks: Vec<(f64, String)>| -> AnyElement {
        if lines.first().map(|l| l.0.len() < 2).unwrap_or(true) {
            return div().h(px(150.)).mx(px(18.)).flex().items_center().justify_center().child(muted(&pp, "Collecting samples; the graph fills in within a minute.")).into_any_element();
        }
        div().relative().h(px(150.)).mx(px(18.)).mt(px(16.)).mb(px(26.))
            .child(div().absolute().left(px(56.)).top_0().bottom_0().right_0().child(line_chart(&pp, lines, max)))
            .children(ticks.into_iter().map(|(v, l)| div().absolute().left_0().w(px(52.)).top(relative(1. - (v / max) as f32)).mt(px(-9.)).text_size(px(12.)).text_color(pp.text3).child(l)))
            .child(div().absolute().left(px(56.)).bottom(px(-22.)).text_size(px(12.)).text_color(pp.text3).child(since.clone()))
            .child(div().absolute().right_0().bottom(px(-22.)).text_size(px(12.)).text_color(pp.text3).child("now"))
            .into_any_element()
    };
    let head = |title: &str, sub_t: String, big: String, small: String| div().flex().flex_row().items_start().px(px(18.)).pt(px(16.))
        .child(div().flex_1().flex().flex_col().gap(px(2.)).child(strong(&pp, title.to_string())).child(muted(&pp, sub_t)))
        .child(div().flex().flex_col().items_end().gap(px(2.)).child(div().text_size(px(24.)).child(big)).child(muted(&pp, small)));
    // Where the CPU goes right now, the same way the Memory card splits RAM: tez, other apps, idle.
    let tez_now = cpu.clamp(0., 100.);
    let cpu_segs = [(tez_now, pp.series[0], "tez"), ((cpu_now - tez_now).max(0.), pp.line2, "Other apps"), ((100. - cpu_now.max(tez_now)).max(0.), pp.surface3, "Idle")];
    let cpu_card = cardbox(p).pb(px(16.))
        .child(head("CPU", format!("{cores} cores · {range_label}"), format!("{cpu_now:.0}%"), format!("tez {cpu:.1}%")))
        .child(chart(vec![(sys_cpu, pp.text2), (tez_cpu, pp.series[0])], 100., vec![(25., "25%".into()), (50., "50%".into()), (75., "75%".into()), (100., "100%".into())]))
        .child(legend2("This computer", pp.text2, "tez", pp.series[0]))
        .when(cores > 0, |d| d.child(div().mt_auto().flex().flex_col().gap(px(8.)).px(px(18.)).pt(px(14.))
            .child(div().flex().flex_row().h(px(8.)).rounded(px(4.)).overflow_hidden().gap(px(2.))
                .children(cpu_segs.iter().filter(|(v, _, _)| *v > 0.05).map(|(v, c, _)| div().h_full().w(relative(*v as f32 / 100.)).bg(*c))))
            .child(div().flex().flex_row().flex_wrap().gap(px(18.)).text_size(px(13.5)).text_color(pp.text2)
                .children(cpu_segs.iter().map(|(v, c, l)| div().flex().flex_row().items_center().gap(px(6.)).child(div().size(px(8.)).rounded(px(2.)).bg(*c)).child(format!("{l} {v:.1}%")))))));
    let gib = (1u64 << 30) as f64;
    let ram_max = (total as f64).max(gib);
    let step = if ram_max > 48. * gib { 16. * gib } else if ram_max > 24. * gib { 8. * gib } else { 4. * gib };
    let ram_ticks: Vec<(f64, String)> = (1..).map(|k| k as f64 * step).take_while(|v| *v <= ram_max + 1.).map(|v| (v, mb(v as u64))).collect();
    let other = used_now.saturating_sub(tez_mem).saturating_sub(wsl);
    let segs = [(tez_mem, pp.series[0], "tez"), (wsl, pp.amber, "WSL"), (other, pp.line2, "Other apps"), (avail, pp.surface3, "Free")];
    let ram_card = cardbox(p).pb(px(16.))
        .child(head("Memory (RAM)", format!("{} in all · {range_label}", mb(total)), format!("{} used", mb(used_now)), format!("{} free", mb(avail))))
        .child(chart(vec![(used, pp.text2), (tez_used, pp.series[0])], ram_max, ram_ticks))
        .child(legend2("Used by everything", pp.text2, "tez", pp.series[0]))
        .when(total > 0, |d| d.child(div().mt_auto().flex().flex_col().gap(px(8.)).px(px(18.)).pt(px(14.))
            .child(div().flex().flex_row().h(px(8.)).rounded(px(4.)).overflow_hidden().gap(px(2.))
                .children(segs.iter().filter(|(v, _, _)| *v > 0).map(|(v, c, _)| div().h_full().w(relative(*v as f32 / total as f32)).bg(*c))))
            .child(div().flex().flex_row().flex_wrap().gap(px(18.)).text_size(px(13.5)).text_color(pp.text2)
                .children(segs.iter().filter(|(v, _, _)| *v > 0).map(|(v, c, l)| div().flex().flex_row().items_center().gap(px(6.)).child(div().size(px(8.)).rounded(px(2.)).bg(*c)).child(format!("{l} {}", mb(*v))))))));
    out = out.child(grid_of(vec![cpu_card.into_any_element(), ram_card.into_any_element()], 2));

    // What inside tez costs the most, and the disk.
    let mut pools: Vec<(String, u64, f64)> = Vec::new();
    for x in &procs {
        let key = pool_name(&txt(&x["name"]));
        let (mem, c) = (x["memory"].as_u64().unwrap_or(0), x["cpu"].as_f64().unwrap_or(0.));
        match pools.iter_mut().find(|(k, _, _)| *k == key) { Some(e) => { e.1 += mem; e.2 += c; } None => pools.push((key, mem, c)) }
    }
    pools.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal).then(b.1.cmp(&a.1)));
    let consumers = cardbox(p).pb(px(6.))
        .child(div().flex().flex_row().items_center().px(px(18.)).pt(px(16.)).pb(px(10.)).child(div().flex_1().child(strong(p, "Busiest in tez"))).child(div().w(px(70.)).flex().justify_end().child(muted(p, "CPU"))).child(div().w(px(80.)).flex().justify_end().child(muted(p, "RAM"))))
        .children(pools.into_iter().take(6).map(|(name, mem, c)| div().flex().flex_row().items_center().px(px(18.)).py(px(9.)).border_t_1().border_color(pp.cardline).text_size(px(13.5))
            .child(div().flex_1().min_w(px(0.)).truncate().child(name))
            .child(div().w(px(70.)).flex().justify_end().text_color(if c > 25. { pp.amber } else { pp.text }).child(format!("{c:.1}%")))
            .child(div().w(px(80.)).flex().justify_end().text_color(if mem > 1 << 30 { pp.amber } else { pp.text }).child(mb(mem)))));
    let disk_total = machine["disk_total"].as_u64().unwrap_or(0);
    let disk_card = cardbox(p).pb(px(10.))
        .child(div().flex().flex_col().gap(px(2.)).px(px(18.)).pt(px(16.)).pb(px(8.)).child(strong(p, "Disk")).child(muted(p, "the drive that holds tez and your sites")))
        .when(disk_total > 0, |d| d.child(div().mx(px(18.)).my(px(6.)).h(px(8.)).rounded(px(4.)).bg(pp.surface3).overflow_hidden()
            .child(div().h_full().w(relative(1. - disk_free as f32 / disk_total as f32)).bg(if disk_free < 20 * (1 << 30) { pp.amber } else { pp.text2 }))))
        .child(div().flex().flex_row().justify_between().px(px(18.)).py(px(8.)).text_size(px(13.5)).child(div().text_color(if disk_free < 20 * (1 << 30) { pp.amber } else { pp.text }).child("Free")).child(mb(disk_free)))
        .when(disk_total > 0, |d| d.child(div().flex().flex_row().justify_between().px(px(18.)).py(px(8.)).text_size(px(13.5)).child(div().text_color(pp.text2).child("Size")).child(mb(disk_total))))
        .when(cache > 0, |d| d.child(div().flex().flex_row().items_center().pl(px(18.)).pr(px(8.)).py(px(4.)).text_size(px(13.5))
            .child(div().flex_1().text_color(pp.text2).child(format!("Download cache {}", mb(cache))))
            .child(text_btn("dk-clear", &pp, "Clear").on_click(|_, _, cx| send(cx, "/api/cache/clear".into(), json!({}), "Cleared the download cache")))));
    out = out.child(grid_of(vec![consumers.into_any_element(), disk_card.into_any_element()], 2));

    out = out.child(site_cards(p, cx, true));
    // Services as cards.
    // Every installed service gets a card, running or not, so turning one off never moves the
    // cards around; a stopped one shows Stopped and its switch.
    let svc_meta = arr(&t.services["services"]);
    let smx = t.svc_metrics.clone();
    let busy = t.busy.clone();
    let mut names: Vec<String> = svc_meta.iter().filter(|s| s["installed"] == true).map(|s| txt(&s["name"])).collect();
    for x in procs.iter().filter(|x| x["group"] == "service") { let n = txt(&x["name"]); if !names.contains(&n) { names.push(n); } }
    let mut sgrid: Vec<AnyElement> = Vec::new();
    for name in names {
        let x = procs.iter().find(|x| txt(&x["name"]) == name).cloned().unwrap_or(Value::Null);
        let meta = svc_meta.iter().find(|m| txt(&m["name"]) == name).cloned().unwrap_or(Value::Null);
        let title = { let t = txt(&meta["title"]); if t.is_empty() { pool_name(&name) } else { t } };
        let kind = { let k = txt(&meta["kind"]); if k.is_empty() { name.split('@').next().unwrap_or_default().to_string() } else { k } };
        let st = if x.is_null() { "stopped".to_string() } else { txt(&x["state"]) };
        let change = busy.get(&format!("svc:{name}"));
        let on = change.map(|b| b.on).unwrap_or_else(|| !x.is_null() && x["wanted"].as_bool().unwrap_or(true));
        let live = st == "running";
        let stat = smx[&name].clone();
        let key_stat: (String, &'static str) = match kind.as_str() {
            "postgres" => (arr(&stat["databases"]).len().to_string(), "databases"),
            "redis" => (stat["ops_per_sec"].as_u64().unwrap_or(0).to_string(), "ops / s"),
            "mailpit" => (stat["unread"].as_u64().unwrap_or(0).to_string(), "unread"),
            "meilisearch" => (stat["indexes"].as_u64().unwrap_or(0).to_string(), "indexes"),
            _ => (x["restarts"].as_u64().unwrap_or(0).to_string(), "restarts"),
        };
        let port = x["port"].as_u64().or(meta["port"].as_u64()).filter(|p| *p > 0).map(|p| format!(" · :{p}")).unwrap_or_default();
        let (status, tone) = match change {
            Some(b) => (if b.on { "Starting…".to_string() } else { "Stopping…".to_string() }, Tone::Warn),
            None => (format!("{}{port}", state_label(&st)), state_tone(&st)),
        };
        let (n2, sel, t2) = (name.clone(), name.clone(), title.clone());
        let dash = || "—".to_string();
        sgrid.push(cardbox(p)
            .child(card_header(p, sid(format!("svh-{name}")), pages::service_icon(&kind), &title, &status, tone,
                Some(switch_busy(sid(format!("svs-{name}")), p, on, change.is_some()).on_click(move |_, _, cx| flip_service(cx, &n2, &t2, !on)).into_any_element()), None))
            .child(stat_strip(p, vec![
                (if live { mb(x["memory"].as_u64().unwrap_or(0)) } else { dash() }, "memory", None),
                (if live { format!("{:.1}%", x["cpu"].as_f64().unwrap_or(0.)) } else { dash() }, "CPU", None),
                (if live { key_stat.0 } else { dash() }, key_stat.1, None)]))
            .child(div().mt_auto().flex().flex_row().px(px(8.)).py(px(6.)).child(text_btn(sid(format!("svd-{name}")), p, "Details").on_click(move |_, _, cx| { cx.global_mut::<Tez>().svc_sel = sel.clone(); go(cx, Page::Services); }))).into_any_element());
    }
    out = out.child(div().flex().flex_col().gap(px(14.)).child(h2(p, "Services")).child(grid_of(sgrid, 3)));
    out.into_any_element()
}

// ------------------------------------------------------------------ a site

fn site_view(p: &Pal, name: &str, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = tez(cx);
    let Some(s) = arr(&t.status["sites"]).into_iter().find(|s| txt(&s["name"]) == name) else {
        return pages::sites_page(p, cx);
    };
    let tab = if t.site_tab.is_empty() || t.site_tab == "processes" { "overview".to_string() } else { t.site_tab.clone() };
    let d = t.sites.get(name).cloned().unwrap_or(Value::Null);
    let procs = arr(&t.status["procs"]);
    let live: Vec<Value> = procs.iter().filter(|x| txt(&x["group"]) == format!("site:{name}")).cloned().collect();
    let laravel = s["laravel"].as_bool().unwrap_or(false);
    let head = div().flex().flex_col().gap(px(8.)).child(h1(p, name.to_string()))
        .child(div().flex().flex_row().flex_wrap().items_center().gap(px(6.)).text_size(px(14.)).text_color(p.text2)
            .child(div().font_family(MONO).text_size(px(14.)).child(txt(&s["url"]).trim_start_matches("https://").to_string()))
            .child(format!("· PHP {} · {} ·", txt(&s["php"]), if laravel { "Laravel" } else { "PHP" }))
            .child(div().font_family(MONO).text_size(px(13.5)).text_color(p.text3).child(txt(&s["path"]))));
    let body: AnyElement = match tab.as_str() {
        "environment" => pages::site_env(p, name, &d, window, cx),
        "domains" => pages::site_domains(p, name, &d, window, cx),
        "config" => pages::site_config(p, &txt(&s["path"]), &d),
        "logs" => {
            let sel = crate::site_log_choice(cx, name);
            let key = if sel == "laravel.log" { format!("site:{name}") } else { format!("proc:{name}/{sel}") };
            let text = tez(cx).logs.get(&key).cloned().unwrap_or_default();
            let mut opts: Vec<(String, String)> = vec![("laravel.log".into(), "laravel.log".into())];
            for pr in arr(&d["processes"]) { let n = txt(&pr["name"]); opts.push((n.clone(), cap(&n))); }
            let site = name.to_string();
            div().flex().flex_col().gap(px(14.))
                .child(div().flex().flex_row().items_center().gap(px(12.)).child(seg("sitelog", p, &opts, &sel, Rc::new(move |v, cx| { set_local(cx, &format!("log:{site}"), v); refresh_now(cx, false); }))).child(div().flex_1()).child(muted(p, "newest at the bottom")))
                .child(code_block(p, &text, 200)).into_any_element()
        }
        _ => site_overview(p, name, &s, &d, &live, window, cx),
    };
    div().flex().flex_col().gap(px(22.)).child(head).child(body).into_any_element()
}

fn site_overview(p: &Pal, name: &str, s: &Value, d: &Value, live: &[Value], window: &mut Window, cx: &mut App) -> AnyElement {
    let pp = *p;
    // Run a command here.
    let key = format!("cmd:{name}");
    let k2 = key.clone();
    let input = text_input(window, cx, &key, &local(cx, &key), "migrate, queue:retry all, test --parallel, or composer / npm …", false, Some(Rc::new(move |v, cx| set_local(cx, &k2, v))));
    let (n1, k3) = (name.to_string(), key.clone());
    let runner = div().flex().flex_row().items_center().gap(px(10.)).h(px(40.)).pl(px(12.)).pr(px(4.)).rounded(px(8.)).border_1().border_color(p.line2).bg(p.surface)
        .child(div().font_family(MONO).text_size(px(13.5)).text_color(p.text3).child("php artisan"))
        .child(div().flex_1().child(input_el(&input, true).appearance(false)))
        .child(div().id("run-cmd").size(px(32.)).rounded(px(6.)).bg(p.accent).flex().items_center().justify_center().hover(|s| s.opacity(0.85))
            .child(icon("corner-down-left", 15., p.bg))
            .on_click(move |_, _, cx| {
                let line = local(cx, &k3).trim().to_string();
                if line.is_empty() { notify(cx, "Type a command first"); return; }
                let mut parts: Vec<String> = line.split_whitespace().map(|x| x.to_string()).collect();
                if !matches!(parts[0].as_str(), "composer" | "npm" | "artisan") { parts.insert(0, "artisan".into()); }
                let args: Vec<Value> = parts.iter().map(|x| Value::String(x.clone())).collect();
                set_local(cx, &k3, String::new());
                send(cx, "/api/jobs".into(), json!({ "args": args, "site": n1.clone(), "title": format!("{line} in {n1}") }), "Running");
            }));
    let t = tez(cx);
    let job_log = if t.job_open.is_some() { t.job_log.clone() } else { String::new() };
    let mm = t.site_metrics["sites"][name].clone();
    let mut out = div().flex().flex_col().gap(px(22.)).child(runner)
        .when(!job_log.is_empty(), |d| d.child(code_block(&pp, &job_log, 14)));

    // Performance
    let series = arr(&mm["series"]);
    let has_req = series.iter().any(|x| x["req"].as_f64().unwrap_or(0.) > 0.);
    let mut perf = div().flex().flex_col().child(group_title(p, "Performance", Some("from the web server's log, last hour".into())));
    if mm.is_null() {
        perf = perf.child(div().px(px(18.)).py(px(16.)).child(muted(p, "Response times appear here once the tez daemon is updated.")));
    } else if !has_req {
        perf = perf.child(div().px(px(18.)).py(px(16.)).child(muted(p, "No requests in the last hour. Open the site and its response times show up here.")));
    } else {
        let p50: Vec<f64> = series.iter().map(|x| x["p50_ms"].as_f64().unwrap_or(0.)).collect();
        let p95: Vec<f64> = series.iter().map(|x| x["p95_ms"].as_f64().unwrap_or(0.)).collect();
        let e5: f64 = series.iter().filter_map(|x| x["e5xx"].as_f64()).sum();
        let kpi = |v: String, l: &str| div().flex_1().flex().flex_col().gap(px(2.)).child(div().text_size(px(24.)).child(v)).child(muted(&pp, l.to_string()));
        perf = perf.child(div().flex().flex_row().gap(px(16.)).px(px(18.)).pt(px(18.))
                .child(kpi(mm["p95_ms"].as_f64().map(ms).unwrap_or("—".into()), "p95 response, 5 min"))
                .child(kpi(p50.last().copied().map(ms).unwrap_or("—".into()), "p50 response"))
                .child(kpi(format!("{:.0} / min", mm["rpm"].as_f64().unwrap_or(0.)), "requests"))
                .child(kpi(format!("{e5:.0}"), "server errors, 1 h")))
            .child(div().flex().flex_row().gap(px(18.)).px(px(18.)).pt(px(14.)).text_size(px(13.5)).text_color(pp.text2)
                .child(div().flex().flex_row().items_center().gap(px(8.)).child(div().w(px(14.)).h(px(2.)).bg(pp.series[0])).child("p95"))
                .child(div().flex().flex_row().items_center().gap(px(8.)).child(div().w(px(14.)).h(px(2.)).bg(pp.series[1])).child("p50")))
            .child(div().h(px(130.)).mx(px(18.)).mt(px(8.)).child(line_chart(p, vec![(p95.clone(), pp.series[0]), (p50.clone(), pp.series[1])], p95.iter().chain(p50.iter()).copied().fold(0., f64::max) * 1.2)));
        let routes = arr(&mm["routes"]);
        if !routes.is_empty() {
            perf = perf.child(div().flex().flex_row().gap(px(12.)).px(px(18.)).pt(px(22.)).pb(px(10.)).border_b_1().border_color(pp.line).text_size(px(13.5)).text_color(pp.text3)
                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).text_color(pp.text).text_size(px(14.)).child("Slowest routes")).child(div().w(px(70.)).child("Hits")).child(div().w(px(90.)).child("p95")).child(div().w(px(90.)).child("Worst")));
            for r in routes.iter().take(8) {
                let p95v = r["p95_ms"].as_f64().unwrap_or(0.);
                perf = perf.child(div().flex().flex_row().items_center().gap(px(12.)).px(px(18.)).py(px(12.)).border_b_1().border_color(pp.line).text_size(px(14.))
                    .child(div().flex_1().min_w(px(0.)).truncate().font_family(MONO).text_size(px(13.)).child(format!("{} {}", txt(&r["method"]), txt(&r["path"]))))
                    .child(div().w(px(70.)).child(r["count"].as_u64().unwrap_or(0).to_string()))
                    .child(div().w(px(90.)).text_color(if p95v > 300. { pp.amber } else { pp.text }).child(ms(p95v)))
                    .child(div().w(px(90.)).child(ms(r["max_ms"].as_f64().unwrap_or(0.)))));
            }
        }
        let errors = arr(&mm["errors"]);
        if !errors.is_empty() {
            perf = perf.child(div().px(px(18.)).pt(px(22.)).pb(px(10.)).border_b_1().border_color(pp.line).child(strong(&pp, "Latest server errors")));
            for e in errors.iter().rev().take(5) {
                perf = perf.child(div().flex().flex_row().items_center().gap(px(12.)).px(px(18.)).py(px(11.)).border_b_1().border_color(pp.line).text_size(px(14.))
                    .child(div().text_color(pp.red).child(txt(&e["status"])))
                    .child(div().flex_1().truncate().font_family(MONO).text_size(px(13.)).child(format!("{} {}", txt(&e["method"]), txt(&e["path"]))))
                    .child(muted(&pp, ms(e["ms"].as_f64().unwrap_or(0.)))));
            }
        }
    }
    out = out.child(perf);

    // Queue and app log
    let q = mm["queue"].clone();
    let log = mm["log"].clone();
    if !q.is_null() || !log.is_null() {
        let horizon_url = live.iter().find(|x| txt(&x["name"]).ends_with("/horizon")).map(|x| txt(&x["url"])).filter(|u| !u.is_empty());
        let mut g = div().flex().flex_col().child(group_title(p, "Queue", if q.is_null() { None } else { Some(format!("{:.0} jobs a minute · {} failed in the last hour", q["jpm"].as_f64().unwrap_or(0.), q["failed_1h"].as_u64().unwrap_or(0))) })
            .when_some(horizon_url, |d, u| d.child(div().flex_1()).child(button("hz-dash", &pp, Kind::Secondary, true, None, Some("Horizon dashboard")).on_click(move |_, _, _| api::open_url(&u)))));
        for j in arr(&q["jobs"]).iter().take(6) {
            let failed = j["failed"].as_u64().unwrap_or(0);
            g = g.child(div().flex().flex_row().items_center().gap(px(16.)).px(px(18.)).py(px(13.)).border_b_1().border_color(pp.line)
                .child(div().flex_1().min_w(px(0.)).flex().flex_col().child(div().font_family(MONO).text_size(px(13.5)).truncate().child(txt(&j["name"])))
                    .child(muted(&pp, format!("{} runs{}", j["count"].as_u64().unwrap_or(0), if failed > 0 { format!(", {failed} failed") } else { String::new() }))))
                .child(div().text_size(px(14.)).text_color(pp.text2).child(format!("avg {} · worst {}", ms(j["avg_ms"].as_f64().unwrap_or(0.)), ms(j["max_ms"].as_f64().unwrap_or(0.))))));
        }
        if !log.is_null() {
            let n = log["errors_1h"].as_u64().unwrap_or(0);
            let last = txt(&log["last"]["message"]);
            g = g.child(div().flex().flex_row().items_center().gap(px(16.)).px(px(18.)).py(px(13.)).border_b_1().border_color(pp.line)
                .child(div().flex_1().min_w(px(0.)).flex().flex_col().child(strong(&pp, "laravel.log")).child(muted(&pp, if n == 0 || last.is_empty() { "No ERROR entries in the last hour".to_string() } else { last }).truncate()))
                .child(if n == 0 { div().text_color(pp.green).text_size(px(14.)).child("None") } else { div().text_color(pp.red).text_size(px(14.)).child(format!("{n} errors")) }));
        }
        out = out.child(g);
    }

    // Processes and site details (shared with the old page).
    let busy = tez(cx).busy.clone();
    out = out.child(process_rows(p, name, d, live, &busy));
    let installed: Vec<String> = arr(&tez(cx).status["php"]).iter().map(|v| txt(&v["minor"])).collect();
    let isolated = s["isolated"].as_bool().unwrap_or(false);
    let php_now = txt(&s["php"]);
    let mut opts = vec![(String::new(), format!("Automatic (PHP {php_now})"))];
    for m in &installed { opts.push((m.clone(), format!("PHP {m}"))); }
    let n2 = name.to_string();
    let php_sel = pages::select(&format!("php-{name}"), p, cx, if isolated { &php_now } else { "" }, opts, 220., Rc::new(move |v, cx| send(cx, format!("/api/site/{n2}"), json!({ "php": if v.is_empty() { Value::Null } else { Value::String(v) } }), "Saved; PHP switched")));
    let node_sel = pages::site_node_select(p, cx, name, d);
    let env_val = |k: &str| arr(&d["env"]).iter().flat_map(|g| arr(&g["keys"])).find(|x| txt(&x["key"]) == k).map(|x| txt(&x["value"])).unwrap_or_default();
    let db = env_val("DB_DATABASE");
    let branch = api::git_branch(&txt(&s["path"])).unwrap_or_else(|| "Not a git repository".into());
    let domains = arr(&d["domains"]).len();
    out = out.child(div().flex().flex_col().child(group_title(p, "Site", None))
        .child(field(p, "PHP version", Some("Read from composer.json unless you pick one".into()), false, php_sel))
        .when_some(node_sel, |g, sel| g.child(field(p, "Node.js version", Some("Read from .nvmrc, .node-version or package.json unless you pick one".into()), false, sel)))
        .child(field(p, "Addresses", Some(format!("{name}.test and {name}.localhost always work")), false, div().text_color(pp.text2).child(if domains == 0 { "No extra domains".to_string() } else { format!("{domains} extra") })))
        .child(field(p, "Database", Some(env_val("DB_CONNECTION")), false, div().text_color(pp.text2).child(if db.is_empty() { "None set".into() } else { db })))
        .child(field(p, "Git", Some("Current branch".into()), false, div().font_family(MONO).text_color(pp.text2).child(branch))));
    let rm = name.to_string();
    out = out.child(div().flex().flex_col().child(group_title(p, "Remove", None))
        .child(field(p, "Remove this site", Some("Stop serving it, or also move its folder to the Recycle Bin. You choose, then confirm.".into()), false,
            button("site-remove", p, Kind::Danger, true, Some("trash-2"), Some("Remove…")).on_click(move |_, _, cx| open_remove(cx, rm.clone())))));
    out.into_any_element()
}

// ------------------------------------------------------------------ the settings sheet

fn settings_sheet(p: &Pal, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = tez(cx);
    let sec = if t.settings_sec.is_empty() { "general".to_string() } else { t.settings_sec.clone() };
    let q = local(cx, "setq").to_lowercase();
    let pp = *p;
    let groups: Vec<(&str, Vec<(&str, &str, &str)>)> = vec![
        ("tez", vec![("general", "General", "settings"), ("appearance", "Appearance", "sun"), ("startup", "Startup", "power")]),
        ("This computer", vec![("https", "HTTPS and hosts", "shield-check"), ("ports", "Web server and ports", "globe"), ("performance", "Performance", "gauge")]),
        ("Accounts", vec![("github", "GitHub", "github")]),
        ("About", vec![("updates", "Updates", "download")]),
    ];
    let search = text_input(window, cx, "setq", &q, "Search settings", false, Some(Rc::new(|v, cx| set_local(cx, "setq", v))));
    let mut nav = div().w(px(320.)).flex_none().flex().flex_col().gap(px(2.)).p(px(20.)).border_r_1().border_color(p.line)
        .child(div().px(px(10.)).text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child("Settings"))
        .child(div().my(px(10.)).h(px(32.)).px(px(10.)).rounded(px(6.)).border_1().border_color(p.line2).flex().flex_row().items_center().gap(px(8.))
            .child(icon("search", 14., p.text3)).child(div().flex_1().child(input_el(&search, false).appearance(false))));
    for (g, items) in &groups {
        let shown: Vec<_> = items.iter().filter(|(_, l, _)| q.is_empty() || l.to_lowercase().contains(&q)).collect();
        if shown.is_empty() { continue; }
        nav = nav.child(div().px(px(10.)).pt(px(14.)).pb(px(4.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text3).child(g.to_string()));
        for (key, label, ic) in shown {
            let on = *key == sec;
            let k = key.to_string();
            nav = nav.child(div().id(sid(format!("set-{key}"))).flex().flex_row().items_center().gap(px(10.)).h(px(32.)).px(px(10.)).rounded(px(6.))
                .when(on, |d| d.bg(pp.surface3)).hover(move |s| s.bg(pp.surface2))
                .child(icon(ic, 16., pp.text2)).child(label.to_string())
                .on_click(move |_, _, cx| { cx.global_mut::<Tez>().settings_sec = k.clone(); cx.refresh_windows(); }));
        }
    }
    let (title, lead, body) = settings_section(p, &sec, window, cx);
    div().absolute().top_0().left_0().size_full().occlude()
        .child(div().id("sheet-dim").absolute().top_0().left_0().size_full().bg(rgba(0x0000008c))
            .on_click(|_, _, cx| { cx.global_mut::<Tez>().settings_open = false; cx.refresh_windows(); }))
        .child(div().id("sheet").occlude().absolute().top(px(56.)).bottom(px(36.)).left(px(56.)).right(px(56.)).flex().flex_row().bg(p.bg).border_1().border_color(p.line2).rounded(px(12.)).shadow_lg().overflow_hidden()
            .child(nav)
            .child(div().id("sheet-body").flex_1().min_w(px(0.)).relative().overflow_y_scroll()
                .child(div().absolute().right(px(18.)).top(px(18.)).child(icon_btn("sheet-close", p, "x", false, false).on_click(|_, _, cx| { cx.global_mut::<Tez>().settings_open = false; cx.refresh_windows(); })))
                .child(div().px(px(56.)).pt(px(48.)).pb(px(48.)).flex().flex_col().gap(px(8.))
                    .child(div().text_size(px(24.)).child(title)).child(sub(p, lead)).child(div().mt(px(26.)).flex().flex_col().gap(px(22.)).child(body)))))
        .into_any_element()
}

fn settings_section(p: &Pal, sec: &str, window: &mut Window, cx: &mut App) -> (String, String, AnyElement) {
    let t = tez(cx);
    let s = t.settings.clone();
    let pp = *p;
    let trusted = s["ca_trusted"].as_bool().unwrap_or(false);
    let flag = |k: &str| s[k].as_bool().unwrap_or(false);
    let toggle = |id: &'static str, key: &'static str, on: bool, ok: &'static str| switch(id, &pp, on).on_click(move |_, _, cx| set_setting(cx, key, json!(!on), ok));
    match sec {
        "appearance" => {
            let theme = if t.theme.is_empty() { "system".to_string() } else { t.theme.clone() };
            let opts = vec![("system".to_string(), "Match the system".to_string()), ("dark".to_string(), "Dark".to_string()), ("light".to_string(), "Light".to_string())];
            ("Appearance".into(), "How tez looks".into(), div().flex().flex_col().child(group_title(p, "Theme", None))
                .child(field(p, "Theme", Some("Dark and light are tuned separately".into()), false, seg("theme", p, &opts, &theme, Rc::new(|v, cx| set_theme(cx, v))))).into_any_element())
        }
        "startup" => ("Startup".into(), "What happens when you sign in".into(), div().flex().flex_col().child(group_title(p, "Startup", None))
            .child(field(p, "Start tez when I sign in", Some("Runs in the tray with the window hidden".into()), false, toggle("set-auto", "autostart", flag("autostart"), "Saved"))).into_any_element()),
        "https" => ("HTTPS and hosts".into(), "One-time steps that need admin rights".into(), div().flex().flex_col().child(group_title(p, "Local HTTPS", None))
            .child(field(p, "Certificate authority", Some("tez's own authority. It never leaves this computer.".into()), false,
                if trusted { div().text_color(pp.green).child("Trusted").into_any_element() } else { button("set-trust", p, Kind::Primary, true, None, Some("Trust")).on_click(|_, _, cx| job(cx, &["trust"], "Trust local HTTPS", "Asking for permission")).into_any_element() }))
            .child(field(p, "Hosts file", Some("Needed for .test names. .localhost names work without it.".into()), false,
                button("set-hosts", p, Kind::Secondary, true, None, Some("Add missing names")).on_click(|_, _, cx| job(cx, &["hosts"], "Update hosts", "Asking for permission")))).into_any_element()),
        "ports" => {
            let note = arr(&t.status["notes"]).iter().map(txt).find(|n| n.contains("port")).unwrap_or_else(|| "If a port is taken, tez falls back to 8088 or 8443 and says so".into());
            let http = crate::draft(cx, "set:http", s["http_port"].as_u64().map(|x| x.to_string()).unwrap_or_default());
            let https = crate::draft(cx, "set:https", s["https_port"].as_u64().map(|x| x.to_string()).unwrap_or_default());
            let hi = text_input(window, cx, "set:http", &http, "80", false, Some(Rc::new(|v, cx| if let Ok(n) = v.trim().parse::<u16>() { crate::queue(cx, "set:http".into(), v.clone(), "/api/settings".into(), json!({ "http_port": n })) })));
            let si = text_input(window, cx, "set:https", &https, "443", false, Some(Rc::new(|v, cx| if let Ok(n) = v.trim().parse::<u16>() { crate::queue(cx, "set:https".into(), v.clone(), "/api/settings".into(), json!({ "https_port": n })) })));
            // Caddy or Nginx: Nginx shows greyed out, with the reason, where it cannot run.
            let now = { let v = txt(&s["web_server"]); if v.is_empty() { "caddy".to_string() } else { v } };
            let nginx_ok = s["web_servers"]["nginx"]["available"].as_bool().unwrap_or(true);
            let why = txt(&s["web_servers"]["nginx"]["reason"]);
            let servers = vec![("caddy".to_string(), "Caddy".to_string()), ("nginx".to_string(), "Nginx".to_string())];
            let off: Vec<String> = if nginx_ok { Vec::new() } else { vec!["nginx".into()] };
            let cur = now.clone();
            let server_seg = seg_with("set-web", p, &servers, &now, &off, Rc::new(move |v, cx| {
                if v == cur { return; }
                set_setting(cx, "web_server", json!(v), if v == "nginx" { "Nginx serves the sites now" } else { "Caddy serves the sites now" });
            }));
            let server_help = if nginx_ok { "Same sites, names and HTTPS with either. PHP is not touched.".to_string() } else { format!("Nginx is not available here. {why}") };
            ("Web server and ports".into(), "What serves your sites, and where it listens".into(), div().flex().flex_col().child(group_title(p, "Web server", None))
                .child(field(p, "Server", Some(server_help), false, server_seg))
                .child(field(p, "HTTPS port", Some("The port for https:// addresses".into()), false, div().w(px(120.)).child(input_el(&si, true))))
                .child(field(p, "HTTP port", Some({ let mut c = note.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }), false, div().w(px(120.)).child(input_el(&hi, true)))).into_any_element())
        }
        "performance" => {
            let workers = crate::draft(cx, "set:workers", s["php_workers"].as_u64().map(|x| x.to_string()).unwrap_or_default());
            let wi = text_input(window, cx, "set:workers", &workers, "6", false, Some(Rc::new(|v, cx| if let Ok(n) = v.trim().parse::<u64>() { crate::queue(cx, "set:workers".into(), v.clone(), "/api/settings".into(), json!({ "php_workers": n })) })));
            let mut g = div().flex().flex_col().child(group_title(p, "Performance", None))
                .child(field(p, "PHP workers per version", Some("Each serves one request at a time".into()), false, div().w(px(90.)).child(input_el(&wi, true))));
            if cfg!(windows) {
                g = g.child(field(p, "Skip Defender scans of tez and Sites", Some("The biggest speed-up left on Windows. Asks for admin once.".into()), false,
                    button("set-defender", p, Kind::Secondary, true, None, Some("Set up")).on_click(|_, _, cx| job(cx, &["doctor", "--fix"], "Defender exclusions", "Asking for permission"))));
            }
            ("Performance".into(), "Speed and memory".into(), g.into_any_element())
        }
        "github" => {
            let accounts = arr(&t.identities["accounts"]);
            let routes = arr(&t.identities["routes"]);
            let mut g = div().flex().flex_col().child(group_title(p, "Accounts", Some("from the GitHub CLI and ~/.ssh/config".into())));
            for a in &accounts {
                let login = txt(&a["login"]);
                let route = routes.iter().find(|r| txt(&r["login"]) == login).map(|r| txt(&r["route"])).unwrap_or_default();
                let how = if let Some(alias) = route.strip_prefix("ssh:") { format!("SSH through {alias}") } else { "HTTPS through gh".into() };
                g = g.child(field(p, &login, Some(how), false, div().text_color(pp.green).child("Signed in")));
            }
            let token = text_input(window, cx, "ghtoken", "", "ghp_… or github_pat_…", true, None);
            let tk = token.clone();
            g = g.child(field(p, "Sign in with a token", Some("Handed to gh auth login. tez never stores it.".into()), false, div().w_full().flex().flex_row().gap(px(8.))
                .child(div().flex_1().child(input_el(&token, true).mask_toggle()))
                .child(button("gh-login", p, Kind::Secondary, false, None, Some("Sign in")).on_click(move |_, window, cx| {
                    let v = tk.read(cx).value().to_string();
                    if v.trim().is_empty() { notify(cx, "Paste a token first"); return; }
                    tk.update(cx, |i, cx| i.set_value("", window, cx));
                    send(cx, "/api/github/login".into(), json!({ "token": v }), "Signed in to GitHub");
                }))));
            ("GitHub".into(), "Accounts used to clone and create sites".into(), g.into_any_element())
        }
        "updates" => {
            let version = txt(&t.status["version"]);
            ("Updates".into(), if version.is_empty() { "tez".into() } else { format!("tez {version}") }, div().flex().flex_col().child(group_title(p, "Releases", None))
                .child(field(p, "Releases", Some("New versions are published on GitHub for Windows, macOS and Linux".into()), false,
                    button("set-rel", p, Kind::Secondary, true, Some("arrow-up-right"), Some("Open")).on_click(|_, _, _| api::open_url("https://github.com/boparaiamrit/tez/releases"))))
                .child(field(p, "Web dashboard", Some("The same settings in a browser, with a config file editor".into()), false,
                    button("set-web", p, Kind::Secondary, true, Some("arrow-up-right"), Some("Open")).on_click(|_, _, _| api::open_url(&format!("http://127.0.0.1:{}", api::port()))))).into_any_element())
        }
        _ => {
            let installed: Vec<String> = arr(&s["php_installed"]).iter().map(txt).collect();
            let php_sel = pages::select("defphp", p, cx, &txt(&s["default_php"]), installed.iter().map(|m| (m.clone(), format!("PHP {m}"))).collect(), 220., Rc::new(|v, cx| set_setting(cx, "default_php", json!(v), "Default PHP changed")));
            let tld = crate::draft(cx, "set:tld", txt(&s["tld"]));
            let tld_in = text_input(window, cx, "set:tld", &tld, "test", false, Some(Rc::new(|v, cx| crate::queue(cx, "set:tld".into(), v.clone(), "/api/settings".into(), json!({ "tld": v })))));
            // Folders removed from tez but kept on disk: each can be served again.
            let hidden = tez(cx).hidden_sites.clone();
            let hidden_rows: Vec<AnyElement> = arr(&hidden["hidden"]).iter().map(|h| {
                let (hn, hp) = (txt(&h["name"]), txt(&h["path"]));
                let hp2 = hp.clone();
                field(p, &format!("{hn} (hidden)"), Some(hp), false,
                    button(sid(format!("unhide-{hn}")), p, Kind::Secondary, true, None, Some("Serve again")).on_click(move |_, _, cx| send(cx, "/api/sites/unhide".into(), json!({ "path": hp2 }), "Serving it again")))
                    .into_any_element()
            }).collect();
            let parked: Vec<String> = arr(&s["parked"]).iter().map(txt).collect();
            let folders = text_input(window, cx, "set:parked", &crate::draft(cx, "set:parked", parked.join("; ")), "", false, Some(Rc::new(|v, cx| {
                let list: Vec<String> = v.split(';').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
                crate::queue(cx, "set:parked".into(), v.clone(), "/api/settings".into(), json!({ "parked": list }));
            })));
            ("General".into(), "Defaults for every site on this computer".into(), div().flex().flex_col().gap(px(22.))
                .child(div().flex().flex_col().child(group_title(p, "Defaults", None))
                    .child(field(p, "Default PHP", Some("For sites that do not ask for a version, and php in a terminal".into()), false, php_sel))
                    .child(field(p, "Domain ending", Some("Sites answer at name.<ending>, and always at name.localhost".into()), false, div().w(px(160.)).child(input_el(&tld_in, true))))
                    .child(field(p, "Sites folders", Some("Every folder inside becomes a site. Separate several with ;".into()), false, div().w_full().child(input_el(&folders, true))))
                    .children(hidden_rows))
                .child(div().flex().flex_col().child(group_title(p, "Debugging", None))
                    .child(field(p, "Capture dump() and dd()", Some("Shows them under Dumps; the page still renders".into()), false, toggle("set-dumps", "dumps", flag("dumps"), "Saved")))
                    .child(field(p, "Xdebug", Some("Step debugging on port 9003. Slows PHP while on.".into()), false, toggle("set-xdebug", "xdebug", flag("xdebug"), "Saved; PHP restarted"))))
                .into_any_element())
        }
    }
}

/// A card's header: icon, name and status on the left, its switch top-right. Clicking the name opens it.
fn card_header(p: &Pal, id: ElementId, ic: &str, title: &str, status: &str, tone: Tone, control: Option<AnyElement>, target: Option<Page>) -> Div {
    let fg = if tone == Tone::Neutral { p.text3 } else { p.tone(tone).0 };
    let clickable = target.is_some();
    let name_el = div().id(id).flex_1().min_w(px(0.)).flex().flex_col().gap(px(3.)).when(clickable, |d| d)
        .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).truncate().child(title.to_string()))
        .child(div().flex().flex_row().items_center().gap(px(6.)).text_size(px(13.5)).text_color(fg).child(dot(fg)).child(div().truncate().child(status.to_string())))
        .when_some(target, |d, t| d.on_click(move |_, _, cx| go(cx, t.clone())));
    div().flex().flex_row().items_start().gap(px(12.)).px(px(16.)).pt(px(16.)).pb(px(12.))
        .child(div().size(px(36.)).flex_none().rounded(px(10.)).bg(p.surface3).flex().items_center().justify_center().child(icon(ic, 18., p.text2)))
        .child(name_el)
        .when_some(control, |d, c| d.child(c))
}

/// The processes this project uses as cards (running or not, so a card never moves when it is
/// switched off); the rest in one "Can also run" list, each with its switch.
fn process_rows(p: &Pal, name: &str, d: &Value, live: &[Value], busy: &BTreeMap<String, Busy>) -> AnyElement {
    let pp = *p;
    let mut cards: Vec<AnyElement> = Vec::new();
    let mut idle: Vec<AnyElement> = Vec::new();
    let mut running = 0;
    let site_up = d["up"].as_bool().unwrap_or(false);
    for pr in arr(&d["processes"]) {
        let pn = txt(&pr["name"]);
        let on_cfg = pr["on"].as_bool().unwrap_or(false);
        let change = busy.get(&format!("proc:{name}/{pn}"));
        let on = change.map(|b| b.on).unwrap_or(on_cfg);
        let lv = live.iter().find(|x| txt(&x["name"]) == format!("{name}/{pn}")).cloned();
        let about = { let a = txt(&pr["about"]); if a.is_empty() { txt(&pr["command"]) } else { a } };
        let (site, p2) = (name.to_string(), pn.clone());
        let sw = switch_busy(sid(format!("pon-{pn}")), p, on, change.is_some()).on_click(move |_, _, cx| flip_proc(cx, &site, &p2, !on, site_up));
        let as_card = lv.is_some() || on_cfg || pr["suggested"].as_bool().unwrap_or(false) || change.is_some();
        if as_card {
            let x = lv.clone().unwrap_or(Value::Null);
            let st = if x.is_null() { "stopped".to_string() } else { txt(&x["state"]) };
            if st == "running" { running += 1; }
            let up = st == "running";
            let mem = x["memory"].as_u64().unwrap_or(0);
            let (status, tone) = match change {
                Some(b) if site_up => (if b.on { "Starting…".to_string() } else { "Stopping…".to_string() }, Tone::Warn),
                Some(b) => (if b.on { "Turning on…".to_string() } else { "Turning off…".to_string() }, Tone::Warn),
                None if !x.is_null() => (format!("{}{}", state_label(&st), x["uptime"].as_u64().filter(|u| *u > 0).map(|u| format!(" · up {}", ago(u))).unwrap_or_default()), state_tone(&st)),
                None if on_cfg => ("On · starts with the site".to_string(), Tone::Neutral),
                None => ("Off".to_string(), Tone::Neutral),
            };
            let dash = Some(txt(&x["url"])).filter(|u| !u.is_empty() && up);
            let full = format!("{name}/{pn}");
            let (site2, p3) = (name.to_string(), pn.clone());
            let none = || "—".to_string();
            cards.push(cardbox(p)
                .child(card_header(p, sid(format!("ph-{pn}")), pages::proc_icon(&pn), &cap(&pn), &status, tone, Some(sw.into_any_element()), None))
                .child(div().px(px(16.)).pb(px(12.)).text_size(px(14.)).text_color(pp.text3).truncate().child(about))
                .child(stat_strip(p, vec![
                    (if up { mb(mem) } else { none() }, "memory", if up && mem > 1 << 30 { Some(pp.amber) } else { None }),
                    (if up { format!("{:.1}%", x["cpu"].as_f64().unwrap_or(0.)) } else { none() }, "CPU", None),
                    (if x.is_null() { none() } else { x["restarts"].as_u64().unwrap_or(0).to_string() }, "restarts", if x["restarts"].as_u64().unwrap_or(0) > 0 { Some(pp.amber) } else { None }),
                ]))
                .child(div().mt_auto().flex().flex_row().gap(px(2.)).px(px(8.)).py(px(6.))
                    .when_some(dash, |d, u| d.child(text_btn(sid(format!("pd-{pn}")), &pp, "Dashboard").on_click(move |_, _, _| api::open_url(&u))))
                    .when(!x.is_null(), |d| d.child(text_btn(sid(format!("pr-{pn}")), p, "Restart").on_click(move |_, _, cx| send(cx, format!("/api/proc/{full}/restart"), json!({}), "Restarting"))))
                    .child(text_btn(sid(format!("pl-{pn}")), p, "Log").on_click(move |_, _, cx| { set_local(cx, &format!("log:{site2}"), p3.clone()); cx.global_mut::<Tez>().site_tab = "logs".into(); refresh_now(cx, false); }))).into_any_element());
        } else {
            {
                idle.push(div().flex().flex_row().items_center().gap(px(14.)).px(px(16.)).py(px(11.)).border_t_1().border_color(pp.cardline)
                    .child(div().size(px(30.)).flex_none().rounded(px(9.)).bg(pp.surface3).flex().items_center().justify_center().child(icon(pages::proc_icon(&pn), 15., pp.text3)))
                    .child(div().flex_1().min_w(px(0.)).flex().flex_row().items_baseline().gap(px(8.)).child(div().font_weight(FontWeight::SEMIBOLD).child(cap(&pn))).child(div().text_size(px(14.)).text_color(pp.text3).truncate().child(about)))
                    .child(sw)
                    .into_any_element());
            }
        }
    }
    let mut g = div().flex().flex_col().gap(px(14.))
        .child(div().flex().flex_row().items_baseline().gap(px(12.)).child(h2(p, "Processes")).child(muted(p, format!("{running} running"))));
    let has_cards = !cards.is_empty();
    if has_cards { g = g.child(grid_of(cards, 3)); }
    if !idle.is_empty() {
        g = g.child(cardbox(p).child(div().px(px(16.)).pt(px(14.)).pb(px(8.)).text_size(px(14.)).text_color(pp.text3).child(if has_cards { "Can also run with this site" } else { "Runs with this site when turned on" })).children(idle));
    }
    g.into_any_element()
}
/// Cards in fixed columns, so a short last row keeps the same card width.
/// Cards in equal columns. A CSS grid stretches every card in a row to the tallest one, so cards
/// side by side line up top and bottom; a short last row keeps its empty cells.
fn grid_of(items: Vec<AnyElement>, cols: usize) -> Div {
    div().grid().grid_cols(cols.max(1) as u16).gap(px(14.)).children(items)
}
/// A card per site (Home and the Sites page).
/// `titled`: Home shows a "Sites" heading above the cards; the Sites page has its own title.
fn site_cards(p: &Pal, cx: &App, titled: bool) -> AnyElement {
    let t = tez(cx);
    let procs = arr(&t.status["procs"]);
    let pp = *p;
    // Sites as cards.
    let sm = t.site_metrics["sites"].clone();
    let sites = arr(&t.status["sites"]);
    let mut grid: Vec<AnyElement> = Vec::new();
    for s in &sites {
        let name = txt(&s["name"]);
        let mm = sm[&name].clone();
        let live: Vec<&Value> = procs.iter().filter(|x| txt(&x["group"]) == format!("site:{name}")).collect();
        let up_cfg = s["up"].as_bool().unwrap_or(false);
        let running = live.iter().any(|x| x["state"] == "running");
        let declared = s["processes"].as_object().map(|o| !o.is_empty()).unwrap_or(false);
        let mem: u64 = live.iter().filter_map(|x| x["memory"].as_u64()).sum();
        let rpm = mm["rpm"].as_f64().unwrap_or(0.);
        let has_req = arr(&mm["series"]).iter().any(|x| x["req"].as_f64().unwrap_or(0.) > 0.);
        let errs: f64 = arr(&mm["series"]).iter().filter_map(|x| x["e5xx"].as_f64()).sum();
        let status = if running { format!("Running · PHP {}", txt(&s["php"])) } else if declared { format!("Served · processes off · PHP {}", txt(&s["php"])) } else { format!("Served · PHP {}", txt(&s["php"])) };
        let (url, path) = (txt(&s["url"]), txt(&s["path"]));
        let horizon = live.iter().find(|x| txt(&x["name"]).ends_with("/horizon")).map(|x| txt(&x["url"])).filter(|u| !u.is_empty());
        let n2 = name.clone();
        let change = busy_on(cx, &format!("site:{name}"));
        let up_now = change.unwrap_or(up_cfg);
        let status = match change { Some(true) => "Starting processes…".to_string(), Some(false) => "Stopping processes…".to_string(), None => status };
        let ctl = if declared { Some(switch_busy(sid(format!("hcs-{name}")), p, up_now, change.is_some()).on_click(move |_, _, cx| flip_site(cx, &n2, !up_now)).into_any_element()) } else { None };
        grid.push(cardbox(p)
            .child(card_header(p, sid(format!("hc-{name}")), "folder", &name, &status, if change.is_some() { Tone::Warn } else if running { Tone::Ok } else { Tone::Neutral }, ctl, Some(Page::Site(name.clone()))))
            .child(stat_strip(p, vec![
                (if has_req { format!("{rpm:.0} / min") } else { "—".into() }, "requests", None),
                (if has_req { mm["p95_ms"].as_f64().filter(|v| *v > 0.).map(ms).unwrap_or("—".into()) } else { "—".into() }, "p95 response", None),
                (if has_req { format!("{errs:.0}") } else { "—".into() }, "errors, 1 h", if errs > 0. { Some(pp.red) } else { None }),
            ]))
            .child(div().mt_auto().flex().flex_row().items_center().gap(px(2.)).px(px(8.)).py(px(6.))
                .child(text_btn(sid(format!("hco-{name}")), p, "Open site").on_click(move |_, _, _| api::open_url(&url)))
                .when_some(horizon, |d, u| d.child(text_btn(sid(format!("hch-{name}")), &pp, "Horizon").on_click(move |_, _, _| api::open_url(&u))))
                .child(text_btn(sid(format!("hct-{name}")), p, "Terminal").on_click(move |_, _, _| api::open_terminal(&path)))
                .child(div().flex_1())
                .child(div().pr(px(10.)).text_size(px(13.5)).text_color(pp.text3).child(if mem > 0 { mb(mem) } else { "PHP pool only".into() }))).into_any_element());
    }
    if sites.is_empty() { grid.push(muted(p, "No sites yet. Add one from the list on the left.").into_any_element()); }
    div().flex().flex_col().gap(px(14.)).when(titled, |d| d.child(div().flex().flex_row().items_baseline().gap(px(12.)).child(h2(p, "Sites")).child(muted(p, format!("{} in your Sites folders", sites.len()))))).child(grid_of(grid, 2)).into_any_element()
}

/// The sidebar's Node.js group (under PHP): installed versions, then "Add a version".
fn node_side(list: &mut Stateful<Div>, p: &Pal, t: &Tez, page: &Page) {
    let sel = t.local.get("node_sel").cloned().unwrap_or_default();
    let mut l = std::mem::replace(list, div().id("side-tmp")).child(side_sec(p, "Node.js"));
    for v in arr(&t.node["installed"]) {
        let ver = txt(&v["version"]);
        let users = arr(&v["sites"]).len();
        let end = if v["default"] == true { "default".to_string() } else if users > 0 { format!("{users} site{}", if users == 1 { "" } else { "s" }) } else { String::new() };
        let v2 = ver.clone();
        l = l.child(side_item(format!("nv-{ver}"), p, Some("hexagon"), format!("Node.js {ver}"), Some(end), *page == Page::Node && sel == ver, false, Page::Node)
            .on_click(move |_, _, cx| { set_local(cx, "node_sel", v2.clone()); go(cx, Page::Node); }));
    }
    l = l.child(side_item("nv-add".into(), p, Some("plus"), "Add a version".into(), None, *page == Page::Node && sel.is_empty(), false, Page::Node)
        .on_click(|_, _, cx| { set_local(cx, "node_sel", String::new()); go(cx, Page::Node); }));
    *list = l;
}
