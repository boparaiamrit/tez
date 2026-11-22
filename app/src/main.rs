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

// GPUI's macros name `gpui::`; gpui-kit asks apps to alias it so they resolve through the Kit.
extern crate gpui_kit as gpui;

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
    Node,
    Dumps,
    Mail,
    Logs,
    Settings,
}

/// What a change in flight waits for in /api/status before it counts as done.
#[derive(Clone, Debug)]
pub enum Check {
    /// Done when the request returns.
    None,
    /// One process (a service, or "site/process"): running when turned on, gone or stopped when off.
    Proc(String),
    /// A site's processes: one running when turned on, all stopped when off.
    Site(String),
}

#[derive(Clone, Debug)]
pub struct Busy {
    /// The side the switch is moving to.
    pub on: bool,
    /// "Stopping PostgreSQL", shown with a spinner.
    pub doing: String,
    /// "PostgreSQL stopped. Its data is kept.", shown when it is done.
    pub done: String,
    pub since: Instant,
    /// The request has returned; now it waits for the status to show the change.
    pub sent: bool,
    pub check: Check,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub tone: ui::Tone,
    pub at: Instant,
}

#[derive(Default)]
pub struct Tez {
    pub online: bool,
    pub ever_online: bool,
    pub error: String,
    pub status: Value,
    pub settings: Value,
    pub php: Value,
    /// /api/node: installed Node.js versions and the newest of each line.
    pub node: Value,
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
    /// The toast in the corner: what just finished or went wrong.
    pub message: Option<Toast>,
    pub toast_seq: u64,
    /// Changes on their way: key ("svc:postgres", "site:hello", "proc:hello/vite", "req:7") ->
    /// what it is doing. A switch shows its new side with a spinner, and a toast says what is
    /// happening, until tez reports it done.
    pub busy: BTreeMap<String, Busy>,
    /// The main panel's scroll position (so `go scroll:bottom` can show the end of a page).
    pub panel_scroll: ScrollHandle,
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
    /// What has been typed into the open long menu to filter it, and the focus that receives it.
    pub combo_q: String,
    pub combo_focus: Option<FocusHandle>,
    /// The open right-click menu: where, and what it is for ("site:hello", "svc:postgres", "php:8.4").
    pub ctx: Option<(Point<Pixels>, String)>,
    /// Show the offline screen while tez is running (`go offline`), to check it.
    pub preview_offline: bool,
    pub select_closed: Option<(String, Instant)>,
    /// Groups the user opened or closed, against their default.
    pub toggled: BTreeSet<String>,
    pub theme: String,
    pub dark_applied: Option<bool>,
    pub jump: bool,
    /// The note shown the first time the window is closed: tez keeps running in the tray.
    pub close_hint: bool,
    /// The Remove site dialog: which site, what tez found about it, the choices, and a save in flight.
    pub del_site: Option<String>,
    pub del_preview: Value,
    pub del_files: bool,
    pub del_db: bool,
    pub del_busy: bool,
    /// Folders in a Sites folder that tez no longer serves (removed from tez, kept on disk).
    pub hidden_sites: Value,
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

gpui_kit::actions!(tez, [GoHome, GoSites, GoServices, GoPhp, GoDebug, OpenSettings, Jump, NewSite, Refresh, Dismiss, GoBack, GoForward, ToggleSidebar, QuitApp]);

pub fn go_back(cx: &mut App) {
    let t = cx.global_mut::<Tez>();
    if let Some(prev) = t.back.pop() { let cur = t.page.clone(); t.fwd.push(cur); t.page = prev; t.menu_open = None; refresh_now(cx, false); cx.refresh_windows(); }
}

pub fn go_forward(cx: &mut App) {
    let t = cx.global_mut::<Tez>();
    if let Some(next) = t.fwd.pop() { let cur = t.page.clone(); t.back.push(cur); t.page = next; t.menu_open = None; refresh_now(cx, false); cx.refresh_windows(); }
}

/// Close whatever floats over the page: menus, the jump box, Settings, dialogs.
pub fn dismiss(cx: &mut App) {
    let t = cx.global_mut::<Tez>();
    t.open_select = None;
    t.menu_open = None;
    t.ctx = None;
    t.jump = false;
    t.settings_open = false;
    t.del_site = None;
    t.close_hint = false;
    cx.refresh_windows();
}

/// The keyboard: Ctrl (⌘ on macOS) + 1-5 for the sections, K to jump, comma for Settings, N for a
/// new site, R or F5 to refresh, B for the sidebar, Alt+arrows for back and forward, Esc to close.
fn bind_shortcuts(cx: &mut App) {
    // ⌘ on macOS, Ctrl elsewhere (this GPUI does not parse "secondary-").
    cx.bind_keys([
        KeyBinding::new(if cfg!(target_os = "macos") { "cmd-1" } else { "ctrl-1" }, GoHome, None),
        KeyBinding::new(if cfg!(target_os = "macos") { "cmd-2" } else { "ctrl-2" }, GoSites, None),
        KeyBinding::new(if cfg!(target_os = "macos") { "cmd-3" } else { "ctrl-3" }, GoServices, None),
        KeyBinding::new(if cfg!(target_os = "macos") { "cmd-4" } else { "ctrl-4" }, GoPhp, None),
        KeyBinding::new(if cfg!(target_os = "macos") { "cmd-5" } else { "ctrl-5" }, GoDebug, None),
        KeyBinding::new(if cfg!(target_os = "macos") { "cmd-," } else { "ctrl-," }, OpenSettings, None),
        KeyBinding::new(if cfg!(target_os = "macos") { "cmd-k" } else { "ctrl-k" }, Jump, None),
        KeyBinding::new(if cfg!(target_os = "macos") { "cmd-n" } else { "ctrl-n" }, NewSite, None),
        KeyBinding::new(if cfg!(target_os = "macos") { "cmd-r" } else { "ctrl-r" }, Refresh, None),
        KeyBinding::new("f5", Refresh, None),
        KeyBinding::new(if cfg!(target_os = "macos") { "cmd-b" } else { "ctrl-b" }, ToggleSidebar, None),
        KeyBinding::new("alt-left", GoBack, None),
        KeyBinding::new("alt-right", GoForward, None),
        KeyBinding::new("escape", Dismiss, None),
        KeyBinding::new(if cfg!(target_os = "macos") { "cmd-q" } else { "ctrl-q" }, QuitApp, None),
    ]);
    cx.on_action(|_: &GoHome, cx| go(cx, Page::Overview));
    cx.on_action(|_: &GoSites, cx| go(cx, Page::Sites));
    cx.on_action(|_: &GoServices, cx| go(cx, Page::Services));
    cx.on_action(|_: &GoPhp, cx| go(cx, Page::Php));
    cx.on_action(|_: &GoDebug, cx| go(cx, Page::Dumps));
    cx.on_action(|_: &OpenSettings, cx| { cx.global_mut::<Tez>().settings_open = true; cx.refresh_windows(); });
    cx.on_action(|_: &Jump, cx| { cx.global_mut::<Tez>().jump = true; cx.refresh_windows(); });
    cx.on_action(|_: &NewSite, cx| go(cx, Page::NewSite));
    cx.on_action(|_: &Refresh, cx| { refresh_now(cx, true); notify(cx, "Refreshed".to_string()); });
    cx.on_action(|_: &ToggleSidebar, cx| { let t = cx.global_mut::<Tez>(); t.side_hidden = !t.side_hidden; cx.refresh_windows(); });
    cx.on_action(|_: &GoBack, cx| go_back(cx));
    cx.on_action(|_: &GoForward, cx| go_forward(cx));
    cx.on_action(|_: &Dismiss, cx| dismiss(cx));
    cx.on_action(|_: &QuitApp, cx| cx.quit());
}
/// A page asked for over the instance port (`go sites`, `go site:hello:logs`, `go settings:ports`),
/// without bringing the window forward: used to check every page.
static NAV_REQUEST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Open the page a `go` request names.
fn navigate(cx: &mut App, target: &str) {
    let mut parts = target.trim().splitn(3, ':');
    let (a, b, c) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    {
        let t = cx.global_mut::<Tez>();
        t.settings_open = false;
        t.jump = false;
        t.close_hint = false;
        t.del_site = None;
        t.open_select = None;
        t.ctx = None;
        t.preview_offline = false;
        t.menu_open = None;
    }
    match a {
        "home" => go(cx, Page::Overview),
        "sites" => go(cx, Page::Sites),
        "site" => { cx.global_mut::<Tez>().site_tab = if c.is_empty() { "overview".into() } else { c.into() }; go(cx, Page::Site(b.to_string())); }
        "new" => { if !b.is_empty() { cx.global_mut::<Tez>().new_source = b.to_string(); } go(cx, Page::NewSite); }
        "services" => { if !b.is_empty() { cx.global_mut::<Tez>().svc_sel = b.to_string(); } go(cx, Page::Services); }
        "php" => go(cx, Page::Php),
        "node" => go(cx, Page::Node),
        "dumps" => go(cx, Page::Dumps),
        "mail" => go(cx, Page::Mail),
        "logs" => go(cx, Page::Logs),
        "settings" => { let t = cx.global_mut::<Tez>(); t.settings_open = true; t.settings_sec = b.to_string(); }
        // `go flip:svc:redis` or `go flip:site:hello`: flip that switch the way a click does.
        "flip" => {
            let t = tez(cx);
            match b {
                "svc" => {
                    let on = t.status["procs"].as_array().map(|a| a.iter().any(|x| txt(&x["name"]) == c && x["wanted"] != false)).unwrap_or(false);
                    let title = t.services["services"].as_array().and_then(|a| a.iter().find(|s| txt(&s["name"]) == c)).map(|s| txt(&s["title"])).unwrap_or_else(|| c.to_string());
                    flip_service(cx, c, &title, !on);
                }
                "site" => {
                    let up = t.status["sites"].as_array().and_then(|a| a.iter().find(|s| txt(&s["name"]) == c)).map(|s| s["up"] == true).unwrap_or(false);
                    flip_site(cx, c, !up);
                }
                _ => {}
            }
        }
        "scroll" => { if b == "bottom" { tez(cx).panel_scroll.scroll_to_bottom(); } else { tez(cx).panel_scroll.set_offset(point(px(0.), px(0.))); } cx.refresh_windows(); }
        "close-note" => cx.global_mut::<Tez>().close_hint = true,
        "offline" => cx.global_mut::<Tez>().preview_offline = true,
        "menu" => cx.global_mut::<Tez>().menu_open = Some(b.to_string()),
        "ctx" => { cx.global_mut::<Tez>().ctx = Some((point(px(420.), px(220.)), format!("{b}:{c}"))); }
        "remove" => { open_remove(cx, b.to_string()); if c == "files" { cx.global_mut::<Tez>().del_files = true; } }
        "theme" => set_theme(cx, b.to_string()),
        "open" => { let t = cx.global_mut::<Tez>(); t.open_select = Some(b.to_string()); t.combo_q = c.to_string(); }
        _ => {}
    }
    refresh_now(cx, true);
    cx.refresh_windows();
}
const INSTANCE_PORT: u16 = 7978;

pub fn tez(cx: &App) -> &Tez { cx.global::<Tez>() }
pub fn txt(v: &Value) -> String {
    match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() }
}

