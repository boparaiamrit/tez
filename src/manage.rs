//! Everything the dashboard can change: global settings, PHP settings and extensions, services,
//! a site's settings, .env and config files, background jobs, and cloning from GitHub.

use crate::config::Config;
use crate::daemon::{self, Shared};
use crate::paths;
use crate::php;
use crate::project;
use crate::services;
use crate::sites;
use crate::util::{self, R};
use crate::web;
use crate::bail;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

// ------------------------------------------------------------------ global settings

pub fn settings_get() -> Value {
    let cfg = Config::load();
    json!({
        "tld": cfg.tld, "default_php": cfg.default_php, "http_port": cfg.http_port, "https_port": cfg.https_port,
        "api_port": cfg.api_port, "php_workers": cfg.php_workers, "xdebug": cfg.xdebug, "parked": cfg.parked,
        "links": cfg.links, "dumps": paths::run_dir().join("dumps-on").exists(),
        "autostart": autostart_enabled(), "home": paths::home(),
        "php_installed": php::installed().into_iter().map(|(m, _)| m).collect::<Vec<_>>(),
        "ca_trusted": ca_trusted(), "free_disk": crate::win::free_bytes(&paths::home()),
    })
}

pub fn settings_set(shared: &Shared, body: &Value) -> R<Value> {
    let mut cfg = Config::load();
    if let Some(v) = body["tld"].as_str() {
        let t = util::host_label(v);
        if t.is_empty() || t == "localhost" { bail!("choose a domain such as test"); }
        cfg.tld = t;
    }
    if let Some(v) = body["default_php"].as_str() { cfg.default_php = php::normalize(v)?; }
    if let Some(v) = body["http_port"].as_u64() { cfg.http_port = v as u16; web::reset_ports(); }
    if let Some(v) = body["https_port"].as_u64() { cfg.https_port = v as u16; web::reset_ports(); }
    if let Some(v) = body["php_workers"].as_u64() { cfg.php_workers = (v as u16).clamp(1, 10); }
    if let Some(v) = body["xdebug"].as_bool() { cfg.xdebug = v; }
    if let Some(list) = body["parked"].as_array() {
        let dirs: Vec<String> = list.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        for d in &dirs { std::fs::create_dir_all(d).map_err(|e| format!("{d}: {e}"))?; }
        cfg.parked = dirs;
    }
    if let Some(v) = body["dumps"].as_bool() {
        let flag = paths::run_dir().join("dumps-on");
        if v { std::fs::write(flag, b"1")?; } else { let _ = std::fs::remove_file(flag); }
    }
    if let Some(v) = body["autostart"].as_bool() { set_autostart(v)?; }
    let restart_php = body.get("xdebug").is_some() || body.get("php_workers").is_some();
    let restart_web = body.get("http_port").is_some() || body.get("https_port").is_some();
    cfg.save()?;
    php::write_all_inis(&cfg)?;
    let _ = web::install_frankenphp(&cfg);
    let notes = daemon::reload(shared)?;
    if restart_php { shared.lock().unwrap().restart("php"); }
    if restart_web { shared.lock().unwrap().restart("web"); }
    Ok(json!({ "ok": true, "notes": notes }))
}

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

