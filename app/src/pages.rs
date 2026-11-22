//! Every screen, built from the parts in ui.rs, laid out as on the design canvas.

use crate::ui::*;
use crate::{api, clone_repo, draft, go, job, load_repos, local, notify, queue, refresh_now, send, set_local, site_log_choice, tez, txt, Page, Tez};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{json, Value};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type Pick = Rc<dyn Fn(String, &mut App)>;

pub(crate) fn sid(s: impl Into<String>) -> ElementId { ElementId::Name(SharedString::from(s.into())) }

pub(crate) fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() { Some(f) => f.to_uppercase().collect::<String>() + c.as_str(), None => String::new() }
}

pub(crate) fn now_secs() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.) }

pub(crate) fn arr(v: &Value) -> Vec<Value> { v.as_array().cloned().unwrap_or_default() }

pub(crate) fn proc_icon(name: &str) -> &'static str {
    match name {
        "horizon" => "layers",
        "reverb" => "radio",
        "vite" => "zap",
        "queue" => "list-checks",
        "schedule" | "scheduler" => "clock",
        _ => "terminal",
    }
}

pub(crate) fn service_icon(kind: &str) -> &'static str {
    match kind {
        "postgres" | "mysql" | "mariadb" | "mongodb" => "database",
        "redis" => "layers",
        "mailpit" => "mail",
        "meilisearch" => "search",
        "rustfs" | "minio" => "hard-drive",
        _ => "server",
    }
}

pub(crate) fn service_tone(kind: &str) -> Tone {
    match kind {
        "postgres" | "mysql" | "mariadb" => Tone::Info,
        "redis" => Tone::Err,
        "mailpit" | "mongodb" => Tone::Ok,
        "meilisearch" => Tone::Accent,
        _ => Tone::Warn,
    }
}

// ------------------------------------------------------------------ the frame

pub(crate) fn header(p: &Pal, title: &str, subtitle: &str, actions: Vec<AnyElement>) -> Div {
    div().flex().flex_row().flex_wrap().items_end().gap(px(16.))
        .child(div().flex_1().min_w(px(300.)).flex().flex_col().gap(px(4.)).child(h1(p, title.to_string())).child(sub(p, subtitle.to_string())))
        .child(div().flex().flex_row().flex_wrap().gap(px(8.)).children(actions))
}

pub(crate) fn empty_state(p: &Pal, icon_name: &str, title: &str, body: &str, actions: Vec<AnyElement>) -> Div {
    card(p).items_center().justify_center().py(px(48.)).px(px(32.)).gap(px(12.))
        .child(tile(p, Tone::Accent, 56., None, Some(icon_name)))
        .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
        .child(div().max_w(px(380.)).text_color(p.text2).text_center().child(body.to_string()))
        .child(div().mt(px(6.)).flex().flex_row().flex_wrap().justify_center().gap(px(8.)).children(actions))
}

/// A dropdown that opens below its trigger and closes on any outside click.
pub(crate) fn select(id: &str, p: &Pal, cx: &App, current: &str, options: Vec<(String, String)>, width: f32, on_pick: Pick) -> Div {
    let open = tez(cx).open_select.as_deref() == Some(id);
    let label = options.iter().find(|(v, _)| v == current).map(|(_, l)| l.clone()).unwrap_or_else(|| current.to_string());
    let pp = *p;
    let idc = id.to_string();
    let trigger = div().id(sid(format!("sel-{id}"))).w(px(width)).h(px(34.)).px(px(10.)).rounded(px(8.)).border_1()
        .border_color(if open { p.accent_hi } else { p.line2 }).bg(p.bg).flex().flex_row().items_center().justify_between().gap(px(8.))
        .cursor_pointer().text_size(px(13.)).text_color(p.text)
        .child(div().flex_1().truncate().child(label))
        .child(icon("chevron-down", 14., p.text3))
        .on_click(move |_, _, cx| {
            let t = cx.global_mut::<Tez>();
            let just_closed = t.select_closed.as_ref().map(|(i, at)| *i == idc && at.elapsed() < Duration::from_millis(300)).unwrap_or(false);
            t.open_select = if t.open_select.as_deref() == Some(idc.as_str()) || just_closed { None } else { Some(idc.clone()) };
            cx.refresh_windows();
        });
    let mut d = div().flex().flex_col().child(trigger);
    if open {
        let idm = id.to_string();
        let menu = div().occlude().w(px(width.max(200.))).mt(px(4.)).p(px(4.)).rounded(px(10.)).bg(p.surface2).border_1().border_color(p.line2)
            .shadow_lg().flex().flex_col()
            .on_mouse_down_out(move |_, _, cx| {
                let t = cx.global_mut::<Tez>();
                if t.open_select.as_deref() == Some(idm.as_str()) { t.open_select = None; t.select_closed = Some((idm.clone(), Instant::now())); }
                cx.refresh_windows();
            })
            .children(options.into_iter().map(|(v, l)| {
                let on = v == current;
                let pick = on_pick.clone();
                let vv = v.clone();
                div().id(sid(format!("opt-{id}-{v}"))).flex().flex_row().items_center().gap(px(8.)).h(px(32.)).px(px(10.)).rounded(px(7.))
                    .text_size(px(13.)).cursor_pointer().text_color(pp.text).hover(move |s| s.bg(pp.surface3))
                    .child(div().flex_1().child(l))
                    .when(on, |d| d.child(icon("check", 14., pp.accent_text)))
                    .on_click(move |_, _, cx| {
                        cx.global_mut::<Tez>().open_select = None;
                        pick(vv.clone(), cx);
                        cx.refresh_windows();
                    })
            }));
        d = d.child(deferred(anchored().snap_to_window().child(menu)).with_priority(1));
    }
    d
}

pub(crate) fn copy_btn(id: &str, p: &Pal, text: String) -> Stateful<Div> {
    button(sid(id.to_string()), p, Kind::Ghost, true, Some("copy"), None)
        .on_click(move |_, _, cx| { cx.write_to_clipboard(ClipboardItem::new_string(text.clone())); notify(cx, "Copied"); })
}

// ------------------------------------------------------------------ offline and jump

pub(crate) fn offline(p: &Pal, cx: &App) -> AnyElement {
    let t = tez(cx);
    let (title, body) = if t.ever_online || !t.error.is_empty() {
        ("tez is not running", "Sites and services are stopped. Your files and databases are untouched.")
    } else {
        ("Connecting to tez…", "Starting the tez daemon in the background.")
    };
    div().pt(px(80.)).child(empty_state(p, "power", title, body, vec![
        button("off-start", p, Kind::Primary, false, Some("play"), Some("Start tez")).on_click(|_, _, cx| { api::start_daemon(); notify(cx, "Starting tez"); }).into_any_element(),
    ]).child(muted(p, "or run tez start in a terminal"))).into_any_element()
}

pub(crate) fn jump_overlay(p: &Pal, window: &mut Window, cx: &mut App) -> AnyElement {
    let q = local(cx, "jumpq").to_lowercase();
    let input = text_input(window, cx, "jump", "", "Go to a site, service, PHP version or page", false, Some(Rc::new(|v, cx| set_local(cx, "jumpq", v))));
    let focus = input.read(cx).focus_handle(cx);
    if !focus.is_focused(window) { window.focus(&focus, cx); }
    let t = tez(cx);
    let mut items: Vec<(String, String, Page)> = vec![
        ("Overview".into(), "layout-dashboard".into(), Page::Overview),
        ("Sites".into(), "globe".into(), Page::Sites),
        ("New site".into(), "plus".into(), Page::NewSite),
        ("Services".into(), "database".into(), Page::Services),
        ("PHP".into(), "code-xml".into(), Page::Php),
        ("Dumps".into(), "bug".into(), Page::Dumps),
        ("Mail".into(), "mail".into(), Page::Mail),
        ("Logs".into(), "scroll-text".into(), Page::Logs),
        ("Settings".into(), "settings".into(), Page::Settings),
    ];
    for s in arr(&t.status["sites"]) { items.push((txt(&s["name"]), "globe".into(), Page::Site(txt(&s["name"])))); }
    let items: Vec<_> = items.into_iter().filter(|(l, _, _)| q.is_empty() || l.to_lowercase().contains(&q)).take(12).collect();
    let pp = *p;
    div().absolute().top_0().left_0().size_full().bg(rgba(0x00000080)).flex().flex_col().items_center().pt(px(110.))
        .occlude()
        .child(div().w(px(540.)).bg(p.surface).border_1().border_color(p.line2).rounded(px(14.)).shadow_lg().flex().flex_col()
            .on_mouse_down_out(|_, _, cx| { cx.global_mut::<Tez>().jump = false; cx.refresh_windows(); })
            .child(div().p(px(12.)).border_b_1().border_color(p.line).child(input_el(&input, false)))
            .child(div().p(px(6.)).flex().flex_col().children(items.into_iter().enumerate().map(move |(i, (label, ic, page))| {
                div().id(sid(format!("jump-{i}"))).flex().flex_row().items_center().gap(px(10.)).h(px(36.)).px(px(10.)).rounded(px(8.))
                    .cursor_pointer().hover(move |s| s.bg(pp.surface2))
                    .child(icon(&ic, 15., pp.text3)).child(label)
                    .on_click(move |_, _, cx| { set_local(cx, "jumpq", String::new()); go(cx, page.clone()); })
            }))))
        .into_any_element()
}

// ------------------------------------------------------------------ overview

pub(crate) fn site_procs(procs: &[Value], site: &str) -> Vec<Value> {
    procs.iter().filter(|x| txt(&x["group"]) == format!("site:{site}")).cloned().collect()
}