pub fn notify(cx: &mut App, msg: impl Into<String>) { toast(cx, ui::Tone::Neutral, msg); }

/// Show a toast: Ok for done, Err for a failure (it stays longer), Warn, or Neutral.
pub fn toast(cx: &mut App, tone: ui::Tone, msg: impl Into<String>) {
    let t = cx.global_mut::<Tez>();
    t.toast_seq += 1;
    t.message = Some(Toast { id: t.toast_seq, text: msg.into(), tone, at: Instant::now() });
    cx.refresh_windows();
}

/// The progress line for a change that is still going, from the message it will end with.
fn doing_label(ok: &str) -> String {
    let first = ok.split([';', '.']).next().unwrap_or(ok).trim();
    if first.ends_with("ing") { first.to_string() } else if first.starts_with("Saved") { "Saving".into() } else { "Working on it".into() }
}

/// Turn something on or off. The switch moves at once and spins, a toast says what is happening,
/// and the request runs in the background; when tez reports the change done the toast says so.
/// A failure puts the switch back and says why. A second click while it runs is ignored.
#[allow(clippy::too_many_arguments)]
pub fn flip(cx: &mut App, key: String, on: bool, doing: String, done: String, check: Check, path: String, body: Value) {
    if tez(cx).busy.contains_key(&key) { return; }
    cx.global_mut::<Tez>().busy.insert(key.clone(), Busy { on, doing, done, since: Instant::now(), sent: false, check });
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        let r = cx.background_executor().spawn(async move { api::post(&path, &body) }).await;
        let _ = cx.update(|cx| {
            match r {
                Ok(_) => { if let Some(b) = cx.global_mut::<Tez>().busy.get_mut(&key) { b.sent = true; } }
                Err(e) => { cx.global_mut::<Tez>().busy.remove(&key); toast(cx, ui::Tone::Err, e); }
            }
            refresh_now(cx, true);
        });
    })
    .detach();
}

