//! The window: title bar (menus, back and forward, window buttons), the icon rail, the list
//! column and the inset content panel. Also the Home dashboard, a site's Overview and the
//! Settings sheet, as specified on the design canvas ("What each page answers").

use crate::pages;
use crate::ui::*;
use crate::{api, go, job, local, notify, refresh_now, restart_all, send, set_local, set_theme, tez, txt, Page, Tez};
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
        Page::Php => Section::Php,
        Page::Dumps | Page::Mail | Page::Logs => Section::Debug,
        _ => Section::Sites,
    }
}

// ------------------------------------------------------------------ the frame

pub fn root(window: &mut Window, cx: &mut App) -> AnyElement {
    let p = pal(cx);
    let t = tez(cx);
    let side_hidden = t.side_hidden;
    let settings_open = t.settings_open;
    let jump = t.jump;
    let message = t.message.as_ref().map(|(m, _)| m.clone());
    let title = titlebar(&p, cx);
    let rail_el = rail(&p, cx);
    let side = if side_hidden { None } else { Some(side_list(&p, cx)) };
    let panel_el = panel(&p, window, cx);
    let sheet = if settings_open { Some(settings_sheet(&p, window, cx)) } else { None };
    let jump_el = if jump { Some(pages::jump_overlay(&p, window, cx)) } else { None };
    div().size_full().relative().flex().flex_col().bg(p.side).text_color(p.text).font_family(SANS).text_size(px(15.))
        .child(title)
        .child(div().flex_1().min_h(px(0.)).flex().flex_row().child(rail_el).when_some(side, |d, s| d.child(s)).child(panel_el))
        .when_some(sheet, |d, s| d.child(s))
        .when_some(jump_el, |d, j| d.child(j))
        .when_some(message, |d, m| d.child(div().absolute().bottom(px(22.)).right(px(22.)).max_w(px(460.)).px(px(16.)).py(px(11.))
            .rounded(px(12.)).bg(p.surface3).border_1().border_color(p.line2).shadow_lg().text_size(px(14.)).child(m)))
        .into_any_element()
}