pub(crate) fn site_row(p: &Pal, s: &Value, procs: &[Value]) -> AnyElement {
    let name = txt(&s["name"]);
    let url = txt(&s["url"]);
    let path = txt(&s["path"]);
    let declared: Vec<String> = s["processes"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    let live = site_procs(procs, &name);
    let up = s["up"].as_bool().unwrap_or(false);
    let memory: u64 = live.iter().filter_map(|x| x["memory"].as_u64()).sum();
    let mut pills: Vec<AnyElement> = Vec::new();
    if !live.is_empty() {
        for x in &live {
            let short = txt(&x["name"]).rsplit('/').next().unwrap_or_default().to_string();
            let st = txt(&x["state"]);
            let label = if st == "running" { cap(&short) } else { format!("{} {}", cap(&short), state_label(&st).to_lowercase()) };
            pills.push(pill(p, state_tone(&st), label, true).into_any_element());
        }
    } else if !declared.is_empty() {
        for d in &declared { pills.push(pill(p, Tone::Neutral, format!("{} stopped", cap(d)), true).into_any_element()); }
    } else {
        pills.push(muted(p, "No background processes").into_any_element());
    }
    let target = Page::Site(name.clone());
    let (u1, path1, n1, n2) = (url.clone(), path.clone(), name.clone(), name.clone());
    div().flex().flex_row().items_center().gap(px(16.)).px(px(18.)).py(px(13.))
        .child(div().id(sid(format!("row-{name}"))).w(px(250.)).flex_none().flex().flex_row().items_center().gap(px(12.)).cursor_pointer()
            .child(tile(p, site_tone(&name), 34., Some(&name[..1]), None))
            .child(div().min_w(px(0.)).flex().flex_col()
                .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(name.clone()))
                .child(div().font_family(MONO).text_size(px(12.)).text_color(p.accent_text).truncate().child(url.trim_start_matches("https://").to_string())))
            .on_click(move |_, _, cx| go(cx, target.clone())))
        .child(div().w(px(78.)).flex_none().child(chip(p, format!("PHP {}", txt(&s["php"])))))
        .child(div().flex_1().min_w(px(0.)).flex().flex_row().flex_wrap().gap(px(6.)).children(pills))
        .child(div().w(px(70.)).flex_none().child(muted(p, if memory > 0 { mb(memory) } else { "Served".into() })))
        .child(div().flex().flex_row().flex_none().gap(px(2.))
            .child(button(sid(format!("open-{name}")), p, Kind::Ghost, true, Some("external-link"), None).on_click(move |_, _, _| api::open_url(&u1)))
            .child(button(sid(format!("term-{name}")), p, Kind::Ghost, true, Some("terminal"), None).on_click(move |_, _, _| api::open_terminal(&path1)))
            .when(!declared.is_empty(), |d| d.child(if up {
                button(sid(format!("stop-{name}")), p, Kind::Secondary, true, Some("square"), Some("Stop")).w(px(76.))
                    .on_click(move |_, _, cx| send(cx, format!("/api/site/{n1}/down"), json!({}), "Stopping processes"))
            } else {
                button(sid(format!("start-{name}")), p, Kind::Secondary, true, Some("play"), Some("Start")).w(px(76.))
                    .on_click(move |_, _, cx| send(cx, format!("/api/site/{n2}/up"), json!({}), "Starting processes"))
            })))
        .into_any_element()
}

pub(crate) fn sites_card(p: &Pal, cx: &App, title: &str) -> Div {
    let t = tez(cx);
    let sites = arr(&t.status["sites"]);
    let procs = arr(&t.status["procs"]);
    let parked = arr(&t.settings["parked"]).first().map(txt).unwrap_or_else(|| "~/Sites".into());
    let parked2 = parked.clone();
    if sites.is_empty() {
        return empty_state(p, "globe", "No sites yet", &format!("Any folder you put in {parked} shows up here with its own address. Or start from a repository."), vec![
            button("empty-gh", p, Kind::Primary, false, Some("github"), Some("From GitHub")).on_click(|_, _, cx| go(cx, Page::NewSite)).into_any_element(),
            button("empty-folder", p, Kind::Ghost, false, Some("folder-open"), Some("Open the Sites folder")).on_click(move |_, _, _| api::open_path(&parked2)).into_any_element(),
        ]);
    }
    card(p)
        .child(card_head(p, title).child(muted(p, format!("{} in {parked}", sites.len()))).child(div().flex_1())
            .child(button("sites-folder", p, Kind::Ghost, true, Some("folder-open"), Some("Open folder")).on_click(move |_, _, _| api::open_path(&parked2)))
            .child(button("sites-add", p, Kind::Secondary, true, Some("plus"), Some("Add")).on_click(|_, _, cx| go(cx, Page::NewSite))))
        .child(rows(p, sites.iter().map(|s| site_row(p, s, &procs)).collect()))
}

pub(crate) fn sites_page(p: &Pal, cx: &App) -> AnyElement {
    div().flex().flex_col().gap(px(22.))
        .child(header(p, "Sites", "Every folder in your Sites folders, each at name.test and name.localhost.", vec![
            button("sp-new", p, Kind::Primary, false, Some("plus"), Some("New site")).on_click(|_, _, cx| go(cx, Page::NewSite)).into_any_element(),
        ]))
        .child(sites_card(p, cx, "All sites"))
        .into_any_element()
}

// ------------------------------------------------------------------ a site

pub(crate) fn site_processes(p: &Pal, name: &str, d: &Value, live: &[Value], cx: &App) -> AnyElement {
    let list = arr(&d["processes"]);
    let mut cards: Vec<AnyElement> = Vec::new();
    for pr in &list {
        let pn = txt(&pr["name"]);
        let on = pr["on"].as_bool().unwrap_or(false);
        let lv = live.iter().find(|x| txt(&x["name"]) == format!("{name}/{pn}")).cloned();
        let st = lv.as_ref().map(|x| txt(&x["state"])).unwrap_or_else(|| "stopped".into());
        let about = { let a = txt(&pr["about"]); if a.is_empty() { txt(&pr["command"]) } else { a } };
        let (s1, s2, p1, p2) = (name.to_string(), name.to_string(), pn.clone(), pn.clone());
        let full = format!("{name}/{pn}");
        let dash = lv.as_ref().map(|x| txt(&x["url"])).filter(|u| !u.is_empty());
        let mut stats: Vec<String> = Vec::new();
        if let Some(x) = &lv {
            if let Some(u) = x["uptime"].as_u64() { if u > 0 { stats.push(ago(u)); } }
            if let Some(m) = x["memory"].as_u64() { if m > 0 { stats.push(mb(m)); } }
            stats.push(format!("{} restarts", x["restarts"].as_u64().unwrap_or(0)));
        }
        let log_choice = pn.clone();
        let site_for_log = name.to_string();
        cards.push(card(p).flex_grow(1.).flex_basis(px(300.)).min_w(px(280.)).p(px(16.)).gap(px(12.))
            .child(div().flex().flex_row().items_center().gap(px(10.))
                .child(tile(p, if st == "running" { Tone::Accent } else { Tone::Neutral }, 30., None, Some(proc_icon(&pn))))
                .child(div().flex_1().min_w(px(0.)).flex().flex_col().child(strong(p, cap(&pn))).child(muted(p, about).truncate()))
                .child(if lv.is_some() { pill(p, state_tone(&st), state_label(&st), true) } else { pill(p, Tone::Neutral, if on { "Off" } else { "Not started" }, true) }))
            .child(if stats.is_empty() { muted(p, if on { "Starts with the site's processes." } else { "Off. Turn it on to run it with this site." }) } else { muted(p, stats.join(" · ")) })
            .child(div().flex().flex_row().flex_wrap().items_center().gap(px(6.))
                .when_some(dash, |d, u| d.child(button(sid(format!("dash-{pn}")), p, Kind::Secondary, true, Some("external-link"), Some("Dashboard")).on_click(move |_, _, _| api::open_url(&u))))
                .when(lv.is_some(), |d| d.child(button(sid(format!("rs-{pn}")), p, Kind::Secondary, true, None, Some("Restart"))
                    .on_click(move |_, _, cx| send(cx, format!("/api/proc/{full}/restart"), json!({}), "Restarting"))))
                .child(button(sid(format!("log-{pn}")), p, Kind::Ghost, true, None, Some("Log"))
                    .on_click(move |_, _, cx| { set_local(cx, &format!("log:{site_for_log}"), log_choice.clone()); refresh_now(cx, false); }))
                .child(div().flex_1())
                .child(muted(p, "Runs with site"))
                .child(switch(sid(format!("on-{pn}")), p, on).on_click(move |_, _, cx| {
                    let _ = (&s1, &p1);
                    send(cx, format!("/api/site/{s2}"), json!({ "processes": { p2.clone(): !on } }), "Saved");
                })))
            .into_any_element());
    }
    let sel = site_log_choice(cx, name);
    let key = if sel == "laravel.log" { format!("site:{name}") } else { format!("proc:{name}/{sel}") };
    let text = tez(cx).logs.get(&key).cloned().unwrap_or_default();
    let mut opts: Vec<(String, String)> = list.iter().map(|x| { let n = txt(&x["name"]); (n.clone(), cap(&n)) }).collect();
    opts.push(("laravel.log".into(), "laravel.log".into()));
    let site = name.to_string();
    let pick: Pick = Rc::new(move |v, cx| { set_local(cx, &format!("log:{site}"), v); refresh_now(cx, false); });
    let mut out = div().flex().flex_col().gap(px(16.));
    if cards.is_empty() {
        out = out.child(card(p).p(px(18.)).child(muted(p, "This site has no background processes. Horizon, Reverb, Vite, queues and the scheduler appear here when the project uses them.")));
    } else {
        out = out.child(div().flex().flex_row().flex_wrap().gap(px(12.)).children(cards));
    }
    out.child(card(p)
        .child(card_head(p, "Live log").child(seg("logsel", p, &opts, &sel, pick)).child(div().flex_1()).child(copy_btn("copy-live", p, text.clone())))
        .child(div().p(px(14.)).child(code_block(p, &text, 40))))
        .into_any_element()
}