/// Whether a switch is mid-change, and to which side.
pub fn busy_on(cx: &App, key: &str) -> Option<bool> { tez(cx).busy.get(key).map(|b| b.on) }

pub fn flip_service(cx: &mut App, name: &str, title: &str, on: bool) {
    let (doing, done) = if on { (format!("Starting {title}"), format!("{title} is running")) } else { (format!("Stopping {title}"), format!("{title} stopped. Its data is kept.")) };
    flip(cx, format!("svc:{name}"), on, doing, done, Check::Proc(name.to_string()), format!("/api/service/{name}"), json!({ "enabled": on }));
}

pub fn flip_site(cx: &mut App, site: &str, on: bool) {
    let (doing, done) = if on { (format!("Starting {site}'s processes"), format!("{site}'s processes are running")) } else { (format!("Stopping {site}'s processes"), format!("{site}'s processes stopped")) };
    flip(cx, format!("site:{site}"), on, doing, done, Check::Site(site.to_string()), format!("/api/site/{site}/{}", if on { "up" } else { "down" }), json!({}));
}

/// One of a site's processes. With the site's processes running it starts or stops now;
/// otherwise the choice is saved for the next start.
pub fn flip_proc(cx: &mut App, site: &str, process: &str, on: bool, site_up: bool) {
    let label = { let mut c = process.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() };
    let (doing, done, check) = if site_up {
        if on { (format!("Starting {label}"), format!("{label} is running"), Check::Proc(format!("{site}/{process}"))) }
        else { (format!("Stopping {label}"), format!("{label} stopped"), Check::Proc(format!("{site}/{process}"))) }
    } else if on { (format!("Turning {label} on"), format!("{label} starts with {site}'s processes"), Check::None) }
    else { (format!("Turning {label} off"), format!("{label} is off"), Check::None) };
    let mut procs = serde_json::Map::new();
    procs.insert(process.to_string(), Value::Bool(on));
    flip(cx, format!("proc:{site}/{process}"), on, doing, done, check, format!("/api/site/{site}"), json!({ "processes": procs }));
}

