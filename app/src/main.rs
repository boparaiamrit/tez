//! tez desktop app: a native GPUI Kit window over the tez daemon, plus a tray icon.
//!
//! Everything it shows and changes goes through the daemon's API on 127.0.0.1, so the app, the
//! web dashboard and the `tez` command line always agree. Pages follow one rule: the settings
//! people change most sit at the top of each page, the rest below in named groups.

#![windows_subsystem = "windows"]

mod api;
mod tray;

use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    label::Label,
    setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    ActiveTheme, Theme,
};
use gpui_kit::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ state

#[derive(Default)]
struct Tez {
    online: bool,
    error: String,
    status: Value,
    settings: Value,
    php: Value,
    services: Value,
    jobs: Value,
    identities: Value,
    sites: BTreeMap<String, Value>,
    github: Value,
    repos: Value,
    gh_account: String,
    gh_owner: String,
    gh_route: String,
    gh_loading: bool,
    /// What the user is typing, shown instead of the saved value until the save lands.
    drafts: BTreeMap<String, String>,
    /// Saves waiting for typing to pause: key -> (API path, body, when queued).
    pending: BTreeMap<String, (String, Value, Instant)>,
    message: Option<(String, Instant)>,
    local: BTreeMap<String, String>,
}
impl Global for Tez {}

struct TrayHolder(Option<tray::Tray>);
impl Global for TrayHolder {}

struct MainWindow(Option<AnyWindowHandle>);
impl Global for MainWindow {}

static SHOW_REQUESTED: AtomicBool = AtomicBool::new(false);
const INSTANCE_PORT: u16 = 7978;

fn tez(cx: &App) -> &Tez { cx.global::<Tez>() }
fn ss(v: &Value) -> SharedString { SharedString::from(v.as_str().unwrap_or_default().to_string()) }
fn txt(v: &Value) -> String {
    match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() }
}
fn mb(b: u64) -> String { if b >= 1 << 30 { format!("{:.1} GB", b as f64 / (1u64 << 30) as f64) } else { format!("{} MB", b >> 20) } }

fn notify(cx: &mut App, msg: impl Into<String>) {
    cx.global_mut::<Tez>().message = Some((msg.into(), Instant::now()));
    cx.refresh_windows();
}

/// Send a change now (switches, dropdowns, buttons).
fn send(cx: &mut App, path: String, body: Value, ok: &'static str) {
    cx.spawn(async move |cx| {
        let r = cx.background_executor().spawn(async move { api::post(&path, &body) }).await;
        let _ = cx.update(|cx| {
            let msg = match &r {
                Ok(v) if v["job"].is_number() => format!("{ok} (running in the background)"),
                Ok(v) if v["warnings"].as_str().map(|s| !s.is_empty()).unwrap_or(false) => v["warnings"].as_str().unwrap_or_default().to_string(),
                Ok(_) => ok.to_string(),
                Err(e) => e.clone(),
            };
            notify(cx, msg);
            refresh_now(cx, true);
        });
    })
    .detach();
}

/// Queue a typed change; it is sent once typing pauses.
fn queue(cx: &mut App, key: String, text: String, path: String, body: Value) {
    let t = cx.global_mut::<Tez>();
    t.drafts.insert(key.clone(), text);
    t.pending.insert(key, (path, body, Instant::now()));
}

fn draft(cx: &App, key: &str, saved: String) -> SharedString {
    SharedString::from(tez(cx).drafts.get(key).cloned().unwrap_or(saved))
}

// ------------------------------------------------------------------ fetching

#[derive(Default)]
struct Fetched {
    status: Option<Result<Value, String>>,
    jobs: Option<Value>,
    settings: Option<Value>,
    php: Option<Value>,
    services: Option<Value>,
    identities: Option<Value>,
    sites: Vec<(String, Value)>,
}

fn fetch(full: bool, site_names: Vec<String>) -> Fetched {
    let mut f = Fetched { status: Some(api::get("/api/status")), ..Default::default() };
    if !matches!(f.status, Some(Ok(_))) { return f; }
    f.jobs = api::get("/api/jobs").ok();
    if full {
        f.settings = api::get("/api/settings").ok();
        f.php = api::get("/api/php").ok();
        f.services = api::get("/api/services").ok();
        f.identities = api::get("/api/github/identities").ok();
        for n in site_names { if let Ok(v) = api::get(&format!("/api/site/{n}")) { f.sites.push((n, v)); } }
    }
    f
}

fn apply(cx: &mut App, f: Fetched) {
    let t = cx.global_mut::<Tez>();
    match f.status {
        Some(Ok(v)) => { t.online = true; t.status = v; t.error.clear(); }
        Some(Err(e)) => { t.online = false; t.error = e; }
        None => {}
    }
    if let Some(v) = f.jobs { t.jobs = v; }
    if let Some(v) = f.settings { t.settings = v; }
    if let Some(v) = f.php { t.php = v; }
    if let Some(v) = f.services { t.services = v; }
    if let Some(v) = f.identities {
        if t.gh_account.is_empty() {
            if let Some(a) = v["accounts"].as_array().and_then(|a| a.iter().find(|x| x["active"].as_bool() == Some(true)).or(a.first())) {
                t.gh_account = txt(&a["login"]);
            }
        }
        if t.gh_route.is_empty() {
            let acct = t.gh_account.clone();
            if let Some(r) = v["routes"].as_array().and_then(|r| r.iter().find(|x| txt(&x["login"]) == acct)) { t.gh_route = txt(&r["route"]); }
        }
        t.identities = v;
    }
    for (n, v) in f.sites { t.sites.insert(n, v); }
    // Drop drafts the server has caught up with (no save pending for them).
    let pending: Vec<String> = t.pending.keys().cloned().collect();
    t.drafts.retain(|k, _| pending.contains(k));
    // Tray: tooltip and the Sites menu.
    let running = t.status["procs"].as_array().map(|p| p.iter().filter(|x| x["state"] == "running").count()).unwrap_or(0);
    let memory = t.status["memory"].as_u64().unwrap_or(0);
    let tip = if t.online { format!("tez · {running} running · {}", mb(memory)) } else { "tez · not running".to_string() };
    let sites: Vec<(String, String)> = t.status["sites"].as_array().map(|a| a.iter().map(|s| (txt(&s["name"]), txt(&s["url"]))).collect()).unwrap_or_default();
    if let Some(tray) = cx.global_mut::<TrayHolder>().0.as_mut() {
        tray.set_tooltip(&tip);
        tray.set_sites(&sites);
    }
    cx.refresh_windows();
}