pub(crate) fn site_env(p: &Pal, name: &str, d: &Value, window: &mut Window, cx: &mut App) -> AnyElement {
    let groups = arr(&d["env"]);
    let q = local(cx, &format!("envq:{name}")).to_lowercase();
    let raw = local(cx, &format!("envraw:{name}")) == "raw";
    let total: usize = groups.iter().map(|g| arr(&g["keys"]).len()).sum();
    let nq = name.to_string();
    let filter = text_input(window, cx, &format!("envfilter:{name}"), &q, &format!("Filter {total} keys"), false, Some(Rc::new(move |v, cx| set_local(cx, &format!("envq:{nq}"), v))));
    let nr = name.to_string();
    let view_pick: Pick = Rc::new(move |v, cx| set_local(cx, &format!("envraw:{nr}"), v));
    let top = div().flex().flex_row().flex_wrap().items_center().gap(px(10.))
        .child(div().w(px(320.)).child(input_el(&filter, false)))
        .child(seg("envview", p, &[("grouped".into(), "Grouped".into()), ("raw".into(), "Raw .env".into())], if raw { "raw" } else { "grouped" }, view_pick))
        .child(div().flex_1())
        .child(div().flex().flex_row().items_center().gap(px(6.)).child(icon("check", 14., p.green)).child(muted(p, "Saved to .env as you type · previous copy in .env.tez-backup")));
    let mut out = div().flex().flex_col().gap(px(16.)).child(top);
    if raw {
        let text: String = groups.iter().flat_map(|g| arr(&g["keys"])).map(|k| {
            let v = if k["secret"].as_bool() == Some(true) { "••••••••".to_string() } else { txt(&k["value"]) };
            format!("{}={}", txt(&k["key"]), v)
        }).collect::<Vec<_>>().join("\n");
        return out.child(card(p).p(px(14.)).child(code_block(p, &text, 400))).into_any_element();
    }
    for (gi, g) in groups.iter().enumerate() {
        let title = txt(&g["title"]);
        let keys: Vec<Value> = arr(&g["keys"]).into_iter().filter(|k| q.is_empty() || txt(&k["key"]).to_lowercase().contains(&q) || txt(&k["value"]).to_lowercase().contains(&q)).collect();
        if keys.is_empty() { continue; }
        let tkey = format!("env:{name}:{title}");
        let default_open = gi < 2;
        let open = !q.is_empty() || (default_open != tez(cx).toggled.contains(&tkey));
        let secrets = keys.iter().filter(|k| k["secret"].as_bool() == Some(true)).count();
        let tk = tkey.clone();
        let head = div().id(sid(format!("envg-{gi}"))).flex().flex_row().items_center().gap(px(12.)).px(px(18.)).py(px(14.)).cursor_pointer()
            .when(open, |d| d.border_b_1().border_color(p.line))
            .child(icon(if open { "chevron-down" } else { "chevron-right" }, 14., p.text3))
            .child(h2(p, title.clone()))
            .child(muted(p, format!("{} keys", keys.len())))
            .when(secrets > 0, |d| d.child(pill(p, Tone::Accent, format!("{secrets} secret{}", if secrets == 1 { "" } else { "s" }), false)))
            .on_click(move |_, _, cx| { let t = cx.global_mut::<Tez>(); if !t.toggled.remove(&tk) { t.toggled.insert(tk.clone()); } cx.refresh_windows(); });
        let mut c = card(p).child(head);
        if open {
            let mut items: Vec<AnyElement> = Vec::new();
            for k in keys {
                let key = txt(&k["key"]);
                let saved = txt(&k["value"]);
                if k["secret"].as_bool() == Some(true) {
                    items.push(secret_field(p, &format!("env:{name}:{key}"), &key, &saved, format!("/api/site/{name}/env"), "Only in this project's .env on this computer, never logged.", window, cx));
                } else {
                    let dk = format!("env:{name}:{key}");
                    let value = draft(cx, &dk, saved);
                    let (dk2, site, k2) = (dk.clone(), name.to_string(), key.clone());
                    let st = text_input(window, cx, &dk, &value, "", false, Some(Rc::new(move |v, cx| {
                        queue(cx, dk2.clone(), v.clone(), format!("/api/site/{site}/env"), json!({ "set": { k2.clone(): v } }));
                    })));
                    items.push(field(p, &key, None, true, div().w_full().child(input_el(&st, true))).into_any_element());
                }
            }
            c = c.child(rows(p, items));
        }
        out = out.child(c);
    }
    out.into_any_element()
}

/// A masked value with show, copy and its own Save: secrets never save as you type.
pub(crate) fn secret_field(p: &Pal, key: &str, label: &str, saved: &str, path: String, where_kept: &str, window: &mut Window, cx: &mut App) -> AnyElement {
    let st = text_input(window, cx, key, saved, "", true, None);
    let current = st.read(cx).value().to_string();
    let edited = current != saved;
    let (s1, s2) = (st.clone(), st.clone());
    let saved_v = saved.to_string();
    let lbl = label.to_string();
    let copy_v = current.clone();
    let mut ctrl = div().w_full().flex().flex_col().gap(px(8.))
        .child(div().flex().flex_row().gap(px(6.))
            .child(div().flex_1().child(input_el(&st, true).mask_toggle()))
            .child(button(sid(format!("copy-{key}")), p, Kind::Secondary, false, Some("copy"), None)
                .on_click(move |_, _, cx| { cx.write_to_clipboard(ClipboardItem::new_string(copy_v.clone())); notify(cx, "Copied"); })));
    if edited {
        ctrl = ctrl.child(div().flex().flex_row().items_center().gap(px(6.))
            .child(pill(p, Tone::Warn, "Edited, not saved", false))
            .child(div().flex_1())
            .child(button(sid(format!("cancel-{key}")), p, Kind::Ghost, true, None, Some("Cancel"))
                .on_click(move |_, window, cx| { let v = saved_v.clone(); s1.update(cx, |i, cx| i.set_value(v, window, cx)); }))
            .child(button(sid(format!("save-{key}")), p, Kind::Primary, true, None, Some("Save"))
                .on_click(move |_, _, cx| {
                    let v = s2.read(cx).value().to_string();
                    let body = if path.contains("/env") { json!({ "set": { lbl.clone(): v } }) } else { json!({ "options": { lbl.clone(): v } }) };
                    send(cx, path.clone(), body, "Saved; restart processes to apply");
                })));
    }
    div().flex().flex_row().flex_wrap().items_start().gap(px(16.)).px(px(18.)).py(px(13.))
        .child(div().flex_1().min_w(px(220.)).flex().flex_col().gap(px(3.))
            .child(div().flex().flex_row().items_center().gap(px(8.))
                .child(div().font_family(MONO).text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(label.to_string()))
                .child(div().flex().flex_row().items_center().gap(px(4.)).h(px(20.)).px(px(7.)).rounded_full().bg(p.accent_soft).text_color(p.accent_text).text_size(px(11.5))
                    .child(icon("lock", 11., p.accent_text)).child("Secret")))
            .child(muted(p, where_kept.to_string())))
        .child(div().w(px(320.)).child(ctrl))
        .into_any_element()
}

pub(crate) fn site_domains(p: &Pal, name: &str, d: &Value, window: &mut Window, cx: &mut App) -> AnyElement {
    let list: Vec<String> = arr(&d["domains"]).iter().map(txt).collect();
    let tld = txt(&tez(cx).status["tld"]);
    let key = format!("newdomain:{name}");
    let k2 = key.clone();
    let st = text_input(window, cx, &key, &local(cx, &key), "admin.myapp.test", false, Some(Rc::new(move |v, cx| set_local(cx, &k2, v))));
    let mut chips: Vec<AnyElement> = Vec::new();
    for dom in &list {
        let rest: Vec<String> = list.iter().filter(|x| *x != dom).cloned().collect();
        let n = name.to_string();
        chips.push(div().flex().flex_row().items_center().gap(px(6.)).h(px(30.)).pl(px(10.)).pr(px(4.)).rounded(px(8.)).border_1().border_color(p.line2)
            .child(div().font_family(MONO).text_size(px(12.5)).child(dom.clone()))
            .child(button(sid(format!("rmdom-{dom}")), p, Kind::Ghost, true, Some("x"), None)
                .on_click(move |_, _, cx| send(cx, format!("/api/site/{n}"), json!({ "domains": rest.clone() }), "Saved")))
            .into_any_element());
    }
    let (n, all, k3) = (name.to_string(), list.clone(), key.clone());
    card(p)
        .child(card_head(p, "Extra domains"))
        .child(div().p(px(18.)).flex().flex_col().gap(px(14.))
            .child(muted(p, format!("{name}.{tld} and {name}.localhost always work. Add more for apps that route by host name.")))
            .child(div().flex().flex_row().flex_wrap().gap(px(8.)).children(chips))
            .child(div().flex().flex_row().gap(px(8.))
                .child(div().w(px(340.)).child(input_el(&st, true)))
                .child(button("adddom", p, Kind::Secondary, false, Some("plus"), Some("Add domain")).on_click(move |_, _, cx| {
                    let v = local(cx, &k3).trim().to_string();
                    if v.is_empty() { return; }
                    let mut l = all.clone(); l.push(v);
                    set_local(cx, &k3, String::new());
                    send(cx, format!("/api/site/{n}"), json!({ "domains": l }), "Saved; tez serves it now");
                }))))
        .into_any_element()
}