/// Has the status caught up with a change? None while it is still going, Err when it failed.
fn settled(status: &Value, b: &Busy) -> Option<Result<(), String>> {
    let procs = status["procs"].as_array().cloned().unwrap_or_default();
    let state = |x: &Value| txt(&x["state"]);
    match &b.check {
        Check::None => Some(Ok(())),
        Check::Proc(n) => {
            let p = procs.iter().find(|x| txt(&x["name"]) == *n);
            match (b.on, p.map(state).as_deref()) {
                (true, Some("running")) => Some(Ok(())),
                (true, Some("failed")) => Some(Err(p.map(|x| txt(&x["last_exit"])).filter(|e| !e.is_empty()).unwrap_or_else(|| "it stopped straight away; its log has the details".into()))),
                (false, None | Some("stopped") | Some("failed")) => Some(Ok(())),
                _ => None,
            }
        }
        Check::Site(n) => {
            let group = format!("site:{n}");
            let live: Vec<String> = procs.iter().filter(|x| txt(&x["group"]) == group).map(state).collect();
            if b.on {
                if live.iter().any(|s| s == "running") { Some(Ok(())) } else if !live.is_empty() && live.iter().all(|s| s == "failed") { Some(Err("its processes stopped straight away; their logs have the details".into())) } else { None }
            } else if live.iter().all(|s| s == "stopped" || s == "failed") { Some(Ok(())) } else { None }
        }
    }
}