fn site_names(cx: &App) -> Vec<String> {
    tez(cx).status["sites"].as_array().map(|a| a.iter().map(|s| txt(&s["name"])).collect()).unwrap_or_default()
}

fn refresh_now(cx: &mut App, full: bool) {
    let names = site_names(cx);
    cx.spawn(async move |cx| {
        let f = cx.background_executor().spawn(async move { fetch(full, names) }).await;
        let _ = cx.update(|cx| apply(cx, f));
    })
    .detach();
}

fn load_repos(cx: &mut App) {
    let account = tez(cx).gh_account.clone();
    let owner = tez(cx).gh_owner.clone();
    cx.global_mut::<Tez>().gh_loading = true;
    cx.spawn(async move |cx| {
        let res = cx.background_executor().spawn(async move {
            let status = api::get(&format!("/api/github?account={account}")).unwrap_or(Value::Null);
            let owner = if owner.is_empty() { status["owners"].as_array().and_then(|o| o.first()).map(txt).unwrap_or_default() } else { owner };
            let repos = if owner.is_empty() { Value::Null } else { api::get(&format!("/api/github/repos/{owner}?account={account}")).unwrap_or(Value::Null) };
            (status, owner, repos)
        }).await;
        let _ = cx.update(|cx| {
            let t = cx.global_mut::<Tez>();
            t.github = res.0;
            t.gh_owner = res.1;
            t.repos = res.2;
            t.gh_loading = false;
            cx.refresh_windows();
        });
    })
    .detach();
}

/// One loop drives everything: tray clicks, the second-launch signal, typed-change saves, refresh.
fn start_loop(cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut tick: u64 = 0;
        loop {
            let _ = cx.update(|cx| {
                if SHOW_REQUESTED.swap(false, Ordering::SeqCst) { show_window(cx); }
                let actions = cx.global::<TrayHolder>().0.as_ref().map(|t| t.poll()).unwrap_or_default();
                for a in actions { tray_action(cx, a); }
                // Flush saves whose typing has paused.
                let due: Vec<(String, String, Value)> = tez(cx).pending.iter()
                    .filter(|(_, (_, _, at))| at.elapsed() > Duration::from_millis(900))
                    .map(|(k, (p, b, _))| (k.clone(), p.clone(), b.clone())).collect();
                for (k, path, body) in due {
                    cx.global_mut::<Tez>().pending.remove(&k);
                    send(cx, path, body, "Saved");
                }
                if let Some((_, at)) = &tez(cx).message { if at.elapsed() > Duration::from_secs(4) { cx.global_mut::<Tez>().message = None; cx.refresh_windows(); } }
                if tick % 8 == 0 { refresh_now(cx, tick % 40 == 0); }
            });
            cx.background_executor().timer(Duration::from_millis(250)).await;
            tick += 1;
        }
    })
    .detach();
}

fn tray_action(cx: &mut App, a: tray::TrayAction) {
    use tray::TrayAction::*;
    match a {
        Open => show_window(cx),
        Dashboard => api::open_url(&format!("http://127.0.0.1:{}", api::port())),
        Site(url) => api::open_url(&url),
        Start => { api::start_daemon(); notify(cx, "Starting tez"); }
        Restart => send(cx, "/api/proc/php/restart".into(), json!({}), "Restarting PHP"),
        Stop => send(cx, "/api/shutdown".into(), json!({}), "Stopping tez"),
        Quit => cx.quit(),
    }
}

fn show_window(cx: &mut App) {
    if let Some(h) = cx.global::<MainWindow>().0 {
        if h.update(cx, |_, window, _| window.activate_window()).is_ok() { return; }
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1200.), px(800.)), cx))),
        titlebar: Some(TitlebarOptions { title: Some("tez".into()), ..Default::default() }),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        Theme::sync_system_appearance(Some(window), cx);
        cx.new(|_| TezView)
    }) {
        Ok((h, _)) => cx.global_mut::<MainWindow>().0 = Some(h),
        Err(e) => eprintln!("tez: {e}"),
    }
}

// ------------------------------------------------------------------ building blocks

fn button(id: impl Into<SharedString>, label: &str, on: impl Fn(&mut App) + 'static) -> Button {
    let label = label.to_string();
    Button::new(ElementId::Name(id.into())).label(label).on_click(move |_, _, cx| on(cx))
}

fn row() -> Div { div().flex().flex_row().items_center().gap_2() }

/// A masked field with show/hide, saved with its own button (secrets are never auto-saved).
fn secret(id: String, value: String, save: Rc<dyn Fn(String, &mut App)>) -> SettingField<SharedString> {
    SettingField::render(move |_, window, cx| {
        let v = value.clone();
        let state = window.use_keyed_state(ElementId::Name(SharedString::from(id.clone())), cx, move |window, cx| {
            InputState::new(window, cx).masked(true).default_value(v)
        });
        let st = state.clone();
        let save = save.clone();
        row().w(px(380.))
            .child(div().flex_1().child(Input::new(&state).mask_toggle()))
            .child(Button::new(ElementId::Name(SharedString::from(format!("{id}-save")))).label("Save").on_click(move |_, _, cx| {
                let v = st.read(cx).value().to_string();
                save(v, cx);
            }))
    })
}

fn about(text: &str) -> SettingItem {
    let t = text.to_string();
    SettingItem::render(move |_, _, cx| div().text_sm().text_color(cx.theme().muted_foreground).child(t.clone()))
}

// ------------------------------------------------------------------ pages