pub(crate) fn site_config(p: &Pal, path: &str, d: &Value) -> AnyElement {
    let files: Vec<String> = arr(&d["config_files"]).iter().map(txt).collect();
    let important = ["horizon.php", "reverb.php", "queue.php", "broadcasting.php", "database.php", "cache.php", "session.php", "mail.php", "filesystems.php", "app.php"];
    let mut list: Vec<String> = files.iter().filter(|f| important.contains(&f.as_str())).cloned().collect();
    list.extend(files.iter().filter(|f| !important.contains(&f.as_str())).cloned());
    let sep = if cfg!(windows) { "\\" } else { "/" };
    let items: Vec<AnyElement> = list.iter().map(|f| {
        let full = format!("{path}{sep}config{sep}{f}");
        let full2 = full.clone();
        div().flex().flex_row().items_center().gap(px(12.)).px(px(18.)).py(px(11.))
            .child(icon("file-code", 15., p.text3))
            .child(div().flex_1().font_family(MONO).text_size(px(12.5)).child(format!("config/{f}")))
            .child(button(sid(format!("cfg-ed-{f}")), p, Kind::Secondary, true, None, Some("Open in editor")).on_click(move |_, _, _| api::open_editor(&full)))
            .child(button(sid(format!("cfg-op-{f}")), p, Kind::Ghost, true, None, Some("Open")).on_click(move |_, _, _| api::open_path(&full2)))
            .into_any_element()
    }).collect();
    card(p).child(card_head(p, "Config files").child(muted(p, "Horizon supervisors, Reverb servers, queues and the rest")))
        .child(if items.is_empty() { div().p(px(18.)).child(muted(p, "No config folder.")) } else { rows(p, items) })
        .into_any_element()
}

// ------------------------------------------------------------------ new site

pub(crate) fn new_site(p: &Pal, window: &mut Window, cx: &mut App) -> AnyElement {
    let source = { let s = tez(cx).new_source.clone(); if s.is_empty() { "github".to_string() } else { s } };
    let src_pick: Pick = Rc::new(|v, cx| { cx.global_mut::<Tez>().new_source = v; cx.refresh_windows(); });
    let srcs = seg("newsrc", p, &[("github".into(), "GitHub".into()), ("url".into(), "Git URL".into()), ("import".into(), "Import a folder".into())], &source, src_pick);
    let head = div().flex().flex_row().flex_wrap().items_end().gap(px(16.))
        .child(div().flex_1().min_w(px(300.)).flex().flex_col().gap(px(4.)).child(h1(p, "New site")).child(sub(p, "Start from GitHub, any git URL, or a project already on this computer.")))
        .child(srcs);
    let body: AnyElement = match source.as_str() {
        "url" => {
            let url = text_input(window, cx, "clone-url", &local(cx, "clone-url"), "git@github.com:owner/repo.git or https://…", false, Some(Rc::new(|v, cx| set_local(cx, "clone-url", v))));
            let folder = text_input(window, cx, "clone-url-name", &local(cx, "clone-url-name"), "Folder name (optional)", false, Some(Rc::new(|v, cx| set_local(cx, "clone-url-name", v))));
            card(p).p(px(18.)).gap(px(14.))
                .child(h2(p, "Clone any git repository"))
                .child(muted(p, "GitLab, Bitbucket, self-hosted, or an SSH alias from ~/.ssh/config such as git@github.com-work:owner/repo.git"))
                .child(input_el(&url, true))
                .child(div().w(px(320.)).child(input_el(&folder, true)))
                .child(div().child(button("clone-url-go", p, Kind::Primary, false, Some("download"), Some("Clone and set up")).on_click(|_, _, cx| {
                    let u = local(cx, "clone-url");
                    if u.trim().is_empty() { notify(cx, "Paste a git URL first"); return; }
                    let f = local(cx, "clone-url-name");
                    clone_repo(cx, u.trim().to_string(), f);
                })))
                .into_any_element()
        }
        "import" => {
            let path = text_input(window, cx, "import-path", &local(cx, "import-path"), if cfg!(windows) { "/home/me/code/myapp (WSL) or D:\\code\\myapp" } else { "/path/to/myapp" }, false, Some(Rc::new(|v, cx| set_local(cx, "import-path", v))));
            card(p).p(px(18.)).gap(px(14.))
                .child(h2(p, "Copy a project into your Sites folder"))
                .child(muted(p, "tez measures it first, leaves out vendor, node_modules, caches and any folder over 1 GB, and never fills the disk. The original stays where it is."))
                .child(input_el(&path, true))
                .child(div().child(button("import-go", p, Kind::Primary, false, Some("download"), Some("Import and set up")).on_click(|_, _, cx| {
                    let pth = local(cx, "import-path");
                    if pth.trim().is_empty() { notify(cx, "Enter the folder to import"); return; }
                    job(cx, &["import", pth.trim()], &format!("Import {}", pth.trim()), "Importing");
                })))
                .into_any_element()
        }
        _ => new_from_github(p, window, cx),
    };
    div().flex().flex_col().gap(px(20.)).child(head).child(body).into_any_element()
}