/// Change one setting: the switch or choice shows the new value at once, and tez saves it in the
/// background. A save that fails puts the old value back (the refresh after it) and says why.
pub fn set_setting(cx: &mut App, key: &str, value: Value, ok: &'static str) {
    let t = cx.global_mut::<Tez>();
    if !t.settings.is_object() { t.settings = json!({}); }
    t.settings[key] = value.clone();
    cx.refresh_windows();
    send(cx, "/api/settings".into(), json!({ key: value }), ok);
}

/// Send a change now (switches, menus, buttons).
pub fn send(cx: &mut App, path: String, body: Value, ok: &'static str) {
    // A settings change only needs the settings and status back, which is quick; the rest of the
    // app catches up on the next regular refresh.
    let settings_only = path == "/api/settings";
    // A request that takes a moment shows a progress toast until it returns.
    let key = { let t = cx.global_mut::<Tez>(); t.toast_seq += 1; format!("req:{}", t.toast_seq) };
    cx.global_mut::<Tez>().busy.insert(key.clone(), Busy { on: true, doing: doing_label(ok), done: String::new(), since: Instant::now(), sent: false, check: Check::None });
    cx.spawn(async move |cx| {
        let r = cx.background_executor().spawn(async move { api::post(&path, &body) }).await;
        let _ = cx.update(|cx| {
            cx.global_mut::<Tez>().busy.remove(&key);
            let (tone, msg) = match &r {
                Ok(v) if v["job"].is_number() => {
                    cx.global_mut::<Tez>().job_open = v["job"].as_u64();
                    (ui::Tone::Neutral, format!("{ok}; follow it under Recent activity"))
                }
                Ok(v) if v["warnings"].as_str().map(|s| !s.is_empty()).unwrap_or(false) => (ui::Tone::Warn, v["warnings"].as_str().unwrap_or_default().to_string()),
                Ok(_) => (ui::Tone::Ok, ok.to_string()),
                Err(e) => (ui::Tone::Err, e.clone()),
            };
            toast(cx, tone, msg);
            if settings_only { refresh_settings(cx) } else { refresh_now(cx, true) }
        });
    })
    .detach();
}