fn overview_page(cx: &App) -> SettingPage {
    let t = tez(cx);
    let running = t.status["procs"].as_array().map(|p| p.iter().filter(|x| x["state"] == "running").count()).unwrap_or(0);
    let restarting = t.status["procs"].as_array().map(|p| p.iter().filter(|x| x["state"] == "backoff" || x["state"] == "failed").count()).unwrap_or(0);
    let memory = t.status["memory"].as_u64().unwrap_or(0);
    let notes: Vec<String> = t.status["notes"].as_array().map(|a| a.iter().map(txt).collect()).unwrap_or_default();
    let summary = format!("{running} processes running{} · {} of memory", if restarting > 0 { format!(", {restarting} restarting") } else { String::new() }, mb(memory));

    let mut status = SettingGroup::new().title("Status")
        .item(SettingItem::new("tez", SettingField::render(move |_, _, _| row()
            .child(button("ov-dash", "Web dashboard", |_| api::open_url(&format!("http://127.0.0.1:{}", api::port()))))
            .child(button("ov-restart-php", "Restart PHP", |cx| send(cx, "/api/proc/php/restart".into(), json!({}), "PHP restarted")))
            .child(button("ov-reload", "Reload sites", |cx| send(cx, "/api/reload".into(), json!({}), "Reloaded")))))
            .description(summary));
    for n in notes { status = status.item(about(&format!("⚠ {n}"))); }

    let mut sites = SettingGroup::new().title("Sites").description("Start or stop each project's background processes (Horizon, Reverb, Vite)");
    for s in t.status["sites"].as_array().cloned().unwrap_or_default() {
        let name = txt(&s["name"]);
        let url = txt(&s["url"]);
        let procs: Vec<String> = s["processes"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
        let n1 = name.clone();
        let n2 = name.clone();
        let u = url.clone();
        let desc = format!("{url} · PHP {} · {}", txt(&s["php"]), if procs.is_empty() { "no background processes".to_string() } else { procs.join(", ") });
        let item = if procs.is_empty() {
            SettingItem::new(name.clone(), SettingField::render(move |_, _, _| button(format!("open-{}", n1), "Open", { let u = u.clone(); move |_| api::open_url(&u) })))
        } else {
            SettingItem::new(name.clone(), SettingField::switch(
                move |cx| tez(cx).status["sites"].as_array().and_then(|a| a.iter().find(|x| txt(&x["name"]) == n1)).and_then(|x| x["up"].as_bool()).unwrap_or(false),
                move |v, cx| send(cx, format!("/api/site/{n2}/{}", if v { "up" } else { "down" }), json!({}), if v { "Starting processes" } else { "Stopping processes" }),
            ))
        };
        sites = sites.item(item.description(desc));
    }

    let mut services = SettingGroup::new().title("Services");
    for p in t.status["procs"].as_array().cloned().unwrap_or_default().into_iter().filter(|p| p["group"] == "service" || p["group"] == "web") {
        let name = txt(&p["name"]);
        let state = txt(&p["state"]);
        let n = name.clone();
        let mem = p["memory"].as_u64().unwrap_or(0);
        let port = p["port"].as_u64().map(|x| format!(" · port {x}")).unwrap_or_default();
        services = services.item(SettingItem::new(name.clone(), SettingField::render(move |_, _, _| row()
            .child(button(format!("rs-{n}"), "Restart", { let n = n.clone(); move |cx| send(cx, format!("/api/proc/{n}/restart"), json!({}), "Restarting") }))))
            .description(format!("{state}{port} · {}", mb(mem))));
    }

    let mut jobs = SettingGroup::new().title("Recent jobs");
    for j in t.jobs["jobs"].as_array().cloned().unwrap_or_default().into_iter().take(6) {
        jobs = jobs.item(about(&format!("{} — {}", txt(&j["title"]), txt(&j["state"]))));
    }

    SettingPage::new("Overview").default_open(true).resettable(false).groups([status, sites, services, jobs])
}

const PHP_GROUPS: &[(&str, &[&str])] = &[
    ("Limits", &["max_input_time", "max_input_vars", "max_file_uploads", "default_socket_timeout", "session.gc_maxlifetime"]),
    ("Errors", &["error_reporting", "display_startup_errors", "log_errors"]),
    ("Performance (OPcache and paths)", &["opcache.enable", "opcache.enable_cli", "opcache.memory_consumption", "opcache.interned_strings_buffer", "opcache.max_accelerated_files", "opcache.validate_timestamps", "opcache.revalidate_freq", "opcache.jit", "opcache.jit_buffer_size", "realpath_cache_size", "realpath_cache_ttl"]),
    ("Xdebug", &["xdebug.mode", "xdebug.start_with_request", "xdebug.client_port"]),
    ("Other", &["date.timezone", "output_buffering", "short_open_tag", "allow_url_fopen"]),
];

fn ini_item(minor: &str, key: &str, catalog: &BTreeMap<String, (String, String)>) -> SettingItem {
    let (default, about) = catalog.get(key).cloned().unwrap_or_default();
    let (m1, m2, k1, k2) = (minor.to_string(), minor.to_string(), key.to_string(), key.to_string());
    let dk = format!("php:{minor}:{key}");
    let dk2 = dk.clone();
    SettingItem::new(key.to_string(), SettingField::input(
        move |cx| {
            let saved = tez(cx).php["versions"].as_array().and_then(|v| v.iter().find(|x| txt(&x["minor"]) == m1)).map(|v| txt(&v["values"][&k1])).unwrap_or_default();
            draft(cx, &dk, saved)
        },
        move |v, cx| {
            let val = if v.trim().is_empty() { Value::Null } else { Value::String(v.to_string()) };
            queue(cx, dk2.clone(), v.to_string(), format!("/api/php/{m2}"), json!({ "settings": { k2.clone(): val } }));
        },
    ))
    .description(format!("{about} · default {}", if default.is_empty() { "empty".to_string() } else { default }))
}

fn php_pages(cx: &App) -> Vec<SettingPage> {
    let t = tez(cx);
    let catalog: BTreeMap<String, (String, String)> = t.php["catalog"].as_array().map(|a| a.iter().map(|c| (txt(&c["key"]), (txt(&c["default"]), txt(&c["about"])))).collect()).unwrap_or_default();
    let mut pages = Vec::new();
    for v in t.php["versions"].as_array().cloned().unwrap_or_default() {
        let minor = txt(&v["minor"]);
        let used: Vec<String> = v["sites"].as_array().map(|a| a.iter().map(txt).collect()).unwrap_or_default();
        let mut groups = vec![SettingGroup::new().title("Essentials").items(
            ["memory_limit", "max_execution_time", "upload_max_filesize", "post_max_size"].iter().map(|k| ini_item(&minor, k, &catalog)).chain(std::iter::once({
                let (m1, m2) = (minor.clone(), minor.clone());
                SettingItem::new("display_errors", SettingField::dropdown(
                    vec![("On".into(), "On".into()), ("Off".into(), "Off".into())],
                    move |cx| ss(&tez(cx).php["versions"].as_array().and_then(|v| v.iter().find(|x| txt(&x["minor"]) == m1)).map(|v| v["values"]["display_errors"].clone()).unwrap_or(Value::Null)),
                    move |val, cx| send(cx, format!("/api/php/{m2}"), json!({ "settings": { "display_errors": val.to_string() } }), "Saved; PHP restarted"),
                )).description("Show errors in the page")
            })).collect::<Vec<_>>(),
        )];
        for (title, keys) in PHP_GROUPS {
            groups.push(SettingGroup::new().title(*title).items(keys.iter().map(|k| ini_item(&minor, k, &catalog)).collect::<Vec<_>>()));
        }
        let exts: Vec<Value> = v["extensions"].as_array().cloned().unwrap_or_default();
        for (title, default) in [("Extensions loaded by default", true), ("Extensions available (off)", false)] {
            let mut g = SettingGroup::new().title(title);
            for e in exts.iter().filter(|e| e["default"].as_bool() == Some(default)) {
                let name = txt(&e["name"]);
                let (m1, m2, n1, n2) = (minor.clone(), minor.clone(), name.clone(), name.clone());
                g = g.item(SettingItem::new(name.clone(), SettingField::switch(
                    move |cx| tez(cx).php["versions"].as_array().and_then(|v| v.iter().find(|x| txt(&x["minor"]) == m1))
                        .and_then(|v| v["extensions"].as_array().and_then(|a| a.iter().find(|x| txt(&x["name"]) == n1)).and_then(|x| x["on"].as_bool())).unwrap_or(false),
                    move |on, cx| send(cx, format!("/api/php/{m2}"), json!({ "extensions": { n2.clone(): on } }), "Saved; PHP restarted"),
                )));
            }
            groups.push(g);
        }
        {
            let m = minor.clone();
            let key = format!("pecl-{minor}");
            let k2 = key.clone();
            groups.push(SettingGroup::new().title("Install from PECL").description("Any extension with a Windows build: imagick, igbinary, apcu, pcov, swoole, yaml, ds…").items([
                SettingItem::new("Extension name", SettingField::input(move |cx| SharedString::from(tez(cx).local.get(&key).cloned().unwrap_or_default()), move |v, cx| { cx.global_mut::<Tez>().local.insert(k2.clone(), v.to_string()); })),
                SettingItem::new("", SettingField::render(move |_, _, _| row()
                    .child(button(format!("pecl-go-{m}"), &format!("Install for PHP {m}"), { let m = m.clone(); move |cx| { let n = tez(cx).local.get(&format!("pecl-{m}")).cloned().unwrap_or_default(); if !n.is_empty() { send(cx, format!("/api/php/{m}"), json!({ "install": n }), "Installing") } } }))
                    .child(button(format!("pecl-all-{m}"), "All versions", { let m = m.clone(); move |cx| { let n = tez(cx).local.get(&format!("pecl-{m}")).cloned().unwrap_or_default(); if !n.is_empty() { send(cx, "/api/php/all".into(), json!({ "install": n }), "Installing") } } })))),
            ]));
        }
        pages.push(SettingPage::new(format!("PHP {minor}"))
            .description(format!("{}{} · used by {}", txt(&v["version"]), if v["default"].as_bool() == Some(true) { " · default" } else { "" }, if used.is_empty() { "no sites".to_string() } else { used.join(", ") }))
            .resettable(false)
            .groups(groups));
    }
    pages
}

fn site_pages(cx: &App) -> Vec<SettingPage> {
    let t = tez(cx);
    let installed: Vec<String> = t.php["versions"].as_array().map(|a| a.iter().map(|v| txt(&v["minor"])).collect()).unwrap_or_default();
    let mut pages = Vec::new();
    for s in t.status["sites"].as_array().cloned().unwrap_or_default() {
        let name = txt(&s["name"]);
        let d = t.sites.get(&name).cloned().unwrap_or(Value::Null);
        let path = txt(&d["path"]);
        let url = txt(&s["url"]);
        let mut php_opts: Vec<(SharedString, SharedString)> = vec![("".into(), "Automatic (composer.json / default)".into())];
        for m in ["7.4", "8.0", "8.1", "8.2", "8.3", "8.4", "8.5", "8.6"] {
            let label = if installed.iter().any(|x| x == m) { format!("PHP {m}") } else { format!("PHP {m} (installs it)") };
            php_opts.push((m.into(), label.into()));
        }
        let (n1, n2, n3, n4) = (name.clone(), name.clone(), name.clone(), name.clone());
        let mut ess = SettingGroup::new().title("Essentials")
            .item(SettingItem::new("PHP version", SettingField::dropdown(php_opts,
                move |cx| { let d = tez(cx).sites.get(&n1); match d { Some(d) if d["isolated"].as_bool() == Some(true) => ss(&d["php"]), _ => "".into() } },
                move |v, cx| send(cx, format!("/api/site/{n2}"), json!({ "php": if v.is_empty() { Value::Null } else { Value::String(v.to_string()) } }), "Saved"),
            )).description(format!("Now PHP {}", txt(&s["php"]))))
            .item(SettingItem::new("Background processes", SettingField::switch(
                move |cx| tez(cx).sites.get(&n3).and_then(|d| d["up"].as_bool()).unwrap_or(false),
                move |v, cx| send(cx, format!("/api/site/{n4}"), json!({ "up": v }), if v { "Starting processes" } else { "Stopping processes" }),
            )).description("What Sail ran as extra containers: Horizon, Reverb, Vite, the scheduler"));
        for p in d["processes"].as_array().cloned().unwrap_or_default() {
            let pn = txt(&p["name"]);
            let (a, b, pa, pb) = (name.clone(), name.clone(), pn.clone(), pn.clone());
            let desc = format!("{}{}", txt(&p["about"]), p["command"].as_str().map(|c| format!(" · {c}")).unwrap_or_default());
            ess = ess.item(SettingItem::new(pn.clone(), SettingField::switch(
                move |cx| tez(cx).sites.get(&a).and_then(|d| d["processes"].as_array().and_then(|x| x.iter().find(|y| txt(&y["name"]) == pa)).and_then(|y| y["on"].as_bool())).unwrap_or(false),
                move |v, cx| send(cx, format!("/api/site/{b}"), json!({ "processes": { pb.clone(): v } }), "Saved"),
            )).description(desc));
        }
        let (u1, p1, n5) = (url.clone(), path.clone(), name.clone());
        ess = ess.item(SettingItem::new("Open", SettingField::render(move |_, _, _| row()
            .child(button(format!("os-{n5}"), "Site", { let u = u1.clone(); move |_| api::open_url(&u) }))
            .child(button(format!("oh-{n5}"), "Horizon", { let u = u1.clone(); move |_| api::open_url(&format!("{u}/horizon")) }))
            .child(button(format!("of-{n5}"), "Folder", { let p = p1.clone(); move |_| api::open_url(&p) }))
            .child(button(format!("oc-{n5}"), "Code", { let p = p1.clone(); move |_| { use std::os::windows::process::CommandExt; let _ = std::process::Command::new("cmd.exe").args(["/c", "code", &p]).creation_flags(0x0800_0000).spawn(); } })))));

        let dk = format!("domains:{name}");
        let (dk2, n6, n7) = (dk.clone(), name.clone(), name.clone());
        let domains = SettingGroup::new().title("Domains").description("Extra host names for apps that route by domain; also answers on name.test and name.localhost")
            .item(SettingItem::new("Extra domains", SettingField::input(
                move |cx| draft(cx, &dk, tez(cx).sites.get(&n6).and_then(|d| d["domains"].as_array().map(|a| a.iter().map(txt).collect::<Vec<_>>().join(", "))).unwrap_or_default()),
                move |v, cx| { let list: Vec<String> = v.split([',', ' ']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(); queue(cx, dk2.clone(), v.to_string(), format!("/api/site/{n7}"), json!({ "domains": list })); },
            )).description("Comma separated, e.g. admin.myapp.test, hub.localhost"));

        let mut groups = vec![ess, domains];
        for g in d["env"].as_array().cloned().unwrap_or_default() {
            let mut group = SettingGroup::new().title(format!("{} (.env)", txt(&g["title"])));
            for k in g["keys"].as_array().cloned().unwrap_or_default() {
                let key = txt(&k["key"]);
                let value = txt(&k["value"]);
                let site = name.clone();
                if k["secret"].as_bool() == Some(true) {
                    let (s2, k2) = (site.clone(), key.clone());
                    let save: Rc<dyn Fn(String, &mut App)> = Rc::new(move |v, cx| send(cx, format!("/api/site/{s2}/env"), json!({ "set": { k2.clone(): v } }), "Saved .env; restart processes to apply"));
                    group = group.item(SettingItem::new(key.clone(), secret(format!("env-{site}-{key}"), value, save))
                        .description("Secret: kept only in this project's .env; sent nowhere but 127.0.0.1, never logged"));
                } else {
                    let dk = format!("env:{site}:{key}");
                    let (dk2, s1, s2, k1, k2) = (dk.clone(), site.clone(), site.clone(), key.clone(), key.clone());
                    group = group.item(SettingItem::new(key.clone(), SettingField::input(
                        move |cx| draft(cx, &dk, tez(cx).sites.get(&s1).and_then(|d| d["env"].as_array().and_then(|gs| gs.iter().flat_map(|g| g["keys"].as_array().cloned().unwrap_or_default()).find(|x| txt(&x["key"]) == k1)).map(|x| txt(&x["value"]))).unwrap_or_default()),
                        move |v, cx| queue(cx, dk2.clone(), v.to_string(), format!("/api/site/{s2}/env"), json!({ "set": { k2.clone(): v.to_string() } })),
                    )));
                }
            }
            groups.push(group);
        }
        let files: Vec<String> = d["config_files"].as_array().map(|a| a.iter().map(txt).collect()).unwrap_or_default();
        let mut cfg_group = SettingGroup::new().title("Config files").description("Horizon supervisors and workers, Reverb servers, queues: opens in your editor; or edit them in the web dashboard");
        for f in files.iter().filter(|f| ["horizon.php", "reverb.php", "queue.php", "broadcasting.php", "database.php", "cache.php", "session.php", "mail.php", "filesystems.php", "app.php"].contains(&f.as_str())) {
            let full = format!("{path}\\config\\{f}");
            let id = format!("cf-{name}-{f}");
            cfg_group = cfg_group.item(SettingItem::new(format!("config/{f}"), SettingField::render(move |_, _, _| button(id.clone(), "Open", { let full = full.clone(); move |_| api::open_url(&full) }))));
        }
        groups.push(cfg_group);
        pages.push(SettingPage::new(name.clone()).description(format!("{url} · {path}")).resettable(false).groups(groups));
    }
    pages
}

fn service_pages(cx: &App) -> Vec<SettingPage> {
    let t = tez(cx);
    let mut pages = Vec::new();
    let mut add = SettingGroup::new().title("Not installed yet");
    for sv in t.services["services"].as_array().cloned().unwrap_or_default() {
        let key = txt(&sv["name"]);
        let kind = txt(&sv["kind"]);
        let title = txt(&sv["title"]);
        if sv["installed"].as_bool() != Some(true) {
            let versions: Vec<String> = sv["versions"].as_array().map(|a| a.iter().map(txt).collect()).unwrap_or_default();
            let (k, def) = (kind.clone(), txt(&sv["version"]));
            add = add.item(SettingItem::new(title.clone(), SettingField::render(move |_, _, _| {
                let mut r = row();
                let list = if versions.is_empty() { vec![def.clone()] } else { versions.clone() };
                for v in list { let k2 = k.clone(); let v2 = v.clone(); r = r.child(button(format!("inst-{k}-{v}"), &format!("Install {v}"), move |cx| send(cx, format!("/api/service/{k2}"), json!({ "install": v2.clone() }), "Installing"))); }
                r
            })).description(txt(&sv["about"])));
            continue;
        }
        let (k1, k2, k3, k4) = (key.clone(), key.clone(), key.clone(), key.clone());
        let pk = format!("port:{key}");
        let pk2 = pk.clone();
        let mut ess = SettingGroup::new().title("Essentials")
            .item(SettingItem::new("Running", SettingField::switch(
                move |cx| tez(cx).services["services"].as_array().and_then(|a| a.iter().find(|x| txt(&x["name"]) == k1)).and_then(|x| x["enabled"].as_bool()).unwrap_or(false),
                move |v, cx| send(cx, format!("/api/service/{k2}"), json!({ "enabled": v }), if v { "Starting" } else { "Stopped; data kept" }),
            )).description("Off stops the service and keeps its data"))
            .item(SettingItem::new("Port", SettingField::number_input(NumberFieldOptions { min: 1025.0, max: 65535.0, step: 1.0 },
                move |cx| tez(cx).drafts.get(&pk).and_then(|d| d.parse().ok()).unwrap_or_else(|| tez(cx).services["services"].as_array().and_then(|a| a.iter().find(|x| txt(&x["name"]) == k3)).and_then(|x| x["port"].as_f64()).unwrap_or(0.0)),
                move |v, cx| queue(cx, pk2.clone(), (v as u16).to_string(), format!("/api/service/{k4}"), json!({ "port": v as u16 })),
            )).description("Listens on 127.0.0.1 only"))
            .item(about(&format!("Version {}", txt(&sv["full_version"]))));
        let versions: Vec<String> = sv["versions"].as_array().map(|a| a.iter().map(txt).collect()).unwrap_or_default();
        if !versions.is_empty() {
            let k = kind.clone();
            ess = ess.item(SettingItem::new("Add another version", SettingField::render(move |_, _, _| {
                let mut r = row();
                for v in versions.clone() { let k2 = k.clone(); let v2 = v.clone(); r = r.child(button(format!("addv-{k}-{v}"), &v, move |cx| send(cx, format!("/api/service/{k2}"), json!({ "install": v2.clone() }), "Installing"))); }
                r
            })).description("Each version runs beside the others on its own port"));
        }
        let mut groups = vec![ess];
        let opts: Vec<Value> = sv["options"].as_array().cloned().unwrap_or_default();
        if !opts.is_empty() {
            let mut g = SettingGroup::new().title("Tuning");
            for o in opts {
                let ok = txt(&o["key"]);
                let (s1, s2, o1, o2) = (key.clone(), key.clone(), ok.clone(), ok.clone());
                if o["secret"].as_bool() == Some(true) {
                    let save: Rc<dyn Fn(String, &mut App)> = Rc::new(move |v, cx| send(cx, format!("/api/service/{s2}"), json!({ "options": { o2.clone(): v } }), "Saved; restarting"));
                    g = g.item(SettingItem::new(ok.clone(), secret(format!("svc-{key}-{ok}"), txt(&o["value"]), save)).description(format!("{} · secret: stored in tez.json on this computer", txt(&o["about"]))));
                } else {
                    let dk = format!("svc:{key}:{ok}");
                    let dk2 = dk.clone();
                    g = g.item(SettingItem::new(ok.clone(), SettingField::input(
                        move |cx| draft(cx, &dk, tez(cx).services["services"].as_array().and_then(|a| a.iter().find(|x| txt(&x["name"]) == s1)).and_then(|x| x["options"].as_array().and_then(|os| os.iter().find(|y| txt(&y["key"]) == o1)).map(|y| txt(&y["value"]))).unwrap_or_default()),
                        move |v, cx| queue(cx, dk2.clone(), v.to_string(), format!("/api/service/{s2}"), json!({ "options": { o2.clone(): v.to_string() } })),
                    )).description(format!("{} · default {}", txt(&o["about"]), txt(&o["default"]))));
                }
            }
            groups.push(g);
        }
        if kind == "postgres" {
            let mut g = SettingGroup::new().title("Extensions").description("Every build also has pgcrypto, hstore, citext, pg_trgm, uuid-ossp and the rest of contrib");
            let have: Vec<(String, String)> = sv["extensions"].as_array().map(|a| a.iter().map(|x| (txt(&x[0]), txt(&x[1]))).collect()).unwrap_or_default();
            for e in sv["extension_catalog"].as_array().cloned().unwrap_or_default() {
                let en = txt(&e[0]);
                let installed = have.iter().find(|(n, _)| *n == en).map(|(_, v)| v.clone());
                let (k, e2) = (key.clone(), en.clone());
                g = g.item(SettingItem::new(en.clone(), SettingField::render(move |_, _, _| button(format!("pgx-{k}-{e2}"), if installed.is_some() && e2 != "timescaledb" { "Reinstall" } else { "Add" }, { let k = k.clone(); let e = e2.clone(); move |cx| send(cx, format!("/api/service/{k}"), json!({ "extension": e.clone() }), "Adding extension") })))
                    .description(format!("{}{}", txt(&e[1]), have.iter().find(|(n, _)| *n == en).map(|(_, v)| format!(" · installed {v}")).unwrap_or_default())));
            }
            groups.push(g);
        }
        let env: String = sv["env"].as_array().map(|a| a.iter().map(|p| format!("{}={}", txt(&p[0]), txt(&p[1]))).collect::<Vec<_>>().join("\n")).unwrap_or_default();
        let env2 = env.clone();
        groups.push(SettingGroup::new().title("For your .env").item(SettingItem::new("Connection settings", SettingField::render(move |_, _, _| {
            let e = env2.clone();
            button(format!("copy-{}", e.len()), "Copy", move |cx| { cx.write_to_clipboard(ClipboardItem::new_string(e.clone())); notify(cx, "Copied"); })
        })).description(env)));
        pages.push(SettingPage::new(title).description(txt(&sv["about"])).resettable(false).groups(groups));
    }
    pages.push(SettingPage::new("Add a service").resettable(false).group(add));
    pages
}

fn settings_page(cx: &App) -> SettingPage {
    let t = tez(cx);
    let installed: Vec<(SharedString, SharedString)> = t.settings["php_installed"].as_array().map(|a| a.iter().map(|m| (ss(m), SharedString::from(format!("PHP {}", txt(m))))).collect()).unwrap_or_default();
    let switch = |label: &'static str, key: &'static str, desc: &'static str| {
        SettingItem::new(label, SettingField::switch(
            move |cx| tez(cx).settings[key].as_bool().unwrap_or(false),
            move |v, cx| { cx.global_mut::<Tez>().settings[key] = Value::Bool(v); send(cx, "/api/settings".into(), json!({ key: v }), "Saved"); },
        )).description(desc)
    };
    let text = |label: &'static str, key: &'static str, desc: &'static str| {
        let dk = format!("set:{key}");
        let dk2 = dk.clone();
        SettingItem::new(label, SettingField::input(
            move |cx| draft(cx, &dk, if key == "parked" { tez(cx).settings["parked"].as_array().map(|a| a.iter().map(txt).collect::<Vec<_>>().join("; ")).unwrap_or_default() } else { txt(&tez(cx).settings[key]) }),
            move |v, cx| {
                let body = if key == "parked" { json!({ "parked": v.split(';').map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>() }) } else { json!({ key: v.to_string() }) };
                queue(cx, dk2.clone(), v.to_string(), "/api/settings".into(), body);
            },
        )).description(desc)
    };
    let number = |label: &'static str, key: &'static str, min: f64, max: f64, desc: &'static str| {
        let dk = format!("set:{key}");
        let dk2 = dk.clone();
        SettingItem::new(label, SettingField::number_input(NumberFieldOptions { min, max, step: 1.0 },
            move |cx| tez(cx).drafts.get(&dk).and_then(|d| d.parse().ok()).unwrap_or_else(|| tez(cx).settings[key].as_f64().unwrap_or(0.0)),
            move |v, cx| queue(cx, dk2.clone(), (v as u64).to_string(), "/api/settings".into(), json!({ key: v as u64 })),
        )).description(desc)
    };
    let free = t.settings["free_disk"].as_u64().unwrap_or(0);
    let trusted = t.settings["ca_trusted"].as_bool().unwrap_or(false);
    SettingPage::new("Settings").resettable(false).groups([
        SettingGroup::new().title("Essentials").items([
            SettingItem::new("Default PHP", SettingField::dropdown(installed,
                |cx| ss(&tez(cx).settings["default_php"]),
                |v, cx| send(cx, "/api/settings".into(), json!({ "default_php": v.to_string() }), "Saved"))).description("For sites without a pinned version, and for php in a terminal outside a project"),
            text("Domain", "tld", "Sites answer on name.<domain> and always on name.localhost"),
            text("Sites folders", "parked", "Every folder inside these is a site; separate with ;"),
            switch("Start with Windows", "autostart", "Starts tez and this tray icon when you sign in"),
        ]),
        SettingGroup::new().title("Developer tools").items([
            switch("Xdebug", "xdebug", "Step debugging on port 9003, started with the XDEBUG_TRIGGER cookie; slows PHP while on"),
            switch("Dumps", "dumps", "Copies every dump() and dd() to the web dashboard's Dumps tab"),
        ]),
        SettingGroup::new().title("Web server").items([
            number("HTTP port", "http_port", 1.0, 65535.0, "If taken, tez uses 8088 and says so on the Overview"),
            number("HTTPS port", "https_port", 1.0, 65535.0, "If taken, tez uses 8443"),
            number("PHP workers per version", "php_workers", 1.0, 10.0, "php-cgi processes; each serves one request at a time"),
        ]),
        SettingGroup::new().title("Security and HTTPS").items([
            SettingItem::new("Local certificates", SettingField::render(|_, _, _| button("trust", "Trust (admin prompt)", |cx| send(cx, "/api/jobs".into(), json!({ "args": ["trust"], "title": "Trust local HTTPS" }), "Asking Windows"))))
                .description(if trusted { "Trusted by Windows. tez's own certificate authority signs every site's certificate and never leaves this computer" } else { "Not trusted yet. tez's own certificate authority signs every site's certificate and never leaves this computer" }),
            SettingItem::new("Hosts file", SettingField::render(|_, _, _| button("hosts", "Add missing sites (admin prompt)", |cx| send(cx, "/api/jobs".into(), json!({ "args": ["hosts"], "title": "Update hosts" }), "Asking Windows"))))
                .description("Needed for .test names; .localhost names work without it"),
            about("The API listens on 127.0.0.1 only, accepts only its own host names, and refuses changes without tez's header, so web pages cannot reach it."),
        ]),
        SettingGroup::new().title("Performance").items([
            SettingItem::new("Defender", SettingField::render(|_, _, _| button("defender", "Exclude tez and site folders (admin prompt)", |cx| send(cx, "/api/jobs".into(), json!({ "args": ["doctor", "--fix"], "title": "Defender exclusions" }), "Asking Windows"))))
                .description("Scanning every PHP file read is the biggest PHP slowdown on Windows"),
            about(&format!("{} free on the tez drive. Keep 20 GB or more free: Windows slows down on a full SSD.", mb(free))),
        ]),
    ])
}