fn icon_btn(id: impl Into<ElementId>, p: &Pal, name: &str, dim: bool, round: bool) -> Stateful<Div> {
    let pp = *p;
    div().id(id).size(px(34.)).flex_none().flex().items_center().justify_center().cursor_pointer()
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
        .window_control_area(WindowControlArea::Drag)
        .child(icon_btn("tb-side", p, "panel-left", false, false).on_click(|_, _, cx| { let t = cx.global_mut::<Tez>(); t.side_hidden = !t.side_hidden; cx.refresh_windows(); }))
        .child(icon_btn("tb-back", p, "chevron-left", !can_back, false).on_click(|_, _, cx| {
            let t = cx.global_mut::<Tez>();
            if let Some(prev) = t.back.pop() { let cur = t.page.clone(); t.fwd.push(cur); t.page = prev; t.menu_open = None; refresh_now(cx, false); cx.refresh_windows(); }
        }))
        .child(icon_btn("tb-fwd", p, "chevron-right", !can_fwd, false).on_click(|_, _, cx| {
            let t = cx.global_mut::<Tez>();
            if let Some(next) = t.fwd.pop() { let cur = t.page.clone(); t.back.push(cur); t.page = next; t.menu_open = None; refresh_now(cx, false); cx.refresh_windows(); }
        }));
    if !cfg!(target_os = "macos") {
        bar = bar.child(div().w(px(10.)));
        for (key, label) in [("file", "File"), ("edit", "Edit"), ("view", "View"), ("help", "Help")] {
            let is_open = open.as_deref() == Some(key);
            let k = key.to_string();
            let mut m = div().flex().flex_col()
                .child(div().id(sid(format!("menu-{key}"))).px(px(10.)).py(px(4.)).rounded(px(6.)).text_size(px(14.)).cursor_pointer()
                    .text_color(if is_open { pp.text } else { pp.text2 }).when(is_open, |d| d.bg(pp.surface2))
                    .hover(move |s| s.bg(pp.surface2).text_color(pp.text))
                    .child(label)
                    .on_click(move |_, _, cx| { let t = cx.global_mut::<Tez>(); t.menu_open = if t.menu_open.as_deref() == Some(k.as_str()) { None } else { Some(k.clone()) }; cx.refresh_windows(); }));
            if is_open { m = m.child(deferred(anchored().snap_to_window().child(menu_popover(p, key, cx))).with_priority(2)); }
            bar = bar.child(m);
        }
    }
    bar = bar.child(div().flex_1().h_full());
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
            mi("New site", |_, cx| go(cx, Page::NewSite)),
            mi("Import a project", |_, cx| { cx.global_mut::<Tez>().new_source = "import".into(); go(cx, Page::NewSite) }),
            mi("Open the Sites folder", move |_, _| api::open_path(&parked)),
            MenuItem { sep_before: true, ..mi(if online { "Restart the web server and PHP" } else { "Start tez" }, move |_, cx| if online { restart_all(cx) } else { api::start_daemon(); notify(cx, "Starting tez") }) },
            mi("Stop tez", |_, cx| send(cx, "/api/shutdown".into(), json!({}), "Stopping tez")),
            MenuItem { sep_before: true, ..mi("Close window", |window, _| window.remove_window()) },
            mi("Quit tez", |_, cx| cx.quit()),
        ],
        "edit" => vec![
            mi("Search or jump to", |_, cx| { cx.global_mut::<Tez>().jump = true; cx.refresh_windows(); }),
            mi("Settings", |_, cx| { cx.global_mut::<Tez>().settings_open = true; cx.refresh_windows(); }),
        ],
        "view" => {
            let mut v = vec![
                mi("Home", |_, cx| go(cx, Page::Overview)),
                mi("Sites", |_, cx| go(cx, Page::Sites)),
                mi("Services", |_, cx| go(cx, Page::Services)),
                mi("PHP", |_, cx| go(cx, Page::Php)),
                mi("Dumps", |_, cx| go(cx, Page::Dumps)),
                mi("Mail", |_, cx| go(cx, Page::Mail)),
                mi("Logs", |_, cx| go(cx, Page::Logs)),
                MenuItem { sep_before: true, ..mi("Show or hide the sidebar", |_, cx| { let t = cx.global_mut::<Tez>(); t.side_hidden = !t.side_hidden; cx.refresh_windows(); }) },
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
    div().occlude().mt(px(4.)).min_w(px(260.)).p(px(6.)).rounded(px(14.)).bg(p.surface2).border_1().border_color(p.line2).shadow_lg().flex().flex_col()
        .on_mouse_down_out(move |_, _, cx| { let t = cx.global_mut::<Tez>(); if t.menu_open.as_deref() == Some(k.as_str()) { t.menu_open = None; cx.refresh_windows(); } })
        .children(items.into_iter().enumerate().map(move |(i, it)| {
            let run = it.run.clone();
            div().flex().flex_col()
                .when(it.sep_before && i > 0, |d| d.child(div().h(px(1.)).mx(px(8.)).my(px(5.)).bg(pp.line2)))
                .child(div().id(sid(format!("mi-{i}"))).flex().flex_row().items_center().gap(px(12.)).h(px(36.)).px(px(12.)).rounded(px(9.)).cursor_pointer()
                    .text_size(px(14.5)).hover(move |s| s.bg(pp.surface3))
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
        div().id(sid(format!("rail-{id}"))).size(px(42.)).rounded(px(12.)).flex().items_center().justify_center().relative().cursor_pointer()
            .when(on, |d| d.bg(pp.surface3)).hover(move |s| s.bg(pp.surface2))
            .child(icon(ic, 21., if on { pp.text } else { pp.text3 }))
            .when(badge, |d| d.child(div().absolute().top(px(8.)).right(px(8.)).size(px(8.)).rounded_full().bg(pp.text)))
            .on_click(move |_, _, cx| go(cx, target.clone()))
    };
    let mut status = div().flex().flex_col()
        .child(div().id("rail-status").size(px(42.)).rounded(px(12.)).flex().items_center().justify_center().cursor_pointer()
            .hover(move |s| s.bg(pp.surface2))
            .child(div().size(px(20.)).rounded_full().border_2().border_color(if online { pp.green } else { pp.text3 }).flex().items_center().justify_center()
                .child(div().size(px(8.)).rounded_full().bg(if online { pp.green } else { pp.text3 })))
            .on_click(|_, _, cx| { let t = cx.global_mut::<Tez>(); t.menu_open = if t.menu_open.as_deref() == Some("status") { None } else { Some("status".into()) }; cx.refresh_windows(); }));
    if status_open { status = status.child(deferred(anchored().snap_to_window().child(menu_popover(p, "status", cx))).with_priority(2)); }
    div().w(px(60.)).flex_none().flex().flex_col().items_center().gap(px(6.)).pt(px(4.)).pb(px(12.))
        .child(item("home", "house", home, false, Page::Overview))
        .child(item("sites", "globe", !home && sec == Section::Sites, false, Page::Sites))
        .child(item("services", "database", sec == Section::Services, false, Page::Services))
        .child(item("php", "code-xml", sec == Section::Php, false, Page::Php))
        .child(item("debug", "bug", sec == Section::Debug, new_dumps, Page::Dumps))
        .child(div().flex_1())
        .child(div().id("rail-settings").size(px(42.)).rounded(px(12.)).flex().items_center().justify_center().cursor_pointer()
            .hover(move |s| s.bg(pp.surface2)).child(icon("settings", 21., p.text3))
            .on_click(|_, _, cx| { cx.global_mut::<Tez>().settings_open = true; cx.refresh_windows(); }))
        .child(status)
}

fn side_item(id: String, p: &Pal, ic: Option<&str>, label: String, end: Option<String>, on: bool, sub: bool, target: Page) -> Stateful<Div> {
    let pp = *p;
    div().id(sid(id)).flex().flex_row().items_center().gap(px(12.)).h(px(if sub { 34. } else { 38. })).pl(px(if sub { 46. } else { 10. })).pr(px(10.))
        .rounded(px(10.)).cursor_pointer().text_size(px(if sub { 14.5 } else { 15. })).text_color(if sub { pp.text2 } else { pp.text })
        .when(on, |d| d.bg(pp.surface3)).hover(move |s| s.bg(pp.surface2))
        .when_some(ic.map(|s| s.to_string()), |d, i| d.child(icon(&i, 18., pp.text2)))
        .child(div().flex_1().truncate().child(label))
        .when_some(end, |d, e| d.child(div().text_size(px(13.)).text_color(pp.text3).child(e)))
        .on_click(move |_, _, cx| go(cx, target.clone()))
}

fn side_sec(p: &Pal, label: &str) -> Div { div().px(px(10.)).pt(px(18.)).pb(px(6.)).text_size(px(15.)).text_color(p.text3).child(label.to_string()) }

fn side_list(p: &Pal, cx: &App) -> Div {
    let t = tez(cx);
    let sec = section(&t.page);
    let page = t.page.clone();
    let procs = arr(&t.status["procs"]);
    let sm = t.site_metrics["sites"].clone();
    let title = match sec { Section::Sites => "Sites", Section::Services => "Services", Section::Php => "PHP", Section::Debug => "Debug" };
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
                if rpm <= 0.0 { return Some("idle".into()); }
                m["p95_ms"].as_f64().map(ms)
            };
            if !running.is_empty() {
                list = list.child(side_sec(p, "Running"));
                for s in &running {
                    let name = txt(&s["name"]);
                    list = list.child(side_item(format!("si-{name}"), p, Some("folder"), name.clone(), perf(&name).or(Some(format!("PHP {}", txt(&s["php"])))), page == Page::Site(name.clone()), false, Page::Site(name.clone())));
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
                    list = list.child(side_item(format!("si-{name}"), p, Some("folder"), name.clone(), perf(&name).or(Some(format!("PHP {}", txt(&s["php"])))), page == Page::Site(name.clone()), false, Page::Site(name.clone())));
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
                let end = if installed { s["port"].as_u64().map(|x| format!(":{x}")) } else { Some("add".into()) };
                let n2 = name.clone();
                div().id(sid(format!("sv-{name}"))).flex().flex_row().items_center().gap(px(12.)).h(px(38.)).px(px(10.)).rounded(px(10.)).cursor_pointer()
                    .when(name == sel, |d| d.bg(pp.surface3)).hover(move |s| s.bg(pp.surface2))
                    .child(if installed { dot(if st == "running" { pp.green } else { pp.text3 }).into_any_element() } else { icon("plus", 15., pp.text3).into_any_element() })
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
            for v in arr(&t.php["versions"]) {
                let minor = txt(&v["minor"]);
                let users = arr(&v["sites"]).len();
                let m2 = minor.clone();
                let on = minor == sel || (sel.is_empty() && v["default"] == true);
                list = list.child(div().id(sid(format!("pv-{minor}"))).flex().flex_row().items_center().gap(px(12.)).h(px(38.)).px(px(10.)).rounded(px(10.)).cursor_pointer()
                    .when(on, |d| d.bg(pp.surface3)).hover(move |s| s.bg(pp.surface2))
                    .child(icon("code-xml", 18., pp.text2))
                    .child(div().flex_1().child(format!("PHP {}", txt(&v["version"]))))
                    .child(div().text_size(px(13.)).text_color(pp.text3).child(if v["default"] == true { "default".to_string() } else if users > 0 { format!("{users} site{}", if users == 1 { "" } else { "s" }) } else { String::new() }))
                    .on_click(move |_, _, cx| { cx.global_mut::<Tez>().php_sel = m2.clone(); go(cx, Page::Php); }));
            }
        }
        Section::Debug => {
            let nd = t.dumps.len().saturating_sub(t.dumps_seen);
            let unread = t.mail["unread"].as_u64().unwrap_or(0);
            list = list.child(side_item("dg-dumps".into(), p, Some("bug"), "Dumps".into(), if nd > 0 { Some(format!("{nd} new")) } else { None }, page == Page::Dumps, false, Page::Dumps))
                .child(side_item("dg-mail".into(), p, Some("mail"), "Mail".into(), if unread > 0 { Some(format!("{unread} unread")) } else { None }, page == Page::Mail, false, Page::Mail))
                .child(side_item("dg-logs".into(), p, Some("scroll-text"), "Logs".into(), None, page == Page::Logs, false, Page::Logs));
        }
    }
    div().w(px(296.)).flex_none().flex().flex_col().pl(px(4.)).pr(px(10.)).pb(px(12.))
        .child(div().flex().flex_row().items_center().gap(px(6.)).pl(px(10.)).pr(px(4.)).pb(px(8.))
            .child(div().flex_1().text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(icon_btn("side-search", p, "search", false, false).on_click(|_, _, cx| { cx.global_mut::<Tez>().jump = true; cx.refresh_windows(); })))
        .child(list)
}

fn panel(p: &Pal, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = tez(cx);
    let page = t.page.clone();
    let online = t.online;
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
            (Some(pill_seg("sitetab", p, &[("overview", "Overview"), ("environment", "Environment"), ("domains", "Domains"), ("config", "Config"), ("logs", "Logs")], &tab, Rc::new(|v, cx| { cx.global_mut::<Tez>().site_tab = v; refresh_now(cx, false); cx.refresh_windows(); }))),
             vec![
                icon_btn("pt-open", p, "arrow-up-right", false, true).on_click(move |_, _, _| api::open_url(&url)).into_any_element(),
                icon_btn("pt-term", p, "terminal", false, false).on_click(move |_, _, _| api::open_terminal(&path)).into_any_element(),
                icon_btn("pt-code", p, "code-xml", false, false).on_click(move |_, _, _| api::open_editor(&path2)).into_any_element(),
             ])
        }
        _ => (None, vec![]),
    };
    let body: AnyElement = if !online {
        pages::offline(p, cx)
    } else {
        match page {
            Page::Overview => home(p, cx),
            Page::Sites => pages::sites_page(p, cx),
            Page::Site(name) => site_view(p, &name, window, cx),
            Page::NewSite => pages::new_site(p, window, cx),
            Page::Services => pages::services_page(p, window, cx),
            Page::Php => pages::php_page(p, window, cx),
            Page::Dumps => pages::dumps_page(p, cx),
            Page::Mail => pages::mail_page(p, cx),
            Page::Logs => pages::logs_page(p, cx),
            Page::Settings => { cx.global_mut::<Tez>().settings_open = true; home(p, cx) }
        }
    };
    div().flex_1().min_w(px(0.)).mr(px(8.)).mb(px(8.)).rounded(px(14.)).bg(pp.bg).border_1().border_color(pp.line).flex().flex_col().overflow_hidden()
        .child(div().h(px(64.)).flex_none().relative().flex().items_center().justify_center()
            .when_some(seg_el, |d, s| d.child(s))
            .child(div().absolute().right(px(14.)).top(px(15.)).flex().flex_row().gap(px(8.)).children(right)))
        .child(div().id(sid(format!("panel-scroll-{:?}", tez(cx).page))).flex_1().min_h(px(0.)).overflow_y_scroll()
            .child(div().w_full().px(px(40.)).pt(px(6.)).pb(px(56.)).flex().flex_col().items_center()
                .child(div().w_full().max_w(px(1000.)).flex().flex_col().gap(px(30.)).child(body))))
        .into_any_element()
}

/// The rounded switch at the top of the panel.
fn pill_seg(id: &str, p: &Pal, options: &[(&str, &str)], current: &str, on_pick: Pick) -> Div {
    let pp = *p;
    div().flex().flex_row().p(px(3.)).rounded_full().bg(p.surface2)
        .children(options.iter().map(|(v, l)| {
            let on = *v == current;
            let pick = on_pick.clone();
            let vv = v.to_string();
            div().id(sid(format!("{id}-{v}"))).h(px(38.)).px(px(26.)).rounded_full().flex().items_center().cursor_pointer()
                .text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(if on { pp.text } else { pp.text2 })
                .when(on, |d| d.bg(pp.surface3).shadow_sm())
                .child(l.to_string())
                .on_click(move |_, _, cx| pick(vv.clone(), cx))
        }))
}

// ------------------------------------------------------------------ charts

/// Stacked areas (memory by group). `rows` are oldest first; values in bytes.
fn stacked_chart(p: &Pal, rows: Vec<[f64; 4]>, hover: Option<f32>, cell: Rc<Cell<Option<Bounds<Pixels>>>>) -> impl IntoElement {
    let pp = *p;
    let c2 = cell.clone();
    let n = rows.len();
    let max = rows.iter().map(|r| r.iter().sum::<f64>()).fold(0., f64::max).max(1.) * 1.15;
    let rows2 = rows.clone();
    canvas(move |b, _, _| b, move |b, _, window, _| {
        c2.set(Some(b));
        let (x0, y0, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
        for frac in [0.25f32, 0.5, 0.75] {
            window.paint_quad(fill(Bounds::new(point(px(x0), px(y0 + h * frac)), size(px(w), px(1.))), pp.line));
        }
        window.paint_quad(fill(Bounds::new(point(px(x0), px(y0 + h - 1.)), size(px(w), px(1.))), pp.line2));
        if n < 2 { return; }
        let xs: Vec<f32> = (0..n).map(|i| x0 + w * i as f32 / (n - 1) as f32).collect();
        let mut below = vec![y0 + h; n];
        for k in 0..4 {
            let mut top = below.clone();
            for i in 0..n { top[i] = below[i] - (rows2[i][k] / max) as f32 * h; }
            let mut pb = PathBuilder::fill();
            pb.move_to(point(px(xs[0]), px(top[0])));
            for i in 1..n { pb.line_to(point(px(xs[i]), px(top[i]))); }
            for i in (0..n).rev() { pb.line_to(point(px(xs[i]), px(below[i]))); }
            pb.close();
            if let Ok(path) = pb.build() { window.paint_path(path, pp.series[k]); }
            // A 2 px gap in the panel colour between layers.
            let mut sb = PathBuilder::stroke(px(2.));
            sb.move_to(point(px(xs[0]), px(top[0])));
            for i in 1..n { sb.line_to(point(px(xs[i]), px(top[i]))); }
            if let Ok(path) = sb.build() { window.paint_path(path, pp.bg); }
            below = top;
        }
        if let Some(f) = hover {
            let x = x0 + w * f;
            window.paint_quad(fill(Bounds::new(point(px(x), px(y0)), size(px(1.), px(h))), pp.text3));
        }
    }).size_full()
}

/// Lines (response time p50 and p95), values in ms.
fn line_chart(p: &Pal, lines: Vec<(Vec<f64>, Hsla)>) -> impl IntoElement {
    let pp = *p;
    let max = lines.iter().flat_map(|(v, _)| v.iter().copied()).fold(0., f64::max).max(1.) * 1.2;
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

    // Memory over time.
    let series = arr(&m["series"]);
    let first = series.iter().position(|s| s["php"].as_f64().unwrap_or(0.) + s["services"].as_f64().unwrap_or(0.) > 0.).unwrap_or(0);
    let series: Vec<Value> = series[first..].to_vec();
    let rows: Vec<[f64; 4]> = series.iter().map(|s| [s["php"].as_f64().unwrap_or(0.), s["sites"].as_f64().unwrap_or(0.), s["services"].as_f64().unwrap_or(0.), s["web"].as_f64().unwrap_or(0.)]).collect();
    let group_now = |f: &dyn Fn(&str) -> bool| -> u64 { procs.iter().filter(|x| f(&txt(&x["group"]))).filter_map(|x| x["memory"].as_u64()).sum() };
    let now_vals = [group_now(&|g| g == "php"), group_now(&|g| g.starts_with("site:")), group_now(&|g| g == "service"), group_now(&|g| g == "web")];
    let labels = ["PHP pools", "Site processes", "Services", "Web server"];
    let legend = div().flex().flex_row().flex_wrap().gap(px(26.)).px(px(18.)).pt(px(16.))
        .children((0..4).map(|k| div().flex().flex_row().items_center().gap(px(8.)).text_size(px(14.)).text_color(pp.text2)
            .child(div().size(px(10.)).rounded(px(3.)).bg(pp.series[k])).child(labels[k]).child(div().font_weight(FontWeight::SEMIBOLD).text_color(pp.text).child(mb(now_vals[k])))));
    let range = if t.range.is_empty() { "1h".to_string() } else { t.range.clone() };
    let range_label = match range.as_str() { "15m" => "last 15 minutes", "24h" => "last 24 hours", _ => "last hour" };
    let hover = t.hover_x;
    let cell: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
    let chart_area: AnyElement = if rows.len() >= 2 {
        let c3 = cell.clone();
        let tip = hover.map(|f| {
            let i = ((rows.len() - 1) as f32 * f).round() as usize;
            let r = rows[i.min(rows.len() - 1)];
            let tsec = series.get(i).and_then(|s| s["t"].as_u64()).unwrap_or(0);
            let ago_s = m["now"].as_u64().unwrap_or(tsec).saturating_sub(tsec);
            (f, r, if ago_s < 30 { "now".to_string() } else { format!("{} ago", ago(ago_s)) })
        });
        div().id("memchart").relative().h(px(220.)).mx(px(18.)).mt(px(34.)).mb(px(26.))
            .on_mouse_move(move |ev: &MouseMoveEvent, _, cx| {
                if let Some(b) = c3.get() {
                    let f = (f32::from(ev.position.x - b.origin.x) / f32::from(b.size.width)).clamp(0., 1.);
                    cx.global_mut::<Tez>().hover_x = Some(f);
                    cx.refresh_windows();
                }
            })
            .on_hover(|h, _, cx| if !*h { cx.global_mut::<Tez>().hover_x = None; cx.refresh_windows(); })
            .child(stacked_chart(p, rows.clone(), hover, cell.clone()))
            .child(div().absolute().left(px(-4.)).top(px(-18.)).text_size(px(12.)).text_color(pp.text3).child(mb((rows.iter().map(|r| r.iter().sum::<f64>()).fold(0., f64::max) * 1.15) as u64)))
            .child(div().absolute().left_0().bottom(px(-22.)).text_size(px(12.)).text_color(pp.text3).child(series.first().and_then(|s| s["t"].as_u64()).map(|t0| format!("{} ago", ago(m["now"].as_u64().unwrap_or(t0).saturating_sub(t0)))).unwrap_or_default()))
            .child(div().absolute().right_0().bottom(px(-22.)).text_size(px(12.)).text_color(pp.text3).child("now"))
            .when_some(tip, |d, (f, r, when)| {
                let total: f64 = r.iter().sum();
                let left = if f > 0.6 { relative(f - 0.28) } else { relative(f + 0.02) };
                d.child(div().absolute().top(px(6.)).left(left).w(px(220.)).p(px(12.)).rounded(px(12.)).bg(pp.surface2).border_1().border_color(pp.line2).shadow_lg()
                    .flex().flex_col().gap(px(4.)).text_size(px(13.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{when} · {}", mb(total as u64))))
                    .children((0..4).map(|k| div().flex().flex_row().justify_between().text_color(pp.text2).child(labels[k]).child(mb(r[k] as u64)))))
            })
            .into_any_element()
    } else {
        div().h(px(120.)).mx(px(18.)).mt(px(14.)).flex().items_center().justify_center().child(muted(p, "Collecting memory samples. The graph fills in over the next minute.")).into_any_element()
    };
    let total = machine["total"].as_u64().unwrap_or(0);
    let avail = machine["available"].as_u64().unwrap_or(0);
    let mem_card = cardbox(p).pb(px(14.))
        .child(div().flex().flex_row().items_center().px(px(18.)).pt(px(16.))
            .child(div().flex_1().flex().flex_col().gap(px(2.)).child(strong(p, "Memory")).child(muted(p, format!("used by tez, {range_label}"))))
            .child(div().text_size(px(26.)).child(mb(tez_mem))))
        .child(chart_area).child(legend);
    let mrow = |label: &str, value: String, color: Hsla| div().flex().flex_row().justify_between().px(px(18.)).py(px(8.)).text_size(px(14.5))
        .child(div().text_color(color).child(label.to_string())).child(value);
    let other = total.saturating_sub(avail).saturating_sub(tez_mem).saturating_sub(wsl);
    let mut machine_card = cardbox(p).pb(px(10.))
        .child(div().flex().flex_col().gap(px(2.)).px(px(18.)).pt(px(16.)).pb(px(8.)).child(strong(p, "The machine"))
            .child(muted(p, if total > 0 { format!("{} of memory in all", mb(total)) } else { "memory and disk".into() })));
    if total > 0 {
        machine_card = machine_card
            .child(div().mx(px(18.)).my(px(6.)).flex().flex_row().h(px(8.)).rounded(px(4.)).overflow_hidden().gap(px(2.))
                .children([(tez_mem, pp.text), (wsl, pp.amber), (other, pp.line2), (avail, pp.surface3)].into_iter().filter(|(v, _)| *v > 0)
                    .map(|(v, c)| div().h_full().w(relative(v as f32 / total as f32)).bg(c))))
            .child(mrow("tez", mb(tez_mem), pp.text))
            .when(wsl > 0, |d| d.child(mrow("WSL", mb(wsl), pp.amber)))
            .child(mrow("Other apps", mb(other), pp.text2))
            .child(mrow("Free", mb(avail), pp.green));
    }
    machine_card = machine_card.child(div().mx(px(18.)).my(px(6.)).h(px(1.)).bg(pp.cardline))
        .child(mrow("Disk free", mb(disk_free), if disk_free < 20 * (1 << 30) { pp.amber } else { pp.text }))
        .when(cache > 0, |d| d.child(div().flex().flex_row().items_center().pl(px(18.)).pr(px(8.)).py(px(4.)).text_size(px(14.5))
            .child(div().flex_1().text_color(pp.text2).child(format!("Download cache {}", mb(cache))))
            .child(text_btn("mc-clear", &pp, "Clear").on_click(|_, _, cx| send(cx, "/api/cache/clear".into(), json!({}), "Cleared the download cache")))));
    out = out.child(div().flex().flex_row().gap(px(14.)).items_start().child(mem_card.flex_1().min_w(px(0.))).child(machine_card.flex_none().w(px(320.))));

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
        let ctl = if declared { Some(switch(sid(format!("hcs-{name}")), p, up_cfg).on_click(move |_, _, cx| send(cx, format!("/api/site/{n2}/{}", if up_cfg { "down" } else { "up" }), json!({}), if up_cfg { "Stopping processes" } else { "Starting processes" })).into_any_element()) } else { None };
        grid.push(cardbox(p)
            .child(card_header(p, sid(format!("hc-{name}")), "folder", &name, &status, if running { Tone::Ok } else { Tone::Neutral }, ctl, Some(Page::Site(name.clone()))))
            .child(stat_strip(p, vec![
                (if has_req { format!("{rpm:.0} / min") } else { "—".into() }, "requests", None),
                (if has_req { mm["p95_ms"].as_f64().filter(|v| *v > 0.).map(ms).unwrap_or("—".into()) } else { "—".into() }, "p95 response", None),
                (if has_req { format!("{errs:.0}") } else { "—".into() }, "errors, 1 h", if errs > 0. { Some(pp.red) } else { None }),
            ]))
            .child(div().flex().flex_row().items_center().gap(px(2.)).px(px(8.)).py(px(6.))
                .child(text_btn(sid(format!("hco-{name}")), p, "Open site").on_click(move |_, _, _| api::open_url(&url)))
                .when_some(horizon, |d, u| d.child(text_btn(sid(format!("hch-{name}")), &pp, "Horizon").on_click(move |_, _, _| api::open_url(&u))))
                .child(text_btn(sid(format!("hct-{name}")), p, "Terminal").on_click(move |_, _, _| api::open_terminal(&path)))
                .child(div().flex_1())
                .child(div().pr(px(10.)).text_size(px(13.5)).text_color(pp.text3).child(if mem > 0 { mb(mem) } else { "PHP pool only".into() }))).into_any_element());
    }
    if sites.is_empty() { grid.push(muted(p, "No sites yet. Add one from the list on the left.").into_any_element()); }
    out = out.child(div().flex().flex_col().gap(px(14.)).child(div().flex().flex_row().items_baseline().gap(px(12.)).child(h2(p, "Sites")).child(muted(p, format!("{} in your Sites folders", sites.len())))).child(grid_of(grid, 2)));
    // Services as cards.
    let svc_meta = arr(&t.services["services"]);
    let smx = t.svc_metrics.clone();
    let mut sgrid: Vec<AnyElement> = Vec::new();
    for x in procs.iter().filter(|x| x["group"] == "service") {
        let name = txt(&x["name"]);
        let meta = svc_meta.iter().find(|m| txt(&m["name"]) == name).cloned().unwrap_or(Value::Null);
        let title = { let t = txt(&meta["title"]); if t.is_empty() { pool_name(&name) } else { t } };
        let kind = { let k = txt(&meta["kind"]); if k.is_empty() { name.split('@').next().unwrap_or_default().to_string() } else { k } };
        let st = txt(&x["state"]);
        let on = x["wanted"].as_bool().unwrap_or(true);
        let stat = smx[&name].clone();
        let key_stat: (String, &'static str) = match kind.as_str() {
            "postgres" => (arr(&stat["databases"]).len().to_string(), "databases"),
            "redis" => (stat["ops_per_sec"].as_u64().unwrap_or(0).to_string(), "ops / s"),
            "mailpit" => (stat["unread"].as_u64().unwrap_or(0).to_string(), "unread"),
            "meilisearch" => (stat["indexes"].as_u64().unwrap_or(0).to_string(), "indexes"),
            _ => (x["restarts"].as_u64().unwrap_or(0).to_string(), "restarts"),
        };
        let status = format!("{}{}", state_label(&st), x["port"].as_u64().map(|p| format!(" · :{p}")).unwrap_or_default());
        let (n2, sel) = (name.clone(), name.clone());
        sgrid.push(cardbox(p)
            .child(card_header(p, sid(format!("svh-{name}")), pages::service_icon(&kind), &title, &status, state_tone(&st),
                Some(switch(sid(format!("svs-{name}")), p, on).on_click(move |_, _, cx| send(cx, format!("/api/service/{n2}"), json!({ "enabled": !on }), if on { "Stopped; data kept" } else { "Starting" })).into_any_element()), None))
            .child(stat_strip(p, vec![(mb(x["memory"].as_u64().unwrap_or(0)), "memory", None), (format!("{:.1}%", x["cpu"].as_f64().unwrap_or(0.)), "CPU", None), (key_stat.0, key_stat.1, None)]))
            .child(div().flex().flex_row().px(px(8.)).py(px(6.)).child(text_btn(sid(format!("svd-{name}")), p, "Details").on_click(move |_, _, cx| { cx.global_mut::<Tez>().svc_sel = sel.clone(); go(cx, Page::Services); }))).into_any_element());
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
        .child(div().flex().flex_row().flex_wrap().items_center().gap(px(6.)).text_size(px(15.5)).text_color(p.text2)
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
    div().flex().flex_col().gap(px(30.)).child(head).child(body).into_any_element()
}

fn site_overview(p: &Pal, name: &str, s: &Value, d: &Value, live: &[Value], window: &mut Window, cx: &mut App) -> AnyElement {
    let pp = *p;
    // Run a command here.
    let key = format!("cmd:{name}");
    let k2 = key.clone();
    let input = text_input(window, cx, &key, &local(cx, &key), "migrate, queue:retry all, test --parallel, or composer / npm …", false, Some(Rc::new(move |v, cx| set_local(cx, &k2, v))));
    let (n1, k3) = (name.to_string(), key.clone());
    let runner = div().flex().flex_row().items_center().gap(px(10.)).h(px(54.)).pl(px(18.)).pr(px(8.)).rounded_full().bg(p.surface2)
        .child(div().font_family(MONO).text_size(px(13.5)).text_color(p.text3).child("php artisan"))
        .child(div().flex_1().child(input_el(&input, true).appearance(false)))
        .child(div().id("run-cmd").size(px(38.)).rounded_full().bg(p.text).flex().items_center().justify_center().cursor_pointer()
            .child(icon("arrow-up", 17., p.bg))
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
    let mut out = div().flex().flex_col().gap(px(30.)).child(runner)
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
        let kpi = |v: String, l: &str| div().flex_1().flex().flex_col().gap(px(2.)).child(div().text_size(px(26.)).child(v)).child(muted(&pp, l.to_string()));
        perf = perf.child(div().flex().flex_row().gap(px(16.)).px(px(18.)).pt(px(18.))
                .child(kpi(mm["p95_ms"].as_f64().map(ms).unwrap_or("—".into()), "p95 response, 5 min"))
                .child(kpi(p50.last().copied().map(ms).unwrap_or("—".into()), "p50 response"))
                .child(kpi(format!("{:.0} / min", mm["rpm"].as_f64().unwrap_or(0.)), "requests"))
                .child(kpi(format!("{e5:.0}"), "server errors, 1 h")))
            .child(div().flex().flex_row().gap(px(18.)).px(px(18.)).pt(px(14.)).text_size(px(13.5)).text_color(pp.text2)
                .child(div().flex().flex_row().items_center().gap(px(8.)).child(div().w(px(14.)).h(px(2.)).bg(pp.series[0])).child("p95"))
                .child(div().flex().flex_row().items_center().gap(px(8.)).child(div().w(px(14.)).h(px(2.)).bg(pp.series[1])).child("p50")))
            .child(div().h(px(130.)).mx(px(18.)).mt(px(8.)).child(line_chart(p, vec![(p95, pp.series[0]), (p50, pp.series[1])])));
        let routes = arr(&mm["routes"]);
        if !routes.is_empty() {
            perf = perf.child(div().flex().flex_row().gap(px(12.)).px(px(18.)).pt(px(22.)).pb(px(10.)).border_b_1().border_color(pp.line).text_size(px(13.5)).text_color(pp.text3)
                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).text_color(pp.text).text_size(px(15.)).child("Slowest routes")).child(div().w(px(70.)).child("Hits")).child(div().w(px(90.)).child("p95")).child(div().w(px(90.)).child("Worst")));
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
    out = out.child(process_rows(p, name, d, live));
    let installed: Vec<String> = arr(&tez(cx).status["php"]).iter().map(|v| txt(&v["minor"])).collect();
    let isolated = s["isolated"].as_bool().unwrap_or(false);
    let php_now = txt(&s["php"]);
    let mut opts = vec![(String::new(), format!("Automatic (PHP {php_now})"))];
    for m in &installed { opts.push((m.clone(), format!("PHP {m}"))); }
    let n2 = name.to_string();
    let php_sel = pages::select(&format!("php-{name}"), p, cx, if isolated { &php_now } else { "" }, opts, 220., Rc::new(move |v, cx| send(cx, format!("/api/site/{n2}"), json!({ "php": if v.is_empty() { Value::Null } else { Value::String(v) } }), "Saved; PHP switched")));
    let env_val = |k: &str| arr(&d["env"]).iter().flat_map(|g| arr(&g["keys"])).find(|x| txt(&x["key"]) == k).map(|x| txt(&x["value"])).unwrap_or_default();
    let db = env_val("DB_DATABASE");
    let branch = api::git_branch(&txt(&s["path"])).unwrap_or_else(|| "Not a git repository".into());
    let domains = arr(&d["domains"]).len();
    out = out.child(div().flex().flex_col().child(group_title(p, "Site", None))
        .child(field(p, "PHP version", Some("Read from composer.json unless you pick one".into()), false, php_sel))
        .child(field(p, "Addresses", Some(format!("{name}.test and {name}.localhost always work")), false, div().text_color(pp.text2).child(if domains == 0 { "No extra domains".to_string() } else { format!("{domains} extra") })))
        .child(field(p, "Database", Some(env_val("DB_CONNECTION")), false, div().text_color(pp.text2).child(if db.is_empty() { "None set".into() } else { db })))
        .child(field(p, "Git", Some("Current branch".into()), false, div().font_family(MONO).text_color(pp.text2).child(branch))));
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
        ("This computer", vec![("https", "HTTPS and hosts", "shield-check"), ("ports", "Ports", "globe"), ("performance", "Performance", "gauge")]),
        ("Accounts", vec![("github", "GitHub", "github")]),
        ("About", vec![("updates", "Updates", "download")]),
    ];
    let search = text_input(window, cx, "setq", &q, "Search settings", false, Some(Rc::new(|v, cx| set_local(cx, "setq", v))));
    let mut nav = div().w(px(320.)).flex_none().flex().flex_col().gap(px(2.)).p(px(20.)).border_r_1().border_color(p.line)
        .child(div().px(px(10.)).text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child("Settings"))
        .child(div().my(px(12.)).h(px(44.)).px(px(16.)).rounded_full().border_1().border_color(p.line2).flex().flex_row().items_center().gap(px(10.))
            .child(icon("search", 16., p.text3)).child(div().flex_1().child(input_el(&search, false).appearance(false))));
    for (g, items) in &groups {
        let shown: Vec<_> = items.iter().filter(|(_, l, _)| q.is_empty() || l.to_lowercase().contains(&q)).collect();
        if shown.is_empty() { continue; }
        nav = nav.child(div().px(px(10.)).pt(px(16.)).pb(px(6.)).text_size(px(15.)).text_color(p.text3).child(g.to_string()));
        for (key, label, ic) in shown {
            let on = *key == sec;
            let k = key.to_string();
            nav = nav.child(div().id(sid(format!("set-{key}"))).flex().flex_row().items_center().gap(px(12.)).h(px(40.)).px(px(10.)).rounded(px(10.)).cursor_pointer()
                .when(on, |d| d.bg(pp.surface3)).hover(move |s| s.bg(pp.surface2))
                .child(icon(ic, 18., pp.text2)).child(label.to_string())
                .on_click(move |_, _, cx| { cx.global_mut::<Tez>().settings_sec = k.clone(); cx.refresh_windows(); }));
        }
    }
    let (title, lead, body) = settings_section(p, &sec, window, cx);
    div().absolute().top_0().left_0().size_full().occlude()
        .child(div().id("sheet-dim").absolute().top_0().left_0().size_full().bg(rgba(0x0000008c))
            .on_click(|_, _, cx| { cx.global_mut::<Tez>().settings_open = false; cx.refresh_windows(); }))
        .child(div().absolute().top(px(56.)).bottom(px(36.)).left(px(56.)).right(px(56.)).flex().flex_row().bg(p.bg).border_1().border_color(p.line2).rounded(px(18.)).shadow_lg().overflow_hidden()
            .child(nav)
            .child(div().id("sheet-body").flex_1().min_w(px(0.)).relative().overflow_y_scroll()
                .child(div().absolute().right(px(18.)).top(px(18.)).child(icon_btn("sheet-close", p, "x", false, false).on_click(|_, _, cx| { cx.global_mut::<Tez>().settings_open = false; cx.refresh_windows(); })))
                .child(div().px(px(56.)).pt(px(48.)).pb(px(48.)).flex().flex_col().gap(px(8.))
                    .child(div().text_size(px(32.)).child(title)).child(sub(p, lead)).child(div().mt(px(26.)).flex().flex_col().gap(px(30.)).child(body)))))
        .into_any_element()
}

fn settings_section(p: &Pal, sec: &str, window: &mut Window, cx: &mut App) -> (String, String, AnyElement) {
    let t = tez(cx);
    let s = t.settings.clone();
    let pp = *p;
    let trusted = s["ca_trusted"].as_bool().unwrap_or(false);
    let flag = |k: &str| s[k].as_bool().unwrap_or(false);
    let toggle = |id: &'static str, key: &'static str, on: bool, ok: &'static str| switch(id, &pp, on).on_click(move |_, _, cx| send(cx, "/api/settings".into(), json!({ key: !on }), ok));
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
            ("Ports".into(), "Where the web server listens".into(), div().flex().flex_col().child(group_title(p, "Web server", None))
                .child(field(p, "HTTPS port", Some(note), false, div().w(px(120.)).child(input_el(&si, true))))
                .child(field(p, "HTTP port", None, false, div().w(px(120.)).child(input_el(&hi, true)))).into_any_element())
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
            let php_sel = pages::select("defphp", p, cx, &txt(&s["default_php"]), installed.iter().map(|m| (m.clone(), format!("PHP {m}"))).collect(), 220., Rc::new(|v, cx| send(cx, "/api/settings".into(), json!({ "default_php": v }), "Default PHP changed")));
            let tld = crate::draft(cx, "set:tld", txt(&s["tld"]));
            let tld_in = text_input(window, cx, "set:tld", &tld, "test", false, Some(Rc::new(|v, cx| crate::queue(cx, "set:tld".into(), v.clone(), "/api/settings".into(), json!({ "tld": v })))));
            let parked: Vec<String> = arr(&s["parked"]).iter().map(txt).collect();
            let folders = text_input(window, cx, "set:parked", &crate::draft(cx, "set:parked", parked.join("; ")), "", false, Some(Rc::new(|v, cx| {
                let list: Vec<String> = v.split(';').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
                crate::queue(cx, "set:parked".into(), v.clone(), "/api/settings".into(), json!({ "parked": list }));
            })));
            ("General".into(), "Defaults for every site on this computer".into(), div().flex().flex_col().gap(px(30.))
                .child(div().flex().flex_col().child(group_title(p, "Defaults", None))
                    .child(field(p, "Default PHP", Some("For sites that do not ask for a version, and php in a terminal".into()), false, php_sel))
                    .child(field(p, "Domain ending", Some("Sites answer at name.<ending>, and always at name.localhost".into()), false, div().w(px(160.)).child(input_el(&tld_in, true))))
                    .child(field(p, "Sites folders", Some("Every folder inside becomes a site. Separate several with ;".into()), false, div().w_full().child(input_el(&folders, true)))))
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
    let name_el = div().id(id).flex_1().min_w(px(0.)).flex().flex_col().gap(px(3.)).when(clickable, |d| d.cursor_pointer())
        .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).truncate().child(title.to_string()))
        .child(div().flex().flex_row().items_center().gap(px(6.)).text_size(px(13.5)).text_color(fg).child(dot(fg)).child(div().truncate().child(status.to_string())))
        .when_some(target, |d, t| d.on_click(move |_, _, cx| go(cx, t.clone())));
    div().flex().flex_row().items_start().gap(px(12.)).px(px(16.)).pt(px(16.)).pb(px(12.))
        .child(div().size(px(36.)).flex_none().rounded(px(10.)).bg(p.surface3).flex().items_center().justify_center().child(icon(ic, 18., p.text2)))
        .child(name_el)
        .when_some(control, |d, c| d.child(c))
}

/// Running processes as cards; the rest in one "Can also run" list, each with its switch.
fn process_rows(p: &Pal, name: &str, d: &Value, live: &[Value]) -> AnyElement {
    let pp = *p;
    let mut cards: Vec<AnyElement> = Vec::new();
    let mut idle: Vec<AnyElement> = Vec::new();
    let mut running = 0;
    for pr in arr(&d["processes"]) {
        let pn = txt(&pr["name"]);
        let on = pr["on"].as_bool().unwrap_or(false);
        let lv = live.iter().find(|x| txt(&x["name"]) == format!("{name}/{pn}")).cloned();
        let about = { let a = txt(&pr["about"]); if a.is_empty() { txt(&pr["command"]) } else { a } };
        let (site, p2) = (name.to_string(), pn.clone());
        let sw = switch(sid(format!("pon-{pn}")), p, on).on_click(move |_, _, cx| send(cx, format!("/api/site/{site}"), json!({ "processes": { p2.clone(): !on } }), "Saved"));
        match lv {
            Some(x) => {
                running += 1;
                let st = txt(&x["state"]);
                let mem = x["memory"].as_u64().unwrap_or(0);
                let status = format!("{}{}", state_label(&st), x["uptime"].as_u64().filter(|u| *u > 0).map(|u| format!(" · up {}", ago(u))).unwrap_or_default());
                let dash = Some(txt(&x["url"])).filter(|u| !u.is_empty());
                let full = format!("{name}/{pn}");
                let (site2, p3) = (name.to_string(), pn.clone());
                cards.push(cardbox(p)
                    .child(card_header(p, sid(format!("ph-{pn}")), pages::proc_icon(&pn), &cap(&pn), &status, state_tone(&st), Some(sw.into_any_element()), None))
                    .child(div().px(px(16.)).pb(px(12.)).text_size(px(14.)).text_color(pp.text3).truncate().child(about))
                    .child(stat_strip(p, vec![
                        (mb(mem), "memory", if mem > 1 << 30 { Some(pp.amber) } else { None }),
                        (format!("{:.1}%", x["cpu"].as_f64().unwrap_or(0.)), "CPU", None),
                        (x["restarts"].as_u64().unwrap_or(0).to_string(), "restarts", if x["restarts"].as_u64().unwrap_or(0) > 0 { Some(pp.amber) } else { None }),
                    ]))
                    .child(div().flex().flex_row().gap(px(2.)).px(px(8.)).py(px(6.))
                        .when_some(dash, |d, u| d.child(text_btn(sid(format!("pd-{pn}")), &pp, "Dashboard").on_click(move |_, _, _| api::open_url(&u))))
                        .child(text_btn(sid(format!("pr-{pn}")), p, "Restart").on_click(move |_, _, cx| send(cx, format!("/api/proc/{full}/restart"), json!({}), "Restarting")))
                        .child(text_btn(sid(format!("pl-{pn}")), p, "Log").on_click(move |_, _, cx| { set_local(cx, &format!("log:{site2}"), p3.clone()); cx.global_mut::<Tez>().site_tab = "logs".into(); refresh_now(cx, false); }))).into_any_element());
            }
            None => {
                idle.push(div().flex().flex_row().items_center().gap(px(14.)).px(px(16.)).py(px(11.)).border_t_1().border_color(pp.cardline)
                    .child(div().size(px(30.)).flex_none().rounded(px(9.)).bg(pp.surface3).flex().items_center().justify_center().child(icon(pages::proc_icon(&pn), 15., pp.text3)))
                    .child(div().flex_1().min_w(px(0.)).flex().flex_row().items_baseline().gap(px(8.)).child(div().font_weight(FontWeight::SEMIBOLD).child(cap(&pn))).child(div().text_size(px(14.)).text_color(pp.text3).truncate().child(about)))
                    .when(on, |d| d.child(div().text_size(px(13.5)).text_color(pp.text3).child("starts with the site")))
                    .child(sw)
                    .into_any_element());
            }
        }
    }
    let mut g = div().flex().flex_col().gap(px(14.))
        .child(div().flex().flex_row().items_baseline().gap(px(12.)).child(h2(p, "Processes")).child(muted(p, format!("{running} running"))));
    if running > 0 { g = g.child(grid_of(cards, 3)); }
    if !idle.is_empty() {
        g = g.child(cardbox(p).child(div().px(px(16.)).pt(px(14.)).pb(px(8.)).text_size(px(14.)).text_color(pp.text3).child(if running > 0 { "Can also run with this site" } else { "Runs with this site when turned on" })).children(idle));
    }
    g.into_any_element()
}
/// Cards in fixed columns, so a short last row keeps the same card width.
fn grid_of(items: Vec<AnyElement>, cols: usize) -> Div {
    let mut out = div().flex().flex_col().gap(px(14.));
    let mut it = items.into_iter().peekable();
    while it.peek().is_some() {
        let mut row = div().flex().flex_row().gap(px(14.)).items_start();
        for _ in 0..cols {
            row = match it.next() { Some(e) => row.child(div().flex_1().min_w(px(0.)).child(e)), None => row.child(div().flex_1()) };
        }
        out = out.child(row);
    }
    out
}