/// Fetch just the status and the settings, then redraw: the quick refresh after a settings change.
pub fn refresh_settings(cx: &mut App) {
    let mut w = want(cx, false);
    w.settings = true;
    cx.spawn(async move |cx| {
        let f = cx.background_executor().spawn(async move { fetch(w) }).await;
        let _ = cx.update(|cx| apply(cx, f));
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
    /// Settings without the rest of a full refresh.
    settings: bool,
    sites: Vec<String>,
    logs: Vec<String>,
    sitelog: Option<String>,
    dumps: bool,
    mail: Option<u16>,
    job: Option<u64>,
    metrics: Option<String>,
    site_metrics: bool,
    svc_metrics: bool,
    node: bool,
}

#[derive(Default)]
struct Fetched {
    status: Option<Result<Value, String>>,
    jobs: Option<Value>,
    settings: Option<Value>,
    php: Option<Value>,
    services: Option<Value>,
    identities: Option<Value>,
    hidden: Option<Value>,
    sites: Vec<(String, Value)>,
    logs: Vec<(String, String)>,
    dumps: Option<Vec<Value>>,
    mail: Option<Value>,
    job_log: Option<String>,
    metrics: Option<Value>,
    site_metrics: Option<Value>,
    svc_metrics: Option<Value>,
    catalog: Option<Value>,
    node: Option<Value>,
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
    w.node = full || t.page == Page::Node;
    w
}

fn fetch(w: Want) -> Fetched {
    let mut f = Fetched { status: Some(api::get("/api/status")), ..Default::default() };
    if !matches!(f.status, Some(Ok(_))) { return f; }
    f.jobs = api::get("/api/jobs").ok();
    if w.full || w.settings { f.settings = api::get("/api/settings").ok(); }
    if w.full {
        f.php = api::get("/api/php").ok();
        f.services = api::get("/api/services").ok();
        f.identities = api::get("/api/github/identities").ok();
        f.hidden = api::get("/api/sites/hidden").ok();
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
    if w.node { f.node = api::get("/api/node").ok(); }
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
    if let Some(v) = f.hidden { t.hidden_sites = v; }
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
    if let Some(v) = f.node { if v.is_object() { t.node = v; } }
    // Changes in flight that the status now shows done (or failed, or never finished).
    let status = t.status.clone();
    let mut finished: Vec<(String, ui::Tone, String)> = Vec::new();
    for (k, b) in t.busy.iter() {
        if !b.sent { continue; }
        match settled(&status, b) {
            Some(Ok(())) => finished.push((k.clone(), ui::Tone::Ok, b.done.clone())),
            Some(Err(e)) => finished.push((k.clone(), ui::Tone::Err, format!("{} failed: {e}", b.doing))),
            None if b.since.elapsed() > Duration::from_secs(45) => finished.push((k.clone(), ui::Tone::Warn, format!("{} is taking longer than usual; its log has the details", b.doing))),
            None => {}
        }
    }
    for (k, _, _) in &finished { t.busy.remove(k); }
    if let Some((_, tone, msg)) = finished.into_iter().last() { t.toast_seq += 1; t.message = Some(Toast { id: t.toast_seq, text: msg, tone, at: Instant::now() }); }
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
                if let Some(target) = NAV_REQUEST.lock().ok().and_then(|mut n| n.take()) { navigate(cx, &target); }
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
                if let Some(m) = &tez(cx).message {
                    let keep = if m.tone == ui::Tone::Err { 8 } else { 4 };
                    if m.at.elapsed() > Duration::from_secs(keep) { cx.global_mut::<Tez>().message = None; cx.refresh_windows(); }
                }
                // A progress toast appears once a request has taken a moment.
                if tez(cx).busy.values().any(|b| b.since.elapsed() > Duration::from_millis(300) && b.since.elapsed() < Duration::from_millis(600)) { cx.refresh_windows(); }
                if tick % 4 == 0 { apply_theme(cx, false); }
                // Every 2 s, or every 500 ms while a switch is waiting for tez to finish.
                let changing = tez(cx).busy.values().any(|b| b.sent);
                if tick % 8 == 0 || (changing && tick % 2 == 0) { refresh_now(cx, tick % 40 == 0); }
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
    th.font_family = ui::sans().into();
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
        // Mica on Windows 11, vibrancy on macOS: the desktop colour shows faintly through the sidebar.
        window_background: if ui::window_material() { if cfg!(windows) { WindowBackgroundAppearance::MicaBackdrop } else { WindowBackgroundAppearance::Blurred } } else { WindowBackgroundAppearance::Opaque },
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        // The first close explains that tez keeps running in the tray (with a Quit choice);
        // after that, closing just hides the window.
        window.on_window_should_close(cx, |_window, cx| {
            if api::app_prefs()["close_hint_seen"].as_bool().unwrap_or(false) { return true; }
            cx.global_mut::<Tez>().close_hint = true;
            cx.refresh_windows();
            false
        });
        cx.new(|_| TezView)
    }) {
        Ok((h, _)) => {
            cx.global_mut::<MainWindow>().0 = Some(h);
            cx.activate(true);
        }
        Err(e) => eprintln!("tez: {e}"),
    }
}

/// Open the Remove site dialog and load what removing it would touch.
pub fn open_remove(cx: &mut App, name: String) {
    {
        let t = cx.global_mut::<Tez>();
        t.del_site = Some(name.clone());
        t.del_preview = Value::Null;
        t.del_files = false;
        t.del_db = false;
        t.del_busy = false;
    }
    set_local(cx, "del-confirm", String::new());
    cx.spawn(async move |cx| {
        let n2 = name.clone();
        let r = cx.background_executor().spawn(async move { api::get(&format!("/api/site/{n2}/delete-preview")) }).await;
        let _ = cx.update(|cx| {
            let t = cx.global_mut::<Tez>();
            if t.del_site.as_deref() != Some(name.as_str()) { return; }
            t.del_preview = r.unwrap_or_else(|e| json!({ "error": e }));
            cx.refresh_windows();
        });
    })
    .detach();
    cx.refresh_windows();
}

/// Remove the site as chosen in the dialog.
pub fn do_remove(cx: &mut App) {
    let t = cx.global_mut::<Tez>();
    let Some(name) = t.del_site.clone() else { return };
    if t.del_busy { return; }
    t.del_busy = true;
    let body = json!({ "files": t.del_files, "database": t.del_db, "confirm": local(cx, "del-confirm").trim() });
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        let n2 = name.clone();
        let r = cx.background_executor().spawn(async move { api::post(&format!("/api/site/{n2}/delete"), &body) }).await;
        let _ = cx.update(|cx| {
            match r {
                Ok(v) => {
                    let msg = v["removed"].as_str().map(|s| s.to_string()).unwrap_or_else(|| format!("{name} removed"));
                    cx.global_mut::<Tez>().del_site = None;
                    notify(cx, msg);
                    go(cx, Page::Sites);
                }
                Err(e) => { cx.global_mut::<Tez>().del_busy = false; notify(cx, e); }
            }
            refresh_now(cx, true);
        });
    })
    .detach();
}

