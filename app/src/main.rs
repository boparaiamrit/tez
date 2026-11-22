//! tez desktop app: a native GPUI window over the tez daemon, plus a tray (or menu bar) icon.
//!
//! Everything it shows and changes goes through the daemon's API on 127.0.0.1, so the app, the
//! web dashboard and the `tez` command line always agree. The look follows the design canvas
//! ("Midnight Volt"): status before settings, the most used settings at the top of each page.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod api;
mod frame;
mod pages;
mod tray;
mod ui;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ state

#[derive(Clone, PartialEq, Debug, Default)]
pub enum Page {
    #[default]
    Overview,
    Sites,
    Site(String),
    NewSite,
    Services,
    Php,
    Dumps,
    Mail,
    Logs,
    Settings,
}

#[derive(Default)]
pub struct Tez {
    pub online: bool,
    pub ever_online: bool,
    pub error: String,
    pub status: Value,
    pub settings: Value,
    pub php: Value,
    pub services: Value,
    pub jobs: Value,
    pub identities: Value,
    pub sites: BTreeMap<String, Value>,
    pub github: Value,
    pub repos: Value,
    pub gh_account: String,
    pub gh_owner: String,
    pub gh_route: String,
    pub gh_loading: bool,
    /// What the user is typing, shown instead of the saved value until the save lands.
    pub drafts: BTreeMap<String, String>,
    /// Saves waiting for typing to pause: key -> (API path, body, when queued).
    pub pending: BTreeMap<String, (String, Value, Instant)>,
    pub message: Option<(String, Instant)>,
    /// Small bits of view state: filters, the chosen log, the repo picked, folder names.
    pub local: BTreeMap<String, String>,
    pub page: Page,
    pub site_tab: String,
    pub svc_sel: String,
    pub php_sel: String,
    pub log_sel: String,
    pub new_source: String,
    pub logs: BTreeMap<String, String>,
    pub dumps: Vec<Value>,
    pub dumps_seen: usize,
    pub mail: Value,
    pub open_select: Option<String>,
    pub select_closed: Option<(String, Instant)>,
    /// Groups the user opened or closed, against their default.
    pub toggled: BTreeSet<String>,
    pub theme: String,
    pub dark_applied: Option<bool>,
    pub jump: bool,
    pub job_open: Option<u64>,
    pub job_log: String,
    /// /api/metrics, /api/sites/metrics, /api/services/metrics (newer daemons).
    pub metrics: Value,
    pub site_metrics: Value,
    pub svc_metrics: Value,
    pub range: String,
    pub side_hidden: bool,
    pub back: Vec<Page>,
    pub fwd: Vec<Page>,
    pub menu_open: Option<String>,
    pub settings_open: bool,
    pub settings_sec: String,
    pub hover_x: Option<f32>,
    /// /api/catalog: every version of every service, extension and PHP with a build here.
    pub catalog: Value,
    /// PECL releases per "minor:ext", loaded when someone types an extension name.
    pub pecl: BTreeMap<String, Value>,
    pub pecl_loading: BTreeSet<String>,
}
impl Global for Tez {}

struct TrayHolder(Option<tray::Tray>);
impl Global for TrayHolder {}

struct MainWindow(Option<AnyWindowHandle>);
impl Global for MainWindow {}

static SHOW_REQUESTED: AtomicBool = AtomicBool::new(false);
const INSTANCE_PORT: u16 = 7978;

pub fn tez(cx: &App) -> &Tez { cx.global::<Tez>() }
pub fn txt(v: &Value) -> String {
    match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() }
}

pub fn notify(cx: &mut App, msg: impl Into<String>) {
    cx.global_mut::<Tez>().message = Some((msg.into(), Instant::now()));
    cx.refresh_windows();
}