fn new_site_page(cx: &App) -> SettingPage {
    let t = tez(cx);
    let accounts: Vec<(SharedString, SharedString)> = t.identities["accounts"].as_array().map(|a| a.iter().map(|x| (ss(&x["login"]), SharedString::from(format!("{}{}", txt(&x["login"]), if x["active"].as_bool() == Some(true) { " (active)" } else { "" })))).collect()).unwrap_or_default();
    let mut routes: Vec<(SharedString, SharedString)> = t.identities["ssh_hosts"].as_array().map(|a| a.iter().map(|h| (SharedString::from(format!("ssh:{}", txt(&h["alias"]))), SharedString::from(format!("SSH · {} · {}", txt(&h["alias"]), txt(&h["identity"]).rsplit('\\').next().unwrap_or(""))))).collect()).unwrap_or_default();
    for a in t.identities["accounts"].as_array().cloned().unwrap_or_default() { routes.push((SharedString::from(format!("gh:{}", txt(&a["login"]))), SharedString::from(format!("HTTPS through gh · {}", txt(&a["login"]))))); }
    let owners: Vec<(SharedString, SharedString)> = t.github["owners"].as_array().map(|a| a.iter().map(|o| (ss(o), ss(o))).collect()).unwrap_or_default();

    let ess = SettingGroup::new().title("Where from").items([
        SettingItem::new("GitHub account", SettingField::dropdown(accounts,
            |cx| SharedString::from(tez(cx).gh_account.clone()),
            |v, cx| {
                let t = cx.global_mut::<Tez>();
                t.gh_account = v.to_string();
                t.gh_owner.clear();
                let route = t.identities["routes"].as_array().and_then(|r| r.iter().find(|x| txt(&x["login"]) == v.as_ref())).map(|x| txt(&x["route"]));
                if let Some(r) = route { t.gh_route = r; }
                load_repos(cx);
            })).description("Accounts signed in to the GitHub CLI; their tokens stay in gh's store"),
        SettingItem::new("Owner", SettingField::dropdown(owners,
            |cx| SharedString::from(tez(cx).gh_owner.clone()),
            |v, cx| { cx.global_mut::<Tez>().gh_owner = v.to_string(); load_repos(cx); })),
        SettingItem::new("Clone through", SettingField::dropdown(routes,
            |cx| SharedString::from(tez(cx).gh_route.clone()),
            |v, cx| { cx.global_mut::<Tez>().gh_route = v.to_string(); })).description("From your gh accounts and ~/.ssh/config; tez picks the one that matches the account"),
        SettingItem::new("Folder name", SettingField::input(|cx| SharedString::from(tez(cx).local.get("clone-name").cloned().unwrap_or_default()), |v, cx| { cx.global_mut::<Tez>().local.insert("clone-name".into(), v.to_string()); })).description("Defaults to the repository name"),
        SettingItem::new("Any git URL", SettingField::input(|cx| SharedString::from(tez(cx).local.get("clone-url").cloned().unwrap_or_default()), |v, cx| { cx.global_mut::<Tez>().local.insert("clone-url".into(), v.to_string()); })).description("GitLab, Bitbucket, self-hosted, or git@alias:owner/repo.git"),
        SettingItem::new("", SettingField::render(|_, _, _| row()
            .child(button("clone-url-go", "Clone URL", |cx| { let u = tez(cx).local.get("clone-url").cloned().unwrap_or_default(); if !u.is_empty() { clone_repo(cx, u) } }))
            .child(button("repos-load", "Load repositories", |cx| load_repos(cx))))),
    ]);
    let mut list = SettingGroup::new().title(if t.gh_loading { "Repositories (loading…)" } else { "Repositories" });
    for r in t.repos["repos"].as_array().cloned().unwrap_or_default().into_iter().take(200) {
        let full = txt(&r["nameWithOwner"]);
        let f2 = full.clone();
        let desc = format!("{}{}{}", if r["isPrivate"].as_bool() == Some(true) { "private · " } else { "" }, r["primaryLanguage"]["name"].as_str().map(|l| format!("{l} · ")).unwrap_or_default(), txt(&r["description"]));
        list = list.item(SettingItem::new(txt(&r["name"]), SettingField::render(move |_, _, _| button(format!("clone-{f2}"), "Clone", { let f = f2.clone(); move |cx| clone_repo(cx, f.clone()) }))).description(desc).keywords([full]));
    }
    SettingPage::new("New site from GitHub").description("Clones into your Sites folder and sets it up: composer, npm, .env, database, migrations").resettable(false).groups([ess, list])
}