/// Answer the first-close note: remember it, then close the window (tez stays in the tray) or quit.
pub fn close_window(window: &mut Window, cx: &mut App, quit: bool) {
    let mut prefs = api::app_prefs();
    if !prefs.is_object() { prefs = json!({}); }
    prefs["close_hint_seen"] = json!(true);
    api::save_app_prefs(&prefs);
    cx.global_mut::<Tez>().close_hint = false;
    if quit { cx.quit(); } else { window.remove_window(); }
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
                for mut s in listener.incoming().flatten() {
                    // A bare connection (a second launch, `tez open`) shows the window; `go <page>`
                    // only switches pages.
                    use std::io::Read;
                    let _ = s.set_read_timeout(Some(Duration::from_millis(150)));
                    let mut buf = [0u8; 256];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let msg = String::from_utf8_lossy(&buf[..n]).to_string();
                    match msg.trim().strip_prefix("go ") {
                        Some(target) => { if let Ok(mut q) = NAV_REQUEST.lock() { *q = Some(target.to_string()); } }
                        None => SHOW_REQUESTED.store(true, Ordering::SeqCst),
                    }
                }
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
        cx.set_global(Tez { theme, site_tab: "overview".into(), new_source: "github".into(), range: "1h".into(), settings_sec: "general".into(), combo_focus: Some(cx.focus_handle()), ..Default::default() });
        cx.set_global(MainWindow(None));
        bind_shortcuts(cx);
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