pub(crate) fn new_from_github(p: &Pal, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = tez(cx);
    let accounts = arr(&t.identities["accounts"]);
    let owners: Vec<String> = arr(&t.github["owners"]).iter().map(txt).collect();
    let account = t.gh_account.clone();
    let owner = t.gh_owner.clone();
    let loading = t.gh_loading;
    let repos = arr(&t.repos["repos"]);
    let route = t.gh_route.clone();
    let ssh_hosts = arr(&t.identities["ssh_hosts"]);
    let selected = local(cx, "sel-repo");
    let q = local(cx, "repoq").to_lowercase();
    let tld = txt(&t.status["tld"]);
    let site_names: Vec<String> = arr(&t.status["sites"]).iter().map(|s| txt(&s["name"])).collect();
    let pp = *p;

    let acct_btns: Vec<AnyElement> = accounts.iter().map(|a| {
        let login = txt(&a["login"]);
        let on = login == account;
        let l2 = login.clone();
        div().id(sid(format!("acct-{login}"))).flex().flex_row().items_center().gap(px(8.)).h(px(38.)).px(px(12.)).rounded(px(8.)).border_1().cursor_pointer()
            .border_color(if on { pp.accent_hi } else { pp.line2 }).bg(if on { pp.accent_soft } else { pp.surface })
            .child(div().size(px(22.)).rounded_full().bg(if on { pp.accent } else { pp.surface3 }).flex().items_center().justify_center()
                .text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(if on { white() } else { pp.text2 }).child(login[..1].to_uppercase()))
            .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(login.clone()))
            .when(on, |d| d.child(icon("check", 14., pp.accent_text)))
            .on_click(move |_, _, cx| {
                let t = cx.global_mut::<Tez>();
                t.gh_account = l2.clone();
                t.gh_owner.clear();
                let r = t.identities["routes"].as_array().and_then(|r| r.iter().find(|x| txt(&x["login"]) == l2)).map(|x| txt(&x["route"]));
                if let Some(r) = r { t.gh_route = r; }
                load_repos(cx);
            })
            .into_any_element()
    }).collect();
    let owner_pick: Pick = Rc::new(|v, cx| { cx.global_mut::<Tez>().gh_owner = v; load_repos(cx); });
    let owner_sel = select("owner", p, cx, &owner, owners.iter().map(|o| (o.clone(), o.clone())).collect(), 190., owner_pick);
    let search = text_input(window, cx, "repoq", &q, "Search repositories", false, Some(Rc::new(|v, cx| set_local(cx, "repoq", v))));
    let filtered: Vec<Value> = repos.iter().filter(|r| q.is_empty() || txt(&r["nameWithOwner"]).to_lowercase().contains(&q) || txt(&r["description"]).to_lowercase().contains(&q)).take(60).cloned().collect();
    let repo_rows: Vec<AnyElement> = filtered.iter().map(|r| {
        let full = txt(&r["nameWithOwner"]);
        let on = full == selected;
        let lang = txt(&r["primaryLanguage"]["name"]);
        let desc = txt(&r["description"]);
        let private = r["isPrivate"].as_bool() == Some(true);
        let updated = txt(&r["updatedAt"]).chars().take(10).collect::<String>();
        let f2 = full.clone();
        let short = txt(&r["name"]);
        div().id(sid(format!("repo-{full}"))).flex().flex_row().items_center().gap(px(12.)).px(px(18.)).py(px(11.)).cursor_pointer()
            .when(on, |d| d.bg(pp.accent_soft)).hover(move |s| s.bg(pp.surface2))
            .child(icon(if on { "circle-check" } else { "git-branch" }, 15., if on { pp.accent_text } else { pp.text3 }))
            .child(div().flex_1().min_w(px(0.)).flex().flex_col()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(short.clone()))
                .child(muted(&pp, [lang, desc].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")).truncate()))
            .when(private, |d| d.child(pill(&pp, Tone::Neutral, "Private", false)))
            .child(div().w(px(84.)).flex().justify_end().child(muted(&pp, updated)))
            .on_click(move |_, _, cx| { set_local(cx, "sel-repo", f2.clone()); set_local(cx, "clone-name", String::new()); })
            .into_any_element()
    }).collect();

    let left = card(p).flex_grow(1.).flex_basis(px(460.)).min_w(px(420.))
        .child(div().p(px(18.)).flex().flex_col().gap(px(14.)).border_b_1().border_color(p.line)
            .child(div().flex().flex_col().gap(px(8.)).child(muted(p, "Account, from the GitHub CLI"))
                .child(div().flex().flex_row().flex_wrap().gap(px(8.)).children(acct_btns)
                    .child(button("acct-more", p, Kind::Ghost, false, Some("plus"), Some("Sign in another")).h(px(38.)).on_click(|_, _, cx| go(cx, Page::Settings)))))
            .child(div().flex().flex_row().gap(px(8.)).child(owner_sel).child(div().flex_1().child(input_el(&search, false)))))
        .child(if loading { div().p(px(18.)).child(muted(p, "Loading repositories…")).into_any_element() }
            else if accounts.is_empty() { div().p(px(18.)).child(muted(p, "No GitHub account is signed in. Sign in under Settings, or run gh auth login.")).into_any_element() }
            else if repo_rows.is_empty() { div().p(px(18.)).flex().flex_col().gap(px(10.)).child(muted(p, "No repositories found.")).child(div().child(button("repos-reload", p, Kind::Secondary, true, Some("refresh-cw"), Some("Reload")).on_click(|_, _, cx| load_repos(cx)))).into_any_element() }
            else { div().id("repo-list").max_h(px(520.)).overflow_y_scroll().child(rows(p, repo_rows)).into_any_element() });

    let right: AnyElement = if selected.is_empty() {
        card(p).flex_grow(1.).flex_basis(px(340.)).min_w(px(320.)).p(px(24.)).gap(px(8.)).items_center().justify_center()
            .child(icon("github", 28., p.text3)).child(strong(p, "Pick a repository")).child(muted(p, "Its clone settings appear here.")).into_any_element()
    } else {
        let short = selected.rsplit('/').next().unwrap_or_default().to_string();
        let default_folder = if site_names.contains(&short) { (2..).map(|i| format!("{short}-{i}")).find(|n| !site_names.contains(n)).unwrap_or_default() } else { short.clone() };
        let typed = local(cx, "clone-name");
        let folder = if typed.is_empty() { default_folder.clone() } else { typed };
        let clash = site_names.contains(&folder);
        let fstate = text_input(window, cx, &format!("clone-name:{selected}"), &folder, "", false, Some(Rc::new(|v, cx| set_local(cx, "clone-name", v))));
        let mut route_opts: Vec<(String, String)> = ssh_hosts.iter().map(|h| (format!("ssh:{}", txt(&h["alias"])), format!("SSH · {}", txt(&h["alias"])))).collect();
        for a in &accounts { route_opts.push((format!("gh:{}", txt(&a["login"])), format!("HTTPS through gh · {}", txt(&a["login"])))); }
        let route_pick: Pick = Rc::new(|v, cx| { cx.global_mut::<Tez>().gh_route = v; cx.refresh_windows(); });
        let key_file = ssh_hosts.iter().find(|h| route == format!("ssh:{}", txt(&h["alias"]))).map(|h| txt(&h["identity"])).unwrap_or_default();
        let route_sel = select("route", p, cx, &route, route_opts, 300., route_pick);
        let (repo, fold) = (selected.clone(), folder.clone());
        card(p).flex_grow(1.).flex_basis(px(340.)).min_w(px(320.))
            .child(card_head(p, &format!("Clone {selected}")))
            .child(div().p(px(18.)).flex().flex_col().gap(px(16.))
                .child(div().flex().flex_col().gap(px(6.)).child(muted(p, "Route")).child(route_sel)
                    .child(muted(p, if key_file.is_empty() { "Matched to this account from ~/.ssh/config, or HTTPS through gh.".to_string() } else { format!("Key {key_file}, matched from ~/.ssh/config") })))
                .child(div().flex().flex_col().gap(px(6.)).child(muted(p, "Folder in your Sites folder")).child(input_el(&fstate, true))
                    .when(clash, |d| d.child(div().text_size(px(12.5)).text_color(pp.amber).child(format!("{folder} already exists. Pick another name."))))
                    .when(!clash && folder != short, |d| d.child(div().text_size(px(12.5)).text_color(pp.amber).child(format!("{short} already exists, so this copy gets its own folder and address.")))))
                .child(div().flex().flex_row().gap(px(10.))
                    .child(div().flex_1().flex().flex_col().gap(px(6.)).child(muted(p, "Address")).child(chip(p, format!("{folder}.{tld}"))))
                    .child(div().flex_1().flex().flex_col().gap(px(6.)).child(muted(p, "PHP")).child(chip(p, "from composer.json"))))
                .child(div().flex().flex_col().gap(px(6.)).child(muted(p, "After cloning, tez runs"))
                    .children(["Composer packages", ".env, app key and database, then migrations", "npm packages, icon cache and storage link"].iter().map(|s| {
                        div().flex().flex_row().items_center().gap(px(8.)).child(icon("check", 14., pp.green)).child(s.to_string())
                    })))
                .child(button("clone-go", p, Kind::Primary, false, Some("download"), Some("Clone and set up")).h(px(38.)).on_click(move |_, _, cx| {
                    if clash { notify(cx, "That folder already exists"); return; }
                    clone_repo(cx, repo.clone(), fold.clone());
                }))
                .child(div().text_center().child(muted(p, "Runs in the background. Progress shows under Recent activity."))))
            .into_any_element()
    };
    div().flex().flex_row().flex_wrap().items_start().gap(px(16.)).child(left).child(right).into_any_element()
}

// ------------------------------------------------------------------ services

pub(crate) fn services_page(p: &Pal, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = tez(cx);
    let list = arr(&t.services["services"]);
    let procs = arr(&t.status["procs"]);
    let installed: Vec<Value> = list.iter().filter(|s| s["installed"] == true).cloned().collect();
    let missing: Vec<Value> = list.iter().filter(|s| s["installed"] != true && s["available"] != false).cloned().collect();
    let sel = if t.svc_sel.is_empty() || !list.iter().any(|s| txt(&s["name"]) == t.svc_sel) {
        installed.first().or(list.first()).map(|s| txt(&s["name"])).unwrap_or_default()
    } else { t.svc_sel.clone() };
    let pp = *p;
    let item = |s: &Value, is_installed: bool| {
        let name = txt(&s["name"]);
        let active = name == sel;
        let state = procs.iter().find(|x| txt(&x["name"]) == name).map(|x| txt(&x["state"])).unwrap_or_else(|| "stopped".into());
        let port = s["port"].as_u64().map(|x| format!(":{x}")).unwrap_or_default();
        let n2 = name.clone();
        div().id(sid(format!("svc-{name}"))).flex().flex_row().items_center().gap(px(10.)).px(px(10.)).py(px(9.)).rounded(px(8.)).cursor_pointer()
            .when(active, |d| d.bg(pp.surface2)).hover(move |s| s.bg(pp.surface2))
            .child(if is_installed { dot(if state == "running" { pp.green } else { pp.text3 }).into_any_element() } else { icon("plus", 13., pp.text3).into_any_element() })
            .child(div().flex_1().min_w(px(0.)).flex().flex_col()
                .child(div().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).text_color(if active { pp.text } else { pp.text2 }).child(txt(&s["title"])))
                .when(is_installed, |d| d.child(div().font_family(MONO).text_size(px(11.5)).text_color(pp.text3).child(format!("{port}{}", if state == "running" { String::new() } else { " · stopped".into() })))))
            .on_click(move |_, _, cx| { cx.global_mut::<Tez>().svc_sel = n2.clone(); cx.refresh_windows(); })
    };
    let mut nav = card(p).flex_none().w(px(250.)).p(px(6.)).gap(px(2.));
    for s in &installed { nav = nav.child(item(s, true)); }
    if !missing.is_empty() {
        nav = nav.child(div().px(px(10.)).pt(px(14.)).pb(px(6.)).child(label_caps(p, "NOT INSTALLED")));
        for s in &missing { nav = nav.child(item(s, false)); }
    }
    let detail = list.iter().find(|s| txt(&s["name"]) == sel).cloned().map(|s| service_detail(p, &s, &procs, window, cx))
        .unwrap_or_else(|| card(p).p(px(18.)).child(muted(p, "No services yet.")).into_any_element());
    div().flex().flex_col().gap(px(20.))
        .child(header(p, "Services", "Databases, caches and tools shared by every site. Several versions can run side by side.", vec![]))
        .child(div().flex().flex_row().items_start().gap(px(16.)).child(nav).child(div().flex_1().min_w(px(0.)).child(detail)))
        .into_any_element()
}