fn clone_repo(cx: &mut App, repo: String) {
    let t = tez(cx);
    let route = t.gh_route.clone();
    let name = t.local.get("clone-name").cloned().unwrap_or_default();
    let target = if repo.contains("://") || repo.contains('@') { repo.clone() } else if let Some(alias) = route.strip_prefix("ssh:") { format!("git@{alias}:{repo}.git") } else { repo.clone() };
    let mut args = vec![Value::String("clone".into()), Value::String(target)];
    if !name.trim().is_empty() { args.push(Value::String(name.trim().to_string())); }
    send(cx, "/api/jobs".into(), json!({ "args": args, "title": format!("Clone {repo}") }), "Cloning and setting up");
}

// ------------------------------------------------------------------ the window

struct TezView;

impl Render for TezView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tez(cx);
        let message = t.message.as_ref().map(|(m, _)| m.clone());
        if !t.online {
            let err = if t.error.is_empty() { "Connecting…".to_string() } else { t.error.clone() };
            return div().size_full().flex().flex_col().items_center().justify_center().gap_4()
                .child(Label::new("tez").text_3xl())
                .child(Label::new(err))
                .child(button("start", "Start tez", |cx| { api::start_daemon(); notify(cx, "Starting tez"); }).primary())
                .into_any_element();
        }
        let mut pages = vec![overview_page(cx)];
        pages.extend(site_pages(cx));
        pages.push(new_site_page(cx));
        pages.extend(service_pages(cx));
        pages.extend(php_pages(cx));
        pages.push(settings_page(cx));
        let theme = cx.theme();
        let mut root = div().size_full().relative()
            .child(Settings::new("tez-settings").sidebar_width(px(250.)).pages(pages));
        if let Some(m) = message {
            root = root.child(div().absolute().bottom_4().right_4().px_4().py_2().rounded_md()
                .bg(theme.foreground).text_color(theme.background).child(m));
        }
        root.into_any_element()
    }
}

// ------------------------------------------------------------------ start

fn main() {
    // One app at a time: a second launch just brings the window forward.
    match std::net::TcpListener::bind(("127.0.0.1", INSTANCE_PORT)) {
        Ok(listener) => {
            std::thread::spawn(move || {
                for s in listener.incoming().flatten() { drop(s); SHOW_REQUESTED.store(true, Ordering::SeqCst); }
            });
        }
        Err(_) => {
            let _ = std::net::TcpStream::connect(("127.0.0.1", INSTANCE_PORT));
            return;
        }
    }
    let background = std::env::args().any(|a| a == "--background");
    application().with_assets(assets::Assets).run(move |cx| {
        init(cx);
        // Closing the window leaves tez in the tray; only the tray's Quit ends the app.
        cx.set_quit_mode(QuitMode::Explicit);
        cx.set_global(Tez::default());
        cx.set_global(MainWindow(None));
        cx.set_global(TrayHolder(tray::Tray::new()));
        if !api::get("/api/ping").is_ok() { api::start_daemon(); }
        refresh_now(cx, true);
        start_loop(cx);
        if !background { show_window(cx); }
    });
}