pub fn autostart_enabled() -> bool {
    Command::new(paths::system32("reg.exe")).args(["query", RUN_KEY, "/v", "tez"])
        .creation_flags(crate::win::CREATE_NO_WINDOW).stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

pub fn set_autostart(on: bool) -> R<()> {
    let exe = paths::bin().join("tez.exe");
    let st = if on {
        Command::new(paths::system32("reg.exe")).args(["add", RUN_KEY, "/v", "tez", "/t", "REG_SZ", "/f", "/d", &format!("\"{}\" start --quiet", exe.display())])
            .creation_flags(crate::win::CREATE_NO_WINDOW).stdout(Stdio::null()).status()?
    } else {
        Command::new(paths::system32("reg.exe")).args(["delete", RUN_KEY, "/v", "tez", "/f"])
            .creation_flags(crate::win::CREATE_NO_WINDOW).stdout(Stdio::null()).stderr(Stdio::null()).status()?
    };
    if on && !st.success() { bail!("could not register tez to start at sign-in"); }
    Ok(())
}

fn ca_trusted() -> bool {
    let root = paths::local_ca_root();
    if !root.exists() { return false; }
    let out = Command::new(paths::system32("certutil.exe")).args(["-verify"]).arg(&root)
        .creation_flags(crate::win::CREATE_NO_WINDOW).output();
    out.map(|o| { let t = String::from_utf8_lossy(&o.stdout); !t.contains("CERT_E_UNTRUSTEDROOT") && t.contains("completed successfully") }).unwrap_or(false)
}

// ------------------------------------------------------------------ PHP

pub fn php_get() -> Value {
    let cfg = Config::load();
    let versions: Vec<Value> = php::installed().into_iter().map(|(minor, version)| {
        let dir = paths::php_dir(&minor);
        let values: BTreeMap<String, String> = php::ini_values(&minor, &cfg).into_iter().collect();
        let own = cfg.php_ini_for.get(&minor).cloned().unwrap_or_default();
        let exts: Vec<Value> = php::extensions_state(&dir, &minor, &cfg).into_iter()
            .map(|(n, on, default)| json!({ "name": n, "on": on, "default": default })).collect();
        let users: Vec<String> = sites::all(&cfg).into_iter().filter(|s| s.php == minor).map(|s| s.name).collect();
        json!({ "minor": minor, "version": version, "default": cfg.default_php == minor, "values": values,
                "overridden": own.keys().collect::<Vec<_>>(), "extensions": exts, "sites": users })
    }).collect();
    let catalog: Vec<Value> = php::INI.iter().map(|(k, d, about)| json!({ "key": k, "default": d, "about": about })).collect();
    json!({ "versions": versions, "catalog": catalog, "all": cfg.php_ini, "available": php::MINORS })
}

/// body: { "settings": { key: value | null }, "extensions": { name: true|false } }
pub fn php_set(shared: &Shared, minor: &str, body: &Value) -> R<Value> {
    let mut cfg = Config::load();
    let all = minor == "all";
    let minor = if all { "all".to_string() } else { php::normalize(minor)? };
    if let Some(map) = body["settings"].as_object() {
        for (k, v) in map {
            if k.contains(['\n', '\r', '=', ';', '[']) { bail!("'{k}' is not a php.ini setting name"); }
            let target = if all { &mut cfg.php_ini } else { cfg.php_ini_for.entry(minor.clone()).or_default() };
            match v.as_str().map(|s| s.trim().to_string()) {
                Some(s) if !s.is_empty() => {
                    if s.contains(['\n', '\r']) { bail!("a value cannot span lines"); }
                    target.insert(k.clone(), s);
                }
                _ => { target.remove(k); }
            }
        }
    }
    if let Some(map) = body["extensions"].as_object() {
        if all { bail!("turn extensions on or off for one version at a time"); }
        let dir = paths::php_dir(&minor);
        let state = php::extensions_state(&dir, &minor, &cfg);
        for (name, v) in map {
            let on = v.as_bool().unwrap_or(false);
            if name == "xdebug" { cfg.xdebug = on; continue; }
            let Some((_, _, default)) = state.iter().find(|(n, _, _)| n == name) else { bail!("PHP {minor} has no {name} extension"); };
            let ons = cfg.php_ext_on.entry(minor.clone()).or_default();
            ons.retain(|x| x != name);
            let offs = cfg.php_ext_off.entry(minor.clone()).or_default();
            offs.retain(|x| x != name);
            if *default && !on { offs.push(name.clone()); }
            if !*default && on { cfg.php_ext_on.entry(minor.clone()).or_default().push(name.clone()); }
        }
    }
    cfg.save()?;
    php::write_all_inis(&cfg)?;
    let _ = web::install_frankenphp(&cfg);
    // php -v with the new ini catches mistakes before the workers restart.
    let check_minor = if all { cfg.default_php.clone() } else { minor.clone() };
    let check = Command::new(php::php_exe(&check_minor)).arg("-v").creation_flags(crate::win::CREATE_NO_WINDOW).output()?;
    let warnings = String::from_utf8_lossy(&check.stderr).trim().to_string() + &String::from_utf8_lossy(&check.stdout).lines().filter(|l| l.contains("Warning")).collect::<Vec<_>>().join("\n");
    let mut d = shared.lock().unwrap();
    if all { d.restart("php"); } else { d.restart(&format!("php-{minor}")); }
    drop(d);
    Ok(json!({ "ok": true, "warnings": warnings }))
}

// ------------------------------------------------------------------ services

pub fn services_get() -> Value {
    let cfg = Config::load();
    let list: Vec<Value> = services::CATALOG.iter().map(|i| {
        let sc = cfg.services.get(i.name).cloned().unwrap_or_default();
        let version = if sc.version.is_empty() { i.version.to_string() } else { sc.version.clone() };
        let port = if sc.port == 0 { i.port } else { sc.port };
        let options: Vec<Value> = services::options_for(i.name).iter().map(|(k, d, about)| json!({
            "key": k, "default": d, "about": about, "value": sc.options.get(*k).cloned().unwrap_or_else(|| d.to_string()),
            "secret": k.contains("key") || k.contains("password"),
        })).collect();
        json!({ "name": i.name, "title": i.title, "about": i.about, "version": version, "port": port,
                "installed": services::is_installed(i.name, &version), "enabled": cfg.services.get(i.name).map(|s| s.enabled).unwrap_or(false),
                "options": options, "env": env_snippet(i.name, port, &sc) })
    }).collect();
    json!({ "services": list })
}

fn env_snippet(name: &str, port: u16, sc: &services_cfg::ServiceConfig) -> Vec<(String, String)> {
    let p = port.to_string();
    let v = |a: &str, b: &str| (a.to_string(), b.to_string());
    match name {
        "postgres" => vec![v("DB_CONNECTION", "pgsql"), v("DB_HOST", "127.0.0.1"), v("DB_PORT", &p), v("DB_USERNAME", "postgres"), v("DB_PASSWORD", "")],
        "redis" => vec![v("REDIS_CLIENT", "phpredis"), v("REDIS_HOST", "127.0.0.1"), v("REDIS_PORT", &p), v("REDIS_PASSWORD", "null")],
        "mailpit" => vec![v("MAIL_MAILER", "smtp"), v("MAIL_HOST", "127.0.0.1"), v("MAIL_PORT", &p), v("MAIL_ENCRYPTION", "null")],
        "meilisearch" => vec![v("SCOUT_DRIVER", "meilisearch"), v("MEILISEARCH_HOST", &format!("http://127.0.0.1:{p}")), v("MEILISEARCH_KEY", sc.options.get("master-key").map(|s| s.as_str()).unwrap_or(""))],
        "rustfs" => vec![v("FILESYSTEM_DISK", "s3"), v("AWS_ACCESS_KEY_ID", services::S3_KEY), v("AWS_SECRET_ACCESS_KEY", services::S3_SECRET), v("AWS_DEFAULT_REGION", "us-east-1"), v("AWS_BUCKET", "local"), v("AWS_ENDPOINT", &format!("http://127.0.0.1:{p}")), v("AWS_USE_PATH_STYLE_ENDPOINT", "true")],
        _ => vec![],
    }
}

mod services_cfg { pub use crate::config::ServiceConfig; }

/// body: { enabled?, port?, version?, options?: { key: value|null } }
pub fn service_set(shared: &Shared, name: &str, body: &Value) -> R<Value> {
    services::info(name)?;
    let mut cfg = Config::load();
    let e = cfg.services.entry(name.to_string()).or_default();
    if let Some(v) = body["enabled"].as_bool() { e.enabled = v; }
    if let Some(v) = body["port"].as_u64() {
        let port = v as u16;
        if port < 1024 { bail!("choose a port above 1024"); }
        e.port = port;
    }
    if let Some(v) = body["version"].as_str() { e.version = v.trim_start_matches('v').to_string(); }
    if let Some(map) = body["options"].as_object() {
        for (k, v) in map {
            if k.contains(['\n', '\r', '=']) { bail!("'{k}' is not a setting name"); }
            match v.as_str().map(|s| s.to_string()) {
                Some(s) if !s.contains(['\n', '\r']) => { e.options.insert(k.clone(), s); }
                _ => { e.options.remove(k); }
            }
        }
    }
    cfg.save()?;
    let notes = daemon::reload(shared)?;
    if body.get("options").is_some() { shared.lock().unwrap().restart(name); }
    Ok(json!({ "ok": true, "notes": notes }))
}

// ------------------------------------------------------------------ sites

fn site(name: &str) -> R<sites::Site> {
    match sites::find(&Config::load(), name) { Some(s) => Ok(s), None => bail!("no site named {name}") }
}

const KNOWN_PROCS: &[(&str, &str)] = &[
    ("horizon", "Horizon: queue workers with the /horizon dashboard"),
    ("queue", "queue:work, for apps without Horizon"),
    ("reverb", "Reverb websocket server"),
    ("vite", "Vite dev server (npm run dev) with hot reload"),
    ("schedule", "schedule:work, runs the scheduler every minute"),
    ("pulse", "pulse:check, for Laravel Pulse"),
];

pub fn site_get(name: &str) -> R<Value> {
    let cfg = Config::load();
    let s = site(name)?;
    let detected = project::detect(&s);
    let procs: Vec<Value> = KNOWN_PROCS.iter().map(|(n, about)| {
        let d = detected.iter().find(|p| p.name == *n);
        json!({ "name": n, "about": about, "on": d.is_some(), "reason": d.map(|p| p.reason.clone()), "command": d.map(|p| p.command.join(" ").replace("@php", "php")) })
    }).chain(detected.iter().filter(|p| !KNOWN_PROCS.iter().any(|(n, _)| *n == p.name)).map(|p| json!({
        "name": p.name, "about": "custom process from tez.json", "on": true, "reason": p.reason, "command": p.command.join(" ").replace("@php", "php") })))
        .collect();
    let files: Vec<String> = std::fs::read_dir(s.path.join("config")).map(|rd| {
        let mut v: Vec<String> = rd.flatten().filter_map(|e| e.file_name().to_str().map(|x| x.to_string())).filter(|n| n.ends_with(".php")).collect();
        v.sort(); v
    }).unwrap_or_default();
    Ok(json!({
        "name": s.name, "path": s.path, "php": s.php, "isolated": s.settings.php.is_some(), "up": s.settings.up,
        "url": s.url(&cfg), "laravel": s.laravel, "linked": s.linked, "processes": procs,
        "config_files": files, "env": env_groups(&s.path), "has_vendor": s.path.join("vendor").exists(),
        "has_node_modules": s.path.join("node_modules").exists(),
    }))
}

/// .env keys grouped the way people look for them; secrets flagged so the UI masks them.
fn env_groups(project: &Path) -> Vec<Value> {
    let path = project.join(".env");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut keys: Vec<(String, String)> = Vec::new();
    let map = util::read_env(&path);
    for line in text.lines() {
        if let Some((k, _)) = line.trim().split_once('=') {
            let k = k.trim().trim_start_matches("export ").to_string();
            if k.starts_with('#') || k.is_empty() { continue; }
            if let Some(v) = map.get(&k) { if !keys.iter().any(|(x, _)| x == &k) { keys.push((k, v.clone())); } }
        }
    }
    const GROUPS: &[(&str, &[&str])] = &[
        ("Application", &["APP_", "LOG_", "BCRYPT_"]),
        ("Database", &["DB_"]),
        ("Cache & sessions", &["CACHE_", "SESSION_", "MEMCACHED_"]),
        ("Queues & Horizon", &["QUEUE_", "HORIZON_", "REDIS_"]),
        ("Broadcasting & Reverb", &["BROADCAST_", "REVERB_", "VITE_REVERB_", "PUSHER_", "VITE_PUSHER_"]),
        ("Mail", &["MAIL_", "POSTMARK_", "RESEND_", "MAILGUN_", "SES_"]),
        ("Search", &["SCOUT_", "MEILISEARCH_", "TYPESENSE_", "ALGOLIA_"]),
        ("Storage", &["FILESYSTEM_", "AWS_"]),
        ("Front end", &["VITE_"]),
    ];
    let secret = |k: &str| ["PASSWORD", "SECRET", "KEY", "TOKEN", "PRIVATE", "DSN"].iter().any(|w| k.contains(w)) && !k.ends_with("_KEY_ID") || k == "AWS_ACCESS_KEY_ID";
    let mut out: Vec<Value> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for (title, prefixes) in GROUPS {
        let items: Vec<Value> = keys.iter()
            .filter(|(k, _)| !used.contains(k) && prefixes.iter().any(|p| k.starts_with(p)))
            .map(|(k, v)| json!({ "key": k, "value": v, "secret": secret(k) })).collect();
        for i in &items { used.push(i["key"].as_str().unwrap_or_default().to_string()); }
        if !items.is_empty() { out.push(json!({ "title": title, "keys": items })); }
    }
    let rest: Vec<Value> = keys.iter().filter(|(k, _)| !used.contains(k)).map(|(k, v)| json!({ "key": k, "value": v, "secret": secret(k) })).collect();
    if !rest.is_empty() { out.push(json!({ "title": "Other", "keys": rest })); }
    out
}

/// body: { php?: "8.1"|null, up?: bool, processes?: { name: bool|null } }
pub fn site_set(shared: &Shared, name: &str, body: &Value) -> R<Value> {
    let s = site(name)?;
    let mut cfg = Config::load();
    let entry = cfg.site_mut(&s.name);
    if body.get("php").is_some() {
        entry.php = match body["php"].as_str() { Some(v) if !v.is_empty() => Some(php::normalize(v)?), _ => None };
    }
    if let Some(v) = body["up"].as_bool() { entry.up = v; }
    if let Some(map) = body["processes"].as_object() {
        for (k, v) in map {
            match v.as_bool() { Some(b) => { entry.processes.insert(k.clone(), b); } None => { entry.processes.remove(k); } }
        }
    }
    let wants_php = entry.php.clone();
    cfg.save()?;
    if let Some(v) = wants_php {
        if !php::is_installed(&v) && v != web::EMBEDDED_PHP {
            return Ok(json!({ "ok": true, "job": start_job(&["php".into(), "install".into(), v.clone()], None, &format!("Install PHP {v}"))? }));
        }
    }
    let notes = daemon::reload(shared)?;
    Ok(json!({ "ok": true, "notes": notes }))
}

/// body: { set: { KEY: value }, unset: [KEY] }
pub fn env_set(name: &str, body: &Value) -> R<Value> {
    let s = site(name)?;
    let path = s.path.join(".env");
    let mut updates: Vec<(&str, String)> = Vec::new();
    let set = body["set"].as_object().cloned().unwrap_or_default();
    for (k, v) in &set {
        if !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || k.is_empty() { bail!("'{k}' is not a valid .env key"); }
        let v = v.as_str().unwrap_or_default();
        if v.contains(['\n', '\r']) { bail!("{k}: a value cannot span lines"); }
        updates.push((k.as_str(), v.to_string()));
    }
    let backup = s.path.join(".env.tez-backup");
    let _ = std::fs::copy(&path, &backup);
    if !updates.is_empty() { util::write_env_keys(&path, &updates)?; }
    if let Some(unset) = body["unset"].as_array() {
        let names: Vec<String> = unset.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let kept: Vec<&str> = text.lines().filter(|l| !names.iter().any(|n| l.trim_start().starts_with(&format!("{n}=")))).collect();
        std::fs::write(&path, kept.join("\r\n") + "\r\n")?;
    }
    Ok(json!({ "ok": true }))
}

/// Only these files of a site can be read or written from the dashboard.
fn site_file(s: &sites::Site, rel: &str) -> R<PathBuf> {
    let rel = rel.replace('\\', "/");
    let ok = matches!(rel.as_str(), ".env" | ".env.example" | "tez.json" | ".php-version")
        || (rel.starts_with("config/") && rel.ends_with(".php") && rel[7..rel.len() - 4].chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    if !ok { bail!("{rel} cannot be edited from the dashboard"); }
    Ok(s.path.join(rel.replace('/', "\\")))
}

pub fn file_get(name: &str, rel: &str) -> R<String> {
    let s = site(name)?;
    Ok(std::fs::read_to_string(site_file(&s, rel)?).unwrap_or_default())
}

pub fn file_set(name: &str, rel: &str, content: &str) -> R<Value> {
    let s = site(name)?;
    let p = site_file(&s, rel)?;
    if p.exists() {
        let dir = s.path.join("storage").join("tez-backups");
        std::fs::create_dir_all(&dir)?;
        let stamp = util::utc_now().replace([':', ' '], "-");
        let _ = std::fs::copy(&p, dir.join(format!("{}.{stamp}", rel.replace(['/', '\\'], "_"))));
    }
    std::fs::write(&p, content)?;
    // A PHP config file with a syntax error would take the whole site down: check it.
    if rel.ends_with(".php") {
        let out = Command::new(php::php_exe(&s.php)).args(["-l"]).arg(&p).creation_flags(crate::win::CREATE_NO_WINDOW).output()?;
        if !out.status.success() {
            return Ok(json!({ "ok": true, "warning": String::from_utf8_lossy(&out.stdout).trim().to_string() }));
        }
    }
    Ok(json!({ "ok": true }))
}

/// The configuration Laravel actually uses (after .env), for Horizon, Reverb, queues...
pub fn effective_config(name: &str, key: &str) -> R<Value> {
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-') { bail!("bad key"); }
    let s = site(name)?;
    let code = format!("echo json_encode(config('{key}'), JSON_PARTIAL_OUTPUT_ON_ERROR);");
    let out = Command::new(php::php_exe(&s.php)).args(["artisan", "tinker", "--execute", &code])
        .current_dir(&s.path).creation_flags(crate::win::CREATE_NO_WINDOW).output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let start = text.find(['{', '[']).unwrap_or(0);
    Ok(serde_json::from_str(text[start..].trim()).unwrap_or(json!({ "error": text.trim() })))
}

// ------------------------------------------------------------------ jobs

pub struct JobRec { pub id: u32, pub title: String, pub child: Option<Child>, pub state: String, pub log: PathBuf, pub started: String }
static JOBS: Mutex<Vec<JobRec>> = Mutex::new(Vec::new());

/// Run a tez command in the background with its output in a log. Only tez's own commands.
pub fn start_job(args: &[String], cwd: Option<&Path>, title: &str) -> R<u32> {
    const ALLOWED: &[&str] = &["php", "service", "ext", "clone", "setup", "db", "adopt", "import", "composer", "npm", "artisan", "doctor", "trust", "hosts"];
    let first = args.first().map(|s| s.as_str()).unwrap_or_default();
    if !ALLOWED.contains(&first) { bail!("'{first}' cannot be run from the dashboard"); }
    let mut jobs = JOBS.lock().unwrap();
    let id = jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
    let dir = paths::logs_dir().join("jobs");
    std::fs::create_dir_all(&dir)?;
    let log = dir.join(format!("job-{id}.log"));
    let f = std::fs::File::create(&log)?;
    let bin = paths::bin();
    let (exe, rest): (PathBuf, Vec<String>) = match first {
        "composer" => (bin.join("composer.exe"), args[1..].to_vec()),
        "artisan" => (bin.join("php.exe"), std::iter::once("artisan".to_string()).chain(args[1..].iter().cloned()).collect()),
        "npm" => {
            let node_dir = which_dir("node.exe").ok_or("Node.js is not installed")?;
            (node_dir.join("node.exe"), std::iter::once(node_dir.join("node_modules\\npm\\bin\\npm-cli.js").display().to_string()).chain(args[1..].iter().cloned()).collect())
        }
        _ => (bin.join("tez.exe"), args.to_vec()),
    };
    let child = Command::new(exe).args(rest)
        .current_dir(cwd.map(|p| p.to_path_buf()).unwrap_or_else(paths::home))
        .env("PATH", project::child_path())
        .env("NO_COLOR", "1")
        .stdin(Stdio::null()).stdout(Stdio::from(f.try_clone()?)).stderr(Stdio::from(f))
        .creation_flags(crate::win::CREATE_NO_WINDOW)
        .spawn()?;
    jobs.push(JobRec { id, title: title.to_string(), child: Some(child), state: "running".into(), log, started: util::utc_now() });
    Ok(id)
}

fn which_dir(exe: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).find(|d| d.join(exe).exists())
}

pub fn jobs_get() -> Value {
    let mut jobs = JOBS.lock().unwrap();
    for j in jobs.iter_mut() {
        if let Some(c) = j.child.as_mut() {
            if let Ok(Some(st)) = c.try_wait() {
                j.state = if st.success() { "done".into() } else { format!("failed ({})", st.code().unwrap_or(-1)) };
                j.child = None;
            }
        }
    }
    let list: Vec<Value> = jobs.iter().rev().take(30).map(|j| json!({ "id": j.id, "title": j.title, "state": j.state, "started": j.started })).collect();
    json!({ "jobs": list })
}

pub fn job_log(id: u32) -> String {
    let jobs = JOBS.lock().unwrap();
    jobs.iter().find(|j| j.id == id).map(|j| util::tail(&j.log, 400)).unwrap_or_default()
}

/// body: { args: [...], site?: name, title?: text }
pub fn job_start(body: &Value) -> R<Value> {
    let args: Vec<String> = body["args"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    if args.is_empty() { bail!("nothing to run"); }
    let cwd = match body["site"].as_str() { Some(n) => Some(site(n)?.path), None => None };
    let title = body["title"].as_str().map(|s| s.to_string()).unwrap_or_else(|| args.join(" "));
    let id = start_job(&args, cwd.as_deref(), &title)?;
    Ok(json!({ "ok": true, "job": id }))
}

// ------------------------------------------------------------------ GitHub

fn gh(args: &[&str], stdin: Option<&str>) -> R<String> {
    let mut cmd = Command::new("gh");
    cmd.args(args).creation_flags(crate::win::CREATE_NO_WINDOW).stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin.is_some() { cmd.stdin(Stdio::piped()); } else { cmd.stdin(Stdio::null()); }
    let mut child = cmd.spawn().map_err(|_| "GitHub CLI (gh) is not installed: winget install GitHub.cli".to_string())?;
    if let Some(input) = stdin {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(input.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() { bail!("{}", String::from_utf8_lossy(&out.stderr).trim()); }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn github_status() -> Value {
    match gh(&["api", "user", "--jq", ".login"], None) {
        Ok(login) => {
            let orgs: Vec<String> = gh(&["api", "user/orgs", "--paginate", "--jq", ".[].login"], None).unwrap_or_default().lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            json!({ "ok": true, "login": login.trim(), "owners": std::iter::once(login.trim().to_string()).chain(orgs).collect::<Vec<_>>() })
        }
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

pub fn github_repos(owner: &str) -> R<Value> {
    if !owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') { bail!("bad owner"); }
    let out = gh(&["repo", "list", owner, "--limit", "300", "--json", "name,nameWithOwner,description,isPrivate,updatedAt,primaryLanguage,isArchived"], None)?;
    let v: Value = serde_json::from_str(&out)?;
    Ok(json!({ "repos": v }))
}

/// The token goes straight to gh's own credential store (Windows Credential Manager); tez never keeps it.
pub fn github_login(token: &str) -> R<Value> {
    let t = token.trim();
    if t.is_empty() || t.contains(char::is_whitespace) { bail!("paste a GitHub token (it starts with ghp_ or github_pat_)"); }
    gh(&["auth", "login", "--with-token", "--hostname", "github.com"], Some(t))?;
    Ok(github_status())
}