pub(crate) fn service_detail(p: &Pal, s: &Value, procs: &[Value], window: &mut Window, cx: &mut App) -> AnyElement {
    let name = txt(&s["name"]);
    let kind = txt(&s["kind"]);
    let title = txt(&s["title"]);
    let about = txt(&s["about"]);
    let versions: Vec<String> = arr(&s["versions"]).iter().map(txt).collect();
    if s["installed"] != true {
        let reason = txt(&s["unavailable_reason"]);
        let list = if versions.is_empty() { vec![txt(&s["version"])] } else { versions };
        let k = kind.clone();
        return card(p).p(px(24.)).gap(px(14.))
            .child(div().flex().flex_row().items_center().gap(px(14.)).child(tile(p, service_tone(&kind), 44., None, Some(service_icon(&kind))))
                .child(div().flex().flex_col().child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(title.clone())).child(muted(p, about))))
            .child(if !reason.is_empty() { muted(p, reason).into_any_element() } else {
                div().flex().flex_row().flex_wrap().gap(px(8.)).children(list.into_iter().filter(|v| !v.is_empty()).map(|v| {
                    let (k2, v2) = (k.clone(), v.clone());
                    button(sid(format!("inst-{k}-{v}")), p, Kind::Secondary, false, Some("download"), Some(&format!("Install {v}")))
                        .on_click(move |_, _, cx| send(cx, format!("/api/service/{k2}"), json!({ "install": v2.clone() }), "Installing"))
                })).into_any_element()
            })
            .into_any_element();
    }
    let live = procs.iter().find(|x| txt(&x["name"]) == name).cloned().unwrap_or(Value::Null);
    let state = { let s = txt(&live["state"]); if s.is_empty() { "stopped".to_string() } else { s } };
    let enabled = s["enabled"].as_bool().unwrap_or(false);
    let full = txt(&s["full_version"]);
    let mut stats = Vec::new();
    if let Some(m) = live["memory"].as_u64() { if m > 0 { stats.push(mb(m)); } }
    if let Some(u) = live["uptime"].as_u64() { if u > 0 { stats.push(format!("up {}", ago(u))); } }
    let url = txt(&live["url"]);
    let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
    let mut out = div().flex().flex_col().gap(px(16.))
        .child(card(p).flex_row().flex_wrap().items_center().gap(px(14.)).p(px(18.))
            .child(tile(p, service_tone(&kind), 44., None, Some(service_icon(&kind))))
            .child(div().flex_1().min_w(px(240.)).flex().flex_col().gap(px(2.))
                .child(div().flex().flex_row().items_center().gap(px(10.))
                    .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(format!("{title} {full}")))
                    .child(pill(p, state_tone(&state), state_label(&state), true)))
                .child(muted(p, if stats.is_empty() { about.clone() } else { format!("{} · {about}", stats.join(" · ")) })))
            .when(!url.is_empty(), |d| { let u = url.clone(); d.child(button("svc-open", p, Kind::Secondary, false, Some("external-link"), Some("Open")).on_click(move |_, _, _| api::open_url(&u))) })
            .child(button("svc-restart", p, Kind::Secondary, false, None, Some("Restart")).on_click(move |_, _, cx| send(cx, format!("/api/proc/{n1}/restart"), json!({}), "Restarting"))));

    // Essentials
    let pk = format!("port:{name}");
    let port_now = draft(cx, &pk, s["port"].as_u64().map(|x| x.to_string()).unwrap_or_default());
    let pk2 = pk.clone();
    let port_in = text_input(window, cx, &pk, &port_now, "", false, Some(Rc::new(move |v, cx| {
        if let Ok(n) = v.trim().parse::<u16>() { queue(cx, pk2.clone(), v.clone(), format!("/api/service/{n2}"), json!({ "port": n })); }
    })));
    let k = kind.clone();
    let add_versions: Vec<AnyElement> = versions.iter().map(|v| {
        let (k2, v2) = (k.clone(), v.clone());
        button(sid(format!("addv-{k}-{v}")), p, Kind::Secondary, true, Some("plus"), Some(v)).on_click(move |_, _, cx| send(cx, format!("/api/service/{k2}"), json!({ "install": v2.clone() }), "Installing"))
            .into_any_element()
    }).collect();
    let mut ess: Vec<AnyElement> = vec![
        field(p, "Running", Some("Off stops it for every site and keeps its data".into()), false,
            switch("svc-run", p, enabled).on_click(move |_, _, cx| send(cx, format!("/api/service/{n3}"), json!({ "enabled": !enabled }), if enabled { "Stopped; data kept" } else { "Starting" }))).into_any_element(),
        field(p, "Port", Some("Listens on 127.0.0.1 only. Sites need their .env changed too.".into()), false, div().w(px(120.)).child(input_el(&port_in, true))).into_any_element(),
    ];
    if !add_versions.is_empty() {
        ess.push(field(p, "Other versions", Some("Each runs beside the others on its own port".into()), false, div().flex().flex_row().flex_wrap().justify_end().gap(px(6.)).children(add_versions)).into_any_element());
    }
    out = out.child(card(p).child(card_head(p, "Essentials")).child(rows(p, ess)));

    // Extensions (PostgreSQL)
    if kind == "postgres" {
        let have: Vec<(String, String)> = arr(&s["extensions"]).iter().map(|x| (txt(&x[0]), txt(&x[1]))).collect();
        let items: Vec<AnyElement> = arr(&s["extension_catalog"]).iter().map(|e| {
            let en = txt(&e[0]);
            let installed = have.iter().find(|(n, _)| *n == en).map(|(_, v)| v.clone());
            let (nm, e2) = (name.clone(), en.clone());
            div().flex().flex_row().flex_wrap().items_center().gap(px(12.)).px(px(18.)).py(px(13.))
                .child(div().flex_1().min_w(px(220.)).flex().flex_col().child(strong(p, en.clone())).child(muted(p, txt(&e[1]))))
                .when_some(installed.clone(), |d, v| d.child(chip(p, v)).child(pill(p, Tone::Ok, "Installed", false)))
                .child(button(sid(format!("pgx-{en}")), p, Kind::Secondary, true, if installed.is_some() { None } else { Some("download") }, Some(if installed.is_some() { "Add version" } else { "Install" }))
                    .on_click(move |_, _, cx| send(cx, format!("/api/service/{nm}"), json!({ "extension": e2.clone() }), "Adding the extension")))
                .into_any_element()
        }).collect();
        if !items.is_empty() {
            out = out.child(card(p).child(card_head(p, "Extensions").child(muted(p, "Installed into this version, created per database"))).child(rows(p, items)));
        }
    }

    // Connect and tuning
    let env: String = arr(&s["env"]).iter().map(|x| format!("{}={}", txt(&x[0]), txt(&x[1]))).collect::<Vec<_>>().join("\n");
    let opts = arr(&s["options"]);
    let mut lower = div().flex().flex_row().flex_wrap().items_start().gap(px(16.));
    if !env.is_empty() {
        lower = lower.child(card(p).flex_grow(1.).flex_basis(px(320.)).min_w(px(300.))
            .child(card_head(p, "Connect a site").child(div().flex_1()).child(copy_btn("svc-copy-env", p, env.clone())))
            .child(div().p(px(14.)).flex().flex_col().gap(px(10.)).child(code_block(p, &env, 20))
                .child(muted(p, if kind == "postgres" { "Connections from 127.0.0.1 are trusted, so no password is needed locally." } else { "Paste these into a site's .env." }))));
    }
    if !opts.is_empty() {
        let mut items: Vec<AnyElement> = Vec::new();
        for o in &opts {
            let ok = txt(&o["key"]);
            if o["secret"].as_bool() == Some(true) {
                items.push(secret_field(p, &format!("svc:{name}:{ok}"), &ok, &txt(&o["value"]), format!("/api/service/{name}"), "Stored in tez.json on this computer", window, cx));
            } else {
                let dk = format!("svc:{name}:{ok}");
                let value = draft(cx, &dk, txt(&o["value"]));
                let (dk2, nm, o2) = (dk.clone(), name.clone(), ok.clone());
                let st = text_input(window, cx, &dk, &value, &txt(&o["default"]), false, Some(Rc::new(move |v, cx| {
                    queue(cx, dk2.clone(), v.clone(), format!("/api/service/{nm}"), json!({ "options": { o2.clone(): v } }));
                })));
                items.push(field(p, &ok, Some(txt(&o["about"])), true, div().w(px(160.)).child(input_el(&st, true))).into_any_element());
            }
        }
        lower = lower.child(card(p).flex_grow(1.).flex_basis(px(360.)).min_w(px(320.)).child(card_head(p, "Tuning").child(muted(p, "Restarts the service when saved"))).child(rows(p, items)));
    }
    out.child(lower).into_any_element()
}

// ------------------------------------------------------------------ PHP

const PHP_GROUPS: &[(&str, &[&str])] = &[
    ("Limits", &["post_max_size", "max_input_time", "max_input_vars", "max_file_uploads", "default_socket_timeout"]),
    ("Errors and logging", &["error_reporting", "display_startup_errors", "log_errors"]),
    ("OPcache details", &["opcache.memory_consumption", "opcache.interned_strings_buffer", "opcache.max_accelerated_files", "opcache.validate_timestamps", "opcache.revalidate_freq", "opcache.jit", "opcache.jit_buffer_size", "realpath_cache_size", "realpath_cache_ttl"]),
    ("Xdebug", &["xdebug.mode", "xdebug.start_with_request", "xdebug.client_port"]),
    ("Date, locale, sessions", &["date.timezone", "session.gc_maxlifetime", "output_buffering", "short_open_tag", "allow_url_fopen"]),
];