/// Send a change now (switches, menus, buttons).
pub fn send(cx: &mut App, path: String, body: Value, ok: &'static str) {
    cx.spawn(async move |cx| {
        let r = cx.background_executor().spawn(async move { api::post(&path, &body) }).await;
        let _ = cx.update(|cx| {
            let msg = match &r {
                Ok(v) if v["job"].is_number() => {
                    cx.global_mut::<Tez>().job_open = v["job"].as_u64();
                    format!("{ok}; follow it under Recent activity")
                }
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

/// Run a tez command as a background job (it shows under Recent activity).
pub fn job(cx: &mut App, args: &[&str], title: &str, ok: &'static str) {
    let args: Vec<Value> = args.iter().map(|a| Value::String(a.to_string())).collect();
    send(cx, "/api/jobs".into(), json!({ "args": args, "title": title }), ok);
}

/// Queue a typed change; it is sent once typing pauses.
pub fn queue(cx: &mut App, key: String, text: String, path: String, body: Value) {
    let t = cx.global_mut::<Tez>();
    t.drafts.insert(key.clone(), text);
    t.pending.insert(key, (path, body, Instant::now()));
}

pub fn draft(cx: &App, key: &str, saved: String) -> String { tez(cx).drafts.get(key).cloned().unwrap_or(saved) }

pub fn set_local(cx: &mut App, key: &str, value: String) {
    cx.global_mut::<Tez>().local.insert(key.to_string(), value);
    cx.refresh_windows();
}

pub fn local(cx: &App, key: &str) -> String { tez(cx).local.get(key).cloned().unwrap_or_default() }

pub fn go(cx: &mut App, page: Page) {
    let t = cx.global_mut::<Tez>();
    if t.page != page { t.open_select = None; let prev = t.page.clone(); t.back.push(prev); t.fwd.clear(); if t.back.len() > 50 { t.back.remove(0); } }
    t.menu_open = None;
    t.hover_x = None;
    t.page = page.clone();
    t.jump = false;
    if page == Page::Dumps { t.dumps_seen = t.dumps.len(); }
    let load = page == Page::NewSite && t.repos.is_null() && !t.gh_loading;
    if load { load_repos(cx); }
    refresh_now(cx, false);
    cx.refresh_windows();
}

// ------------------------------------------------------------------ fetching

#[derive(Default)]
struct Want {
    full: bool,
    sites: Vec<String>,
    logs: Vec<String>,
    sitelog: Option<String>,
    dumps: bool,
    mail: Option<u16>,
    job: Option<u64>,
    metrics: Option<String>,
    site_metrics: bool,
    svc_metrics: bool,
}

#[derive(Default)]
struct Fetched {
    status: Option<Result<Value, String>>,
    jobs: Option<Value>,
    settings: Option<Value>,
    php: Option<Value>,
    services: Option<Value>,
    identities: Option<Value>,
    sites: Vec<(String, Value)>,
    logs: Vec<(String, String)>,
    dumps: Option<Vec<Value>>,
    mail: Option<Value>,
    job_log: Option<String>,
    metrics: Option<Value>,
    site_metrics: Option<Value>,
    svc_metrics: Option<Value>,
    catalog: Option<Value>,
}

/// The process whose log the site page shows ("laravel.log" for the app's own log).
pub fn site_log_choice(cx: &App, site: &str) -> String {
    let chosen = local(cx, &format!("log:{site}"));
    if chosen.is_empty() { "laravel.log".into() } else { chosen }
}

pub fn mailpit_port(cx: &App) -> Option<u16> {
    tez(cx).status["procs"].as_array()?.iter().find(|p| p["name"] == "mailpit")
        .and_then(|p| p["url"].as_str()).and_then(|u| u.rsplit(':').next()).and_then(|p| p.trim_end_matches('/').parse().ok())
}

fn want(cx: &App, full: bool) -> Want {
    let t = tez(cx);
    let mut w = Want { full, sites: site_names(cx), dumps: full || t.page == Page::Dumps, job: t.job_open, ..Default::default() };
    match &t.page {
        Page::Site(name) if t.site_tab == "logs" => {
            let sel = site_log_choice(cx, name);
            if sel == "laravel.log" { w.sitelog = Some(name.clone()); } else { w.logs.push(format!("{name}/{sel}")); }
        }
        Page::Site(_) => {}
        Page::Logs if !t.log_sel.is_empty() => w.logs.push(t.log_sel.clone()),
        _ => {}
    }
    if full || t.page == Page::Mail { w.mail = mailpit_port(cx); }
    let range = if t.range.is_empty() { "1h".to_string() } else { t.range.clone() };
    if full || t.page == Page::Overview { w.metrics = Some(range); }
    w.site_metrics = full || matches!(t.page, Page::Overview | Page::Sites | Page::Site(_));
    w.svc_metrics = full || t.page == Page::Services;
    w
}

fn fetch(w: Want) -> Fetched {
    let mut f = Fetched { status: Some(api::get("/api/status")), ..Default::default() };
    if !matches!(f.status, Some(Ok(_))) { return f; }
    f.jobs = api::get("/api/jobs").ok();
    if w.full {
        f.settings = api::get("/api/settings").ok();
        f.php = api::get("/api/php").ok();
        f.services = api::get("/api/services").ok();
        f.identities = api::get("/api/github/identities").ok();
        for n in &w.sites { if let Ok(v) = api::get(&format!("/api/site/{n}")) { f.sites.push((n.clone(), v)); } }
    }
    for name in w.logs { if let Ok(t) = api::get_text(&format!("/api/logs/{name}?n=200")) { f.logs.push((format!("proc:{name}"), t)); } }
    if let Some(s) = w.sitelog { if let Ok(t) = api::get_text(&format!("/api/sitelog/{s}?n=200")) { f.logs.push((format!("site:{s}"), t)); } }
    if w.dumps {
        if let Ok(t) = api::get_text("/api/dumps?n=100") {
            f.dumps = Some(t.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect());
        }
    }
    if let Some(port) = w.mail { f.mail = api::get_local(port, "/api/v1/messages?limit=50").ok(); }
    if let Some(id) = w.job { f.job_log = api::get_text(&format!("/api/jobs/{id}/log")).ok(); }
    if let Some(r) = w.metrics { f.metrics = api::get(&format!("/api/metrics?range={r}")).ok(); }
    if w.site_metrics { f.site_metrics = api::get("/api/sites/metrics?range=1h").ok(); }
    if w.svc_metrics { f.svc_metrics = api::get("/api/services/metrics").ok(); }
    if w.full { f.catalog = api::get("/api/catalog").ok(); }
    f
}

fn apply(cx: &mut App, f: Fetched) {
    let t = cx.global_mut::<Tez>();
    match f.status {
        Some(Ok(v)) => { t.online = true; t.ever_online = true; t.status = v; t.error.clear(); }
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
    for (k, v) in f.logs { t.logs.insert(k, v); }
    if let Some(d) = f.dumps {
        t.dumps = d;
        if t.page == Page::Dumps { t.dumps_seen = t.dumps.len(); }
        if t.dumps_seen > t.dumps.len() { t.dumps_seen = t.dumps.len(); }
    }
    if let Some(m) = f.mail { t.mail = m; }
    if let Some(l) = f.job_log { t.job_log = l; }
    if let Some(v) = f.metrics { if v.is_object() { t.metrics = v; } }
    if let Some(v) = f.site_metrics { if v.is_object() { t.site_metrics = v; } }
    if let Some(v) = f.svc_metrics { if v.is_object() { t.svc_metrics = v; } }
    if let Some(v) = f.catalog { if v["services"].is_object() { t.catalog = v; } }
    // Drop drafts the server has caught up with (no save pending for them).
    let pending: Vec<String> = t.pending.keys().cloned().collect();
    t.drafts.retain(|k, _| pending.contains(k));
    // Tray: tooltip and the Sites menu.
    let running = t.status["procs"].as_array().map(|p| p.iter().filter(|x| x["state"] == "running").count()).unwrap_or(0);
    let memory = t.status["memory"].as_u64().unwrap_or(0);
    let tip = if t.online { format!("tez · {running} running · {}", ui::mb(memory)) } else { "tez · not running".to_string() };
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

pub fn refresh_now(cx: &mut App, full: bool) {
    let w = want(cx, full);
    cx.spawn(async move |cx| {
        let f = cx.background_executor().spawn(async move { fetch(w) }).await;
        let _ = cx.update(|cx| apply(cx, f));
    })
    .detach();
}

pub fn load_repos(cx: &mut App) {
    let account = tez(cx).gh_account.clone();
    let owner = tez(cx).gh_owner.clone();
    cx.global_mut::<Tez>().gh_loading = true;
    cx.spawn(async move |cx| {
        let res = cx.background_executor().spawn(async move {
            let status = api::get(&format!("/api/github?account={account}")).unwrap_or(Value::Null);
            let owner = if owner.is_empty() { status["owners"].as_array().and_then(|o| o.first()).map(txt).unwrap_or_default() } else { owner };
            let repos = if owner.is_empty() { json!({ "repos": [] }) } else { api::get(&format!("/api/github/repos/{owner}?account={account}")).unwrap_or(json!({ "repos": [] })) };
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

pub fn clone_repo(cx: &mut App, repo: String, folder: String) {
    let route = tez(cx).gh_route.clone();
    let target = if repo.contains("://") || repo.contains('@') { repo.clone() }
        else if let Some(alias) = route.strip_prefix("ssh:") { format!("git@{alias}:{repo}.git") }
        else { repo.clone() };
    let mut args = vec![Value::String("clone".into()), Value::String(target)];
    if !folder.trim().is_empty() { args.push(Value::String(folder.trim().to_string())); }
    send(cx, "/api/jobs".into(), json!({ "args": args, "title": format!("Clone {repo}") }), "Cloning and setting up");
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
                if tick % 4 == 0 { apply_theme(cx, false); }
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
        Restart => restart_all(cx),
        Stop => send(cx, "/api/shutdown".into(), json!({}), "Stopping tez"),
        Quit => cx.quit(),
    }
}

pub fn restart_all(cx: &mut App) {
    send(cx, "/api/proc/web/restart".into(), json!({}), "Restarting the web server and PHP");
    send(cx, "/api/proc/php/restart".into(), json!({}), "Restarting the web server and PHP");
}

// ------------------------------------------------------------------ theme

/// Pick dark or light (from the setting, or the system when it is "system") and give the
/// GPUI Kit components the same colours and fonts as the rest of the app.
pub fn apply_theme(cx: &mut App, force: bool) {
    let choice = tez(cx).theme.clone();
    let dark = match choice.as_str() {
        "dark" => true,
        "light" => false,
        _ => matches!(cx.window_appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark),
    };
    if !force && tez(cx).dark_applied == Some(dark) { return; }
    cx.global_mut::<Tez>().dark_applied = Some(dark);
    let p = if dark { ui::Pal::dark() } else { ui::Pal::light() };
    cx.set_global(ui::PalHolder(p));
    Theme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    let th = cx.global_mut::<Theme>();
    th.font_family = ui::SANS.into();
    th.mono_font_family = ui::MONO.into();
    th.font_size = px(14.);
    th.radius = px(8.);
    let c = &mut th.colors;
    c.background = p.bg;
    c.foreground = p.text;
    c.border = p.line2;
    c.input = p.line2;
    c.muted = p.surface2;
    c.muted_foreground = p.text3;
    c.primary = p.accent;
    c.primary_hover = p.accent_hi;
    c.primary_active = p.accent;
    c.primary_foreground = white();
    c.ring = p.accent_hi;
    c.popover = p.surface2;
    c.popover_foreground = p.text;
    c.caret = p.text;
    c.accent = p.surface2;
    c.accent_foreground = p.text;
    cx.refresh_windows();
}

pub fn set_theme(cx: &mut App, choice: String) {
    cx.global_mut::<Tez>().theme = choice.clone();
    let mut prefs = api::app_prefs();
    if !prefs.is_object() { prefs = json!({}); }
    prefs["theme"] = Value::String(choice);
    api::save_app_prefs(&prefs);
    apply_theme(cx, true);
}

// ------------------------------------------------------------------ the window

fn show_window(cx: &mut App) {
    if let Some(h) = cx.global::<MainWindow>().0 {
        if h.update(cx, |_, window, _| window.activate_window()).is_ok() { return; }
    }
    let mac = cfg!(target_os = "macos");
    // Windows and macOS: tez draws its own title bar (menus, back and forward, window buttons on
    // Windows; the traffic lights stay native on macOS). Linux keeps the desktop's own frame.
    let custom = cfg!(any(windows, target_os = "macos"));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1240.), px(820.)), cx))),
        window_min_size: Some(size(px(880.), px(600.))),
        titlebar: Some(TitlebarOptions {
            title: Some("tez".into()),
            appears_transparent: custom,
            traffic_light_position: if mac { Some(point(px(16.), px(15.))) } else { None },
        }),
        app_id: Some("dev.tez.app".into()),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, |_window, cx| cx.new(|_| TezView)) {
        Ok((h, _)) => {
            cx.global_mut::<MainWindow>().0 = Some(h);
            cx.activate(true);
        }
        Err(e) => eprintln!("tez: {e}"),
    }
}

struct TezView;

impl Render for TezView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        frame::root(window, cx)
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
            // tez is already running. A background start (tez start, sign-in) stays silent; only a
            // launch the user asked for brings the existing window forward.
            if !std::env::args().any(|a| a == "--background") { let _ = std::net::TcpStream::connect(("127.0.0.1", INSTANCE_PORT)); }
            return;
        }
    }
    let background = std::env::args().any(|a| a == "--background");
    application().with_assets(assets::AllAssets).run(move |cx| {
        init(cx);
        let _ = cx.text_system().add_fonts(vec![
            Cow::Borrowed(include_bytes!("../assets/fonts/Inter-400.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/Inter-500.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/Inter-600.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/Inter-700.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-400.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-500.ttf").as_slice()),
        ]);
        // Closing the window leaves tez in the tray; only the tray's Quit ends the app.
        cx.set_quit_mode(QuitMode::Explicit);
        let theme = api::app_prefs()["theme"].as_str().unwrap_or("system").to_string();
        cx.set_global(Tez { theme, site_tab: "overview".into(), new_source: "github".into(), range: "1h".into(), settings_sec: "general".into(), ..Default::default() });
        cx.set_global(MainWindow(None));
        cx.set_global(TrayHolder(tray::Tray::new()));
        apply_theme(cx, true);
        if api::get("/api/ping").is_err() { api::start_daemon(); }
        refresh_now(cx, true);
        start_loop(cx);
        if !background { show_window(cx); }
    });
}

/// Load PECL's releases of `ext` for PHP `minor` once, in the background.
pub fn ensure_pecl(cx: &mut App, minor: &str, ext: &str) {
    let key = format!("{minor}:{ext}");
    let t = cx.global_mut::<Tez>();
    if t.pecl.contains_key(&key) || t.pecl_loading.contains(&key) { return; }
    t.pecl_loading.insert(key.clone());
    let path = format!("/api/php/{minor}/pecl/{ext}");
    cx.spawn(async move |cx| {
        let r = cx.background_executor().spawn(async move { api::get(&path) }).await;
        let _ = cx.update(|cx| {
            let t = cx.global_mut::<Tez>();
            t.pecl_loading.remove(&key);
            t.pecl.insert(key, r.unwrap_or_else(|e| json!({ "error": e })));
            cx.refresh_windows();
        });
    })
    .detach();
}