pub(crate) fn php_page(p: &Pal, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = tez(cx);
    let versions = arr(&t.php["versions"]);
    let available: Vec<String> = arr(&t.php["available"]).iter().map(txt).collect();
    let installed: Vec<String> = versions.iter().map(|v| txt(&v["minor"])).collect();
    let sel = if t.php_sel.is_empty() || !installed.contains(&t.php_sel) {
        versions.iter().find(|v| v["default"] == true).or(versions.first()).map(|v| txt(&v["minor"])).unwrap_or_default()
    } else { t.php_sel.clone() };
    let pp = *p;
    let mut grid: Vec<AnyElement> = versions.iter().map(|v| {
        let minor = txt(&v["minor"]);
        let on = minor == sel;
        let users = arr(&v["sites"]).len();
        let m2 = minor.clone();
        let preview = minor.as_str() >= "8.6";
        card(&pp).id(sid(format!("phpv-{minor}"))).w(px(124.)).flex_none().p(px(12.)).gap(px(2.)).cursor_pointer()
            .when(on, |d| d.border_color(pp.accent_hi).bg(pp.accent_soft))
            .child(div().flex().flex_row().items_center().child(div().flex_1().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(minor.clone()))
                .when(v["default"] == true, |d| d.child(pill(&pp, Tone::Accent, "Default", false)))
                .when(v["default"] != true && preview, |d| d.child(pill(&pp, Tone::Warn, "Preview", false))))
            .child(div().font_family(MONO).text_size(px(12.)).text_color(pp.text3).child(txt(&v["version"])))
            .child(div().mt(px(6.)).child(muted(&pp, match users { 0 => "No sites".to_string(), 1 => arr(&v["sites"]).first().map(txt).unwrap_or_default(), n => format!("{n} sites") })))
            .on_click(move |_, _, cx| { cx.global_mut::<Tez>().php_sel = m2.clone(); cx.refresh_windows(); })
            .into_any_element()
    }).collect();
    for m in available.iter().filter(|m| !installed.contains(m)) {
        let m2 = m.clone();
        grid.push(div().id(sid(format!("phpadd-{m}"))).w(px(124.)).flex_none().p(px(12.)).rounded(px(12.)).border_1().border_dashed().border_color(p.line2)
            .flex().flex_col().gap(px(2.)).cursor_pointer().hover(move |s| s.border_color(pp.accent_hi))
            .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).text_color(p.text2).child(m.clone()))
            .child(div().flex().flex_row().items_center().gap(px(4.)).child(icon("download", 12., p.text3)).child(muted(p, "Install")))
            .on_click(move |_, _, cx| job(cx, &["php", "install", &m2], &format!("Install PHP {m2}"), "Installing PHP"))
            .into_any_element());
    }
    let Some(v) = versions.iter().find(|v| txt(&v["minor"]) == sel).cloned() else {
        return div().flex().flex_col().gap(px(20.)).child(header(p, "PHP", "No PHP version is installed yet.", vec![]))
            .child(div().flex().flex_row().flex_wrap().gap(px(8.)).children(grid)).into_any_element();
    };
    let minor = sel.clone();
    let users: Vec<String> = arr(&v["sites"]).iter().map(txt).collect();
    let catalog: Vec<Value> = arr(&t.php["catalog"]);
    let about_of = |k: &str| catalog.iter().find(|c| txt(&c["key"]) == k).map(|c| txt(&c["about"])).unwrap_or_default();
    let values = v["values"].clone();
    let is_default = v["default"] == true;
    let switchable = t.php["extensions_switchable"].as_bool().unwrap_or(true);
    let pecl_ok = t.php["pecl"].as_bool().unwrap_or(true);
    let xdebug_on = t.settings["xdebug"].as_bool().unwrap_or(false);
    let home = api::home();

    let ini_input = |key: &str, window: &mut Window, cx: &mut App| -> Entity<gpui_kit::component::input::InputState> {
        let dk = format!("php:{minor}:{key}");
        let value = draft(cx, &dk, txt(&values[key]));
        let (dk2, m2, k2) = (dk.clone(), minor.clone(), key.to_string());
        text_input(window, cx, &dk, &value, "", false, Some(Rc::new(move |val, cx| {
            let body = if val.trim().is_empty() { Value::Null } else { Value::String(val.clone()) };
            queue(cx, dk2.clone(), val.clone(), format!("/api/php/{m2}"), json!({ "settings": { k2.clone(): body } }));
        })))
    };

    let (m_def, m_rs) = (minor.clone(), minor.clone());
    let ini_path = home.join("php").join(&minor).join("php.ini").to_string_lossy().to_string();
    let head = card(p).flex_row().flex_wrap().items_center().gap(px(14.)).px(px(18.)).py(px(16.))
        .child(div().flex_1().min_w(px(280.)).flex().flex_col().gap(px(2.))
            .child(div().flex().flex_row().items_center().gap(px(10.)).child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(format!("PHP {}", txt(&v["version"]))))
                .when(is_default, |d| d.child(pill(p, Tone::Accent, "Default", false))))
            .child(muted(p, if users.is_empty() { "No site uses it yet".to_string() } else { format!("Used by {}", users.join(", ")) })))
        .when(!is_default, |d| d.child(button("php-default", p, Kind::Secondary, false, None, Some("Make default"))
            .on_click(move |_, _, cx| send(cx, "/api/settings".into(), json!({ "default_php": m_def.clone() }), "Default PHP changed"))))
        .child(button("php-ini", p, Kind::Secondary, false, Some("file-code"), Some("Open php.ini")).on_click(move |_, _, _| api::open_editor(&ini_path)))
        .child(button("php-restart", p, Kind::Secondary, false, None, Some("Restart workers")).on_click(move |_, _, cx| send(cx, format!("/api/proc/php-{m_rs}/restart"), json!({}), "Restarting PHP")));

    let mut ess: Vec<AnyElement> = Vec::new();
    for key in ["memory_limit", "max_execution_time", "upload_max_filesize"] {
        let st = ini_input(key, window, cx);
        ess.push(field(p, key, Some(about_of(key)), true, div().w(px(130.)).child(input_el(&st, true))).into_any_element());
    }
    let opc = txt(&values["opcache.enable"]);
    let opc_on = opc.is_empty() || opc == "1" || opc.eq_ignore_ascii_case("on");
    let disp = txt(&values["display_errors"]);
    let disp_on = disp.is_empty() || disp == "1" || disp.eq_ignore_ascii_case("on");
    let (m_o, m_d) = (minor.clone(), minor.clone());
    let speed: Vec<AnyElement> = vec![
        field(p, "Xdebug", Some(if pecl_ok { "Off by default, because it slows every request".into() } else { "Not in the static PHP builds on this system".into() }), false,
            switch("php-xdebug", p, xdebug_on).on_click(move |_, _, cx| send(cx, "/api/settings".into(), json!({ "xdebug": !xdebug_on }), "Saved; PHP restarted"))).into_any_element(),
        field(p, "OPcache for web and CLI", Some("Why artisan runs in under a second".into()), false,
            switch("php-opcache", p, opc_on).on_click(move |_, _, cx| send(cx, format!("/api/php/{m_o}"), json!({ "settings": { "opcache.enable": if opc_on { "0" } else { "1" }, "opcache.enable_cli": if opc_on { "0" } else { "1" } } }), "Saved; PHP restarted"))).into_any_element(),
        field(p, "Show errors in the browser", Some("display_errors".into()), false,
            switch("php-display", p, disp_on).on_click(move |_, _, cx| send(cx, format!("/api/php/{m_d}"), json!({ "settings": { "display_errors": if disp_on { "Off" } else { "On" } } }), "Saved; PHP restarted"))).into_any_element(),
    ];

    // Extensions
    let exts = arr(&v["extensions"]);
    let q = local(cx, "extq").to_lowercase();
    let filter = text_input(window, cx, "extq", &q, if pecl_ok { "Filter, or a PECL name to install" } else { "Filter" }, false, Some(Rc::new(|v, cx| set_local(cx, "extq", v))));
    let shown: Vec<Value> = exts.iter().filter(|e| q.is_empty() || txt(&e["name"]).to_lowercase().contains(&q)).cloned().collect();
    let on_count = exts.iter().filter(|e| e["on"] == true).count();
    let ext_cells: Vec<AnyElement> = shown.iter().map(|e| {
        let n = txt(&e["name"]);
        let on = e["on"] == true;
        let (m2, n2) = (minor.clone(), n.clone());
        div().w(px(200.)).flex_grow(1.).flex().flex_row().items_center().gap(px(10.)).px(px(18.)).py(px(10.)).border_b_1().border_color(pp.line)
            .child(div().flex_1().font_family(MONO).text_size(px(12.5)).child(n.clone()))
            .when(e["pecl"] == true, |d| d.child(chip(&pp, "PECL")))
            .child(if switchable {
                switch(sid(format!("ext-{n}")), &pp, on).on_click(move |_, _, cx| send(cx, format!("/api/php/{m2}"), json!({ "extensions": { n2.clone(): !on } }), "Saved; PHP restarted")).into_any_element()
            } else { pill(&pp, Tone::Ok, "Built in", false).into_any_element() })
            .into_any_element()
    }).collect();
    let exact = exts.iter().any(|e| txt(&e["name"]).to_lowercase() == q);
    let m_p = minor.clone();
    let q2 = q.clone();
    let ext_card = card(p)
        .child(card_head(p, "Extensions").child(muted(p, format!("{on_count} on, {} off", exts.len() - on_count))).child(div().flex_1()).child(div().w(px(280.)).child(input_el(&filter, false))))
        .child(div().flex().flex_row().flex_wrap().children(ext_cells))
        .when(pecl_ok && !q.is_empty() && !exact && q.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'), |d| d.child(
            div().flex().flex_row().flex_wrap().items_center().gap(px(12.)).px(px(18.)).py(px(14.))
                .child(icon("download", 14., pp.accent_text))
                .child(div().flex_1().child(format!("Install {q} from PECL for PHP {minor}")))
                .child(button("pecl-go", &pp, Kind::Secondary, true, None, Some("Install"))
                    .on_click(move |_, _, cx| send(cx, format!("/api/php/{m_p}"), json!({ "install": q2.clone() }), "Installing")))));

    // Collapsible groups
    let mut groups = div().flex().flex_col().gap(px(12.));
    for (title, keys) in PHP_GROUPS {
        let tk = format!("php:{minor}:{title}");
        let open = tez(cx).toggled.contains(&tk);
        let tk2 = tk.clone();
        let mut c = card(p).child(div().id(sid(format!("phpg-{title}"))).flex().flex_row().items_center().gap(px(12.)).px(px(18.)).py(px(14.)).cursor_pointer()
            .when(open, |d| d.border_b_1().border_color(pp.line))
            .child(icon(if open { "chevron-down" } else { "chevron-right" }, 14., pp.text3))
            .child(h2(p, title.to_string()))
            .child(muted(p, keys.join(", ")).truncate())
            .on_click(move |_, _, cx| { let t = cx.global_mut::<Tez>(); if !t.toggled.remove(&tk2) { t.toggled.insert(tk2.clone()); } cx.refresh_windows(); }));
        if open {
            let mut items: Vec<AnyElement> = Vec::new();
            for key in keys.iter() {
                let st = ini_input(key, window, cx);
                items.push(field(p, key, Some(about_of(key)), true, div().w(px(180.)).child(input_el(&st, true))).into_any_element());
            }
            c = c.child(rows(p, items));
        }
        groups = groups.child(c);
    }

    div().flex().flex_col().gap(px(20.))
        .child(header(p, "PHP", "Every version side by side. Each site uses the version its composer.json asks for.", vec![]))
        .child(div().flex().flex_row().flex_wrap().gap(px(8.)).children(grid))
        .child(head)
        .child(div().flex().flex_row().flex_wrap().items_start().gap(px(16.))
            .child(card(p).flex_grow(1.).flex_basis(px(360.)).min_w(px(340.)).child(card_head(p, "Essentials").child(muted(p, "Changed most often"))).child(rows(p, ess)))
            .child(card(p).flex_grow(1.).flex_basis(px(360.)).min_w(px(340.)).child(card_head(p, "Debugging and speed")).child(rows(p, speed))))
        .child(ext_card)
        .child(groups)
        .into_any_element()
}

// ------------------------------------------------------------------ debug pages

/// Symfony's HTML dump, as readable text.
pub(crate) fn dump_text(html: &str) -> String {
    let mut s = html.to_string();
    for tag in ["script", "style"] {
        while let Some(a) = s.find(&format!("<{tag}")) {
            let close = format!("</{tag}>");
            match s[a..].find(&close) { Some(b) => s.replace_range(a..a + b + close.len(), ""), None => { s.truncate(a); break; } }
        }
    }
    let mut out = String::new();
    let mut in_tag = false;
    for ch in s.chars() {
        match ch { '<' => in_tag = true, '>' => in_tag = false, c if !in_tag => out.push(c), _ => {} }
    }
    out.replace("&quot;", "\"").replace("&#039;", "'").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ").replace("&amp;", "&")
        .trim().to_string()
}

pub(crate) fn dumps_page(p: &Pal, cx: &App) -> AnyElement {
    let t = tez(cx);
    let on = t.settings["dumps"].as_bool().unwrap_or(false);
    let filter = local(cx, "dumpsite");
    let mut sites: Vec<String> = t.dumps.iter().map(|d| txt(&d["site"])).collect();
    sites.sort();
    sites.dedup();
    let mut opts = vec![(String::new(), "All sites".to_string())];
    for s in &sites { opts.push((s.clone(), s.clone())); }
    let pick: Pick = Rc::new(|v, cx| set_local(cx, "dumpsite", v));
    let now = now_secs();
    let site_paths: Vec<(String, String)> = arr(&t.status["sites"]).iter().map(|s| (txt(&s["name"]), txt(&s["path"]))).collect();
    let items: Vec<AnyElement> = t.dumps.iter().rev().filter(|d| filter.is_empty() || txt(&d["site"]) == filter).take(40).map(|d| {
        let mut file = txt(&d["file"]);
        for (_, path) in &site_paths { if !path.is_empty() && file.starts_with(path.as_str()) { file = file[path.len()..].trim_start_matches(['/', '\\']).to_string(); } }
        let when = d["time"].as_f64().map(|tm| format!("{} ago", ago((now - tm).max(0.) as u64))).unwrap_or_default();
        let text = dump_text(&txt(&d["html"]));
        card(p)
            .child(div().flex().flex_row().flex_wrap().items_center().gap(px(12.)).px(px(18.)).py(px(12.)).border_b_1().border_color(p.line)
                .child(div().font_family(MONO).text_size(px(12.5)).text_color(p.text).child(format!("{file}:{}", d["line"].as_u64().unwrap_or(0))))
                .child(div().flex_1())
                .child(muted(p, format!("{} · {} · {when}", txt(&d["site"]), txt(&d["uri"])))))
            .child(div().p(px(14.)).child(code_block(p, &text, 60)))
            .into_any_element()
    }).collect();
    let mut out = div().flex().flex_col().gap(px(16.))
        .child(header(p, "Dumps", "Every dump() and dd() from every site, newest first. The page keeps rendering.", vec![
            select("dumpsite", p, cx, &filter, opts, 180., pick).into_any_element(),
            div().flex().flex_row().items_center().gap(px(8.)).h(px(34.)).child(muted(p, "Capture"))
                .child(switch("dumps-on", p, on).on_click(move |_, _, cx| send(cx, "/api/settings".into(), json!({ "dumps": !on }), if on { "Dumps off" } else { "Capturing dumps" }))).into_any_element(),
            button("dumps-clear", p, Kind::Secondary, false, Some("trash"), Some("Clear")).on_click(|_, _, cx| {
                let _ = api::delete("/api/dumps");
                let t = cx.global_mut::<Tez>(); t.dumps.clear(); t.dumps_seen = 0;
                notify(cx, "Cleared");
            }).into_any_element(),
        ]));
    if items.is_empty() {
        out = out.child(empty_state(p, "bug", "No dumps yet", if on { "Call dump() or dd() anywhere in a site and it shows here." } else { "Turn on Capture, then call dump() or dd() anywhere in a site." }, vec![]));
    } else {
        out = out.children(items);
    }
    out.into_any_element()
}

pub(crate) fn mail_page(p: &Pal, cx: &App) -> AnyElement {
    let t = tez(cx);
    let url = arr(&t.status["procs"]).iter().find(|x| x["name"] == "mailpit").map(|x| txt(&x["url"])).unwrap_or_default();
    let msgs = arr(&t.mail["messages"]);
    let u1 = url.clone();
    let pp = *p;
    let items: Vec<AnyElement> = msgs.iter().map(|m| {
        let id = txt(&m["ID"]);
        let from = { let n = txt(&m["From"]["Name"]); if n.is_empty() { txt(&m["From"]["Address"]) } else { n } };
        let to = arr(&m["To"]).iter().map(|x| txt(&x["Address"])).collect::<Vec<_>>().join(", ");
        let unread = m["Read"] == false;
        let when = txt(&m["Created"]).replace('T', " ").chars().take(16).collect::<String>();
        let link = format!("{url}/view/{id}");
        div().id(sid(format!("mail-{id}"))).flex().flex_row().items_start().gap(px(12.)).px(px(18.)).py(px(12.)).cursor_pointer().hover(move |s| s.bg(pp.surface2))
            .child(div().pt(px(6.)).child(dot(if unread { pp.accent_hi } else { gpui_kit::transparent_black() })))
            .child(div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(2.))
                .child(div().flex().flex_row().gap(px(8.)).child(div().font_weight(if unread { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM }).child(txt(&m["Subject"])))
                    .child(div().flex_1()).child(muted(&pp, when)))
                .child(muted(&pp, format!("{from} → {to}")))
                .child(div().text_size(px(12.5)).text_color(pp.text2).truncate().child(txt(&m["Snippet"]))))
            .on_click(move |_, _, _| api::open_url(&link))
            .into_any_element()
    }).collect();
    let mut out = div().flex().flex_col().gap(px(16.))
        .child(header(p, "Mail", "Every email your sites send lands here. Nothing leaves this computer.", if url.is_empty() { vec![] } else { vec![
            button("mail-open", p, Kind::Primary, false, Some("external-link"), Some("Open inbox")).on_click(move |_, _, _| api::open_url(&u1)).into_any_element(),
        ] }));
    if url.is_empty() {
        out = out.child(empty_state(p, "mail", "Mailpit is not running", "Turn it on under Services to catch mail from your sites.", vec![
            button("mail-svc", p, Kind::Secondary, false, None, Some("Services")).on_click(|_, _, cx| go(cx, Page::Services)).into_any_element(),
        ]));
    } else if items.is_empty() {
        out = out.child(empty_state(p, "mail", "No mail yet", "Sites with MAIL_HOST=127.0.0.1 and MAIL_PORT=1025 send here.", vec![]));
    } else {
        out = out.child(card(p).child(card_head(p, "Inbox").child(muted(p, format!("{} messages, {} unread", t.mail["total"].as_u64().unwrap_or(msgs.len() as u64), t.mail["unread"].as_u64().unwrap_or(0))))).child(rows(p, items)));
    }
    out.into_any_element()
}

pub(crate) fn logs_page(p: &Pal, cx: &App) -> AnyElement {
    let t = tez(cx);
    let procs = arr(&t.status["procs"]);
    let sel = if t.log_sel.is_empty() { procs.iter().find(|x| x["name"] == "web").map(|x| txt(&x["name"])).unwrap_or_default() } else { t.log_sel.clone() };
    let text = t.logs.get(&format!("proc:{sel}")).cloned().unwrap_or_default();
    let pp = *p;
    let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
    for x in &procs {
        let g = txt(&x["group"]);
        let label = if g == "web" { "Web server".to_string() } else if g == "php" { "PHP".into() } else if g == "service" { "Services".into() } else if let Some(s) = g.strip_prefix("site:") { s.to_string() } else { g.clone() };
        match groups.iter_mut().find(|(l, _)| *l == label) { Some((_, v)) => v.push(x.clone()), None => groups.push((label, vec![x.clone()])) }
    }
    let mut nav = card(p).flex_none().w(px(250.)).p(px(6.)).gap(px(2.));
    for (label, list) in groups {
        nav = nav.child(div().px(px(10.)).pt(px(12.)).pb(px(4.)).child(label_caps(p, label.to_uppercase())));
        for x in list {
            let name = txt(&x["name"]);
            let on = name == sel;
            let st = txt(&x["state"]);
            let n2 = name.clone();
            nav = nav.child(div().id(sid(format!("lg-{name}"))).flex().flex_row().items_center().gap(px(10.)).h(px(30.)).px(px(10.)).rounded(px(8.)).cursor_pointer()
                .when(on, |d| d.bg(pp.surface2)).hover(move |s| s.bg(pp.surface2))
                .child(dot(if st == "running" { pp.green } else if st == "failed" { pp.red } else { pp.text3 }))
                .child(div().flex_1().truncate().font_family(MONO).text_size(px(12.)).text_color(if on { pp.text } else { pp.text2 }).child(name.clone()))
                .on_click(move |_, _, cx| { cx.global_mut::<Tez>().log_sel = n2.clone(); refresh_now(cx, false); cx.refresh_windows(); }));
        }
    }
    let s2 = sel.clone();
    div().flex().flex_col().gap(px(20.))
        .child(header(p, "Logs", "One log per process tez runs. Each start is marked with a line.", vec![]))
        .child(div().flex().flex_row().items_start().gap(px(16.)).child(nav)
            .child(card(p).flex_1().min_w(px(0.))
                .child(card_head(p, &sel).child(div().flex_1())
                    .child(button("lg-restart", p, Kind::Secondary, true, None, Some("Restart")).on_click(move |_, _, cx| send(cx, format!("/api/proc/{s2}/restart"), json!({}), "Restarting")))
                    .child(copy_btn("lg-copy", p, text.clone())))
                .child(div().id("logview").max_h(px(640.)).overflow_y_scroll().p(px(14.)).child(code_block(p, &text, 200)))))
        .into_any_element()
}

// ------------------------------------------------------------------ settings

