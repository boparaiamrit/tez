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
use crate::sys::{self, Hidden};
use std::collections::BTreeMap;
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
        "ca_trusted": ca_trusted(), "free_disk": sys::free_bytes(&paths::home()),
        "web_server": web::server(&cfg), "web_server_wanted": cfg.web_server,
        "web_servers": web_servers(),
        "os": sys::os(), "arch": sys::arch(),
    })
}

/// Each web server tez can run here: whether it can, and why not.
fn web_servers() -> Value {
    let (ok, reason) = crate::nginx::available();
    json!({
        "caddy": { "available": true, "reason": "", "version": web::CADDY_VERSION },
        "nginx": { "available": ok, "reason": reason, "version": crate::nginx::VERSION },
    })
}

pub fn settings_set(shared: &Shared, body: &Value) -> R<Value> {
    let mut cfg = Config::load();
    if let Some(v) = body["web_server"].as_str() {
        let v = v.trim().to_ascii_lowercase();
        if v != "caddy" && v != "nginx" { bail!("the web server is caddy or nginx, not {v}"); }
        if v == "nginx" { let (ok, why) = crate::nginx::available(); if !ok { bail!("{why}"); } }
        cfg.web_server = v;
        // Download it before switching, so a failed download leaves the current server running.
        web::install_server(&cfg)?;
    }
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
    let _ = web::install_server(&cfg);
    let notes = daemon::reload(shared)?;
    if restart_php { shared.lock().unwrap().restart("php"); }
    if restart_web { shared.lock().unwrap().restart("web"); }
    Ok(json!({ "ok": true, "notes": notes }))
}

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// Where "start at sign-in" lives on macOS (a LaunchAgent) and Linux (an XDG autostart entry).
fn autostart_file() -> PathBuf {
    let home = paths::user_home();
    if cfg!(target_os = "macos") { home.join("Library").join("LaunchAgents").join("dev.tez.plist") }
    else { home.join(".config").join("autostart").join("tez.desktop") }
}

pub fn autostart_enabled() -> bool {
    if !cfg!(windows) { return autostart_file().exists(); }
    Command::new(paths::system32("reg.exe")).args(["query", RUN_KEY, "/v", "tez"])
        .hidden().stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

pub fn set_autostart(on: bool) -> R<()> {
    let exe = paths::bin().join(sys::exe("tez"));
    if !cfg!(windows) {
        let file = autostart_file();
        if !on { let _ = std::fs::remove_file(&file); return Ok(()); }
        if let Some(d) = file.parent() { std::fs::create_dir_all(d)?; }
        let tez = exe.display().to_string();
        let text = if cfg!(target_os = "macos") {
            let esc = tez.replace('&', "&amp;").replace('<', "&lt;");
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>Label</key><string>dev.tez</string>\n\t<key>ProgramArguments</key>\n\t<array><string>{esc}</string><string>start</string><string>--quiet</string></array>\n\t<key>RunAtLoad</key><true/>\n</dict>\n</plist>\n")
        } else {
            format!("[Desktop Entry]\nType=Application\nName=tez\nComment=Start tez's Laravel services\nExec=\"{tez}\" start --quiet\nX-GNOME-Autostart-enabled=true\nTerminal=false\n")
        };
        std::fs::write(&file, text)?;
        return Ok(());
    }
    let st = if on {
        Command::new(paths::system32("reg.exe")).args(["add", RUN_KEY, "/v", "tez", "/t", "REG_SZ", "/f", "/d", &format!("\"{}\" start --quiet", exe.display())])
            .hidden().stdout(Stdio::null()).status()?
    } else {
        Command::new(paths::system32("reg.exe")).args(["delete", RUN_KEY, "/v", "tez", "/f"])
            .hidden().stdout(Stdio::null()).stderr(Stdio::null()).status()?
    };
    if on && !st.success() { bail!("could not register tez to start at sign-in"); }
    Ok(())
}

/// Where `tez trust` puts the local CA on Linux (Debian/Ubuntu, then Fedora/RHEL layout).
pub const LINUX_CA_FILES: &[(&str, &str)] = &[
    ("/usr/local/share/ca-certificates/tez-local-ca.crt", "update-ca-certificates"),
    ("/etc/pki/ca-trust/source/anchors/tez-local-ca.crt", "update-ca-trust"),
];

pub fn ca_trusted() -> bool {
    let root = paths::local_ca_root();
    if !root.exists() { return false; }
    if cfg!(target_os = "macos") {
        return Command::new("security").args(["verify-cert", "-c"]).arg(&root).stdout(Stdio::null()).stderr(Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false);
    }
    if !cfg!(windows) {
        let ours = std::fs::read(&root).unwrap_or_default();
        return LINUX_CA_FILES.iter().any(|(f, _)| std::fs::read(f).map(|t| t == ours).unwrap_or(false));
    }
    let out = Command::new(paths::system32("certutil.exe")).args(["-verify"]).arg(&root)
        .hidden().output();
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
    // `extensions_switchable`: false for static builds (macOS/Linux), where every extension is
    // compiled in and always on, and PECL installs are not possible.
    json!({ "versions": versions, "catalog": catalog, "all": cfg.php_ini, "available": php::available_minors(),
            "extensions_switchable": php::LOADABLE_EXTENSIONS, "pecl": php::LOADABLE_EXTENSIONS })
}

/// Every extension PECL publishes Windows builds for (cached for a day).
pub fn pecl_catalog() -> R<Value> {
    if !php::LOADABLE_EXTENSIONS { return Ok(json!({ "extensions": [], "note": "static PHP builds include their extensions; PECL add-ons are Windows-only for now" })); }
    let cache = paths::cache_dir().join("pecl-index.json");
    let fresh = cache.metadata().and_then(|m| m.modified()).map(|t| t.elapsed().map(|e| e.as_secs() < 86_400).unwrap_or(false)).unwrap_or(false);
    if fresh { if let Ok(t) = std::fs::read_to_string(&cache) { if let Ok(v) = serde_json::from_str(&t) { return Ok(v); } } }
    let html = crate::fetch::get_text("https://downloads.php.net/~windows/pecl/releases/")?;
    let mut names: Vec<String> = crate::fetch::hrefs(&html).into_iter()
        .filter_map(|h| h.strip_suffix('/').map(|s| s.to_string()))
        .filter(|s| !s.is_empty() && !s.starts_with('.') && !s.contains('/') && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .collect();
    names.sort();
    names.dedup();
    let v = json!({ "extensions": names });
    let _ = std::fs::write(&cache, v.to_string());
    Ok(v)
}

/// POST /api/php { "install_version": "8.3.27" }: install one exact PHP patch as a job.
pub fn php_install_version(body: &Value) -> R<Value> {
    let v = body["install_version"].as_str().unwrap_or_default().trim().to_string();
    if v.split('.').count() != 3 || !v.chars().all(|c| c.is_ascii_digit() || c == '.') { bail!("give a full PHP version such as 8.3.27"); }
    php::normalize(&v)?;
    let id = start_job(&["php".into(), "install".into(), v.clone()], None, &format!("Install PHP {v}"))?;
    Ok(json!({ "ok": true, "job": id }))
}

/// body: { "settings": { key: value | null }, "extensions": { name: true|false },
///         "install": "imagick", "version": "3.8.0"?, "source": true? }
pub fn php_set(shared: &Shared, minor: &str, body: &Value) -> R<Value> {
    if let Some(ext) = body["install"].as_str() {
        if !ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') { bail!("bad extension name"); }
        let target = if minor == "all" { "all".to_string() } else { php::normalize(minor)? };
        let mut args: Vec<String> = vec!["ext".into(), "install".into(), ext.into(), "--php".into(), target.clone()];
        let version = body["version"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        if let Some(v) = &version {
            if !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') { bail!("bad extension version"); }
            args.push("--version".into());
            args.push(v.clone());
        }
        if body["source"].as_bool() == Some(true) { args.push("--source".into()); }
        let title = format!("Install {ext}{} for PHP {target}", version.map(|v| format!(" {v}")).unwrap_or_default());
        let id = start_job(&args, None, &title)?;
        return Ok(json!({ "ok": true, "job": id }));
    }
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
        if !php::LOADABLE_EXTENSIONS && !map.is_empty() { bail!("static PHP builds on {} have every extension compiled in and always on", sys::os()); }
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
    let _ = web::install_server(&cfg);
    // php -v with the new ini catches mistakes before the workers restart.
    let check_minor = if all { cfg.default_php.clone() } else { minor.clone() };
    let check = Command::new(php::php_exe(&check_minor)).arg("-v").hidden().output()?;
    let warnings = String::from_utf8_lossy(&check.stderr).trim().to_string() + &String::from_utf8_lossy(&check.stdout).lines().filter(|l| l.contains("Warning")).collect::<Vec<_>>().join("\n");
    let mut d = shared.lock().unwrap();
    if all { d.restart("php"); } else { d.restart(&format!("php-{minor}")); }
    drop(d);
    Ok(json!({ "ok": true, "warnings": warnings }))
}

// ------------------------------------------------------------------ services

/// Every configured instance, plus one row for each service not set up yet.
pub fn service_instances(cfg: &Config) -> Vec<Value> {
    let mut out = Vec::new();
    for i in services::CATALOG {
        let mut keys: Vec<String> = cfg.services.keys().filter(|k| services::kind(k) == i.name).cloned().collect();
        let configured = !keys.is_empty();
        if !configured { keys.push(i.name.to_string()); }
        for key in keys {
            let sc = cfg.services.get(&key).cloned().unwrap_or_default();
            let version = services::version_of(&key, &sc);
            let port = services::port_of(&key, &sc);
            let full = std::fs::read_to_string(services::dir(&key, &version).join(".tez-version")).unwrap_or_else(|_| version.clone());
            let options: Vec<Value> = services::options_for(&key).iter().map(|(k, d, about)| json!({
                "key": k, "default": d, "about": about, "value": sc.options.get(*k).cloned().unwrap_or_else(|| d.to_string()),
                "secret": k.contains("key") || k.contains("password"),
            })).collect();
            let title = if key.contains('@') { format!("{} {}", i.title, services::series(&key, &version)) } else { i.title.to_string() };
            let avail = services::available(&key);
            let mut v = json!({
                "name": key, "kind": i.name, "title": title, "about": i.about, "version": version, "full_version": full.trim(), "port": port,
                "installed": services::is_installed(&key, &version), "enabled": configured && sc.enabled,
                "options": options, "env": env_snippet(&key, port, &sc), "versions": i.versions,
                "available": avail.is_ok(), "unavailable_reason": avail.err(),
            });
            if i.name == "postgres" {
                v["extensions"] = json!(services::pg_extensions_installed(&services::series("postgres", &version)));
                v["extension_catalog"] = json!(if services::pg_extensions_available() { services::PG_EXTENSIONS } else { &[] });
            }
            out.push(v);
        }
    }
    out
}

pub fn services_get() -> Value {
    let cfg = Config::load();
    json!({ "services": service_instances(&cfg) })
}

fn env_snippet(name: &str, port: u16, sc: &services_cfg::ServiceConfig) -> Vec<(String, String)> {
    let p = port.to_string();
    let v = |a: &str, b: &str| (a.to_string(), b.to_string());
    match services::kind(name) {
        "mysql" | "mariadb" => vec![v("DB_CONNECTION", if services::kind(name) == "mariadb" { "mariadb" } else { "mysql" }), v("DB_HOST", "127.0.0.1"), v("DB_PORT", &p), v("DB_USERNAME", "root"), v("DB_PASSWORD", "")],
        "mongodb" => vec![v("MONGODB_URI", &format!("mongodb://127.0.0.1:{p}"))],
        "postgres" => vec![v("DB_CONNECTION", "pgsql"), v("DB_HOST", &services::pg_host()), v("DB_PORT", &p), v("DB_USERNAME", "postgres"), v("DB_PASSWORD", "")],
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
    // Installing a version or an extension takes a while: run it as a job with a log.
    if let Some(v) = body["install"].as_str() {
        let id = start_job(&["service".into(), "install".into(), services::kind(name).into(), v.into()], None, &format!("Install {} {v}", services::info(name)?.title))?;
        return Ok(json!({ "ok": true, "job": id }));
    }
    if let Some(ext) = body["extension"].as_str() {
        let cfg = Config::load();
        let pg = services::version_of(name, &cfg.services.get(name).cloned().unwrap_or_default());
        let mut args: Vec<String> = vec!["pg".into(), "ext".into(), "install".into(), ext.into()];
        if let Some(v) = body["extension_version"].as_str().filter(|s| !s.is_empty()) { args.push(v.into()); }
        args.push("--pg".into());
        args.push(pg.clone());
        let id = start_job(&args, None, &format!("Add {ext} to PostgreSQL {pg}"))?;
        return Ok(json!({ "ok": true, "job": id }));
    }
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
    // What this project would run with no switches set: those stay as cards when turned off.
    let suggested = { let mut plain = s.clone(); plain.settings.processes.clear(); project::detect(&plain) };
    let procs: Vec<Value> = KNOWN_PROCS.iter().map(|(n, about)| {
        let d = detected.iter().find(|p| p.name == *n);
        let sug = suggested.iter().any(|p| p.name == *n);
        json!({ "name": n, "about": about, "on": d.is_some(), "suggested": sug, "reason": d.map(|p| p.reason.clone()), "command": d.map(|p| p.command.join(" ").replace("@php", "php")) })
    }).chain(detected.iter().filter(|p| !KNOWN_PROCS.iter().any(|(n, _)| *n == p.name)).map(|p| json!({
        "name": p.name, "about": "custom process from tez.json", "on": true, "suggested": true, "reason": p.reason, "command": p.command.join(" ").replace("@php", "php") })))
        .collect();
    let files: Vec<String> = std::fs::read_dir(s.path.join("config")).map(|rd| {
        let mut v: Vec<String> = rd.flatten().filter_map(|e| e.file_name().to_str().map(|x| x.to_string())).filter(|n| n.ends_with(".php")).collect();
        v.sort(); v
    }).unwrap_or_default();
    Ok(json!({
        "name": s.name, "path": s.path, "php": s.php, "isolated": s.settings.php.is_some(), "up": s.settings.up, "domains": s.settings.domains,
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
    if let Some(list) = body["domains"].as_array() {
        let mut ds: Vec<String> = Vec::new();
        for d in list.iter().filter_map(|x| x.as_str()) {
            let d = d.trim().to_ascii_lowercase();
            if d.is_empty() { continue; }
            if d.contains(['/', ' ', ',', '{', '}']) { bail!("'{d}' is not a host name"); }
            if !ds.contains(&d) { ds.push(d); }
        }
        entry.domains = ds;
    }
    if let Some(map) = body["processes"].as_object() {
        for (k, v) in map {
            match v.as_bool() { Some(b) => { entry.processes.insert(k.clone(), b); } None => { entry.processes.remove(k); } }
        }
    }
    let wants_php = entry.php.clone();
    cfg.save()?;
    if let Some(v) = wants_php {
        if !php::is_installed(&v) {
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
    Ok(paths::join_rel(&s.path, &rel))
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
        let out = Command::new(php::php_exe(&s.php)).args(["-l"]).arg(&p).hidden().output()?;
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
        .current_dir(&s.path).hidden().output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let start = text.find(['{', '[']).unwrap_or(0);
    Ok(serde_json::from_str(text[start..].trim()).unwrap_or(json!({ "error": text.trim() })))
}

// ------------------------------------------------------------------ jobs

pub struct JobRec { pub id: u32, pub title: String, pub child: Option<Child>, pub state: String, pub log: PathBuf, pub started: String }
static JOBS: Mutex<Vec<JobRec>> = Mutex::new(Vec::new());

/// Run a tez command in the background with its output in a log. Only tez's own commands.
pub fn start_job(args: &[String], cwd: Option<&Path>, title: &str) -> R<u32> {
    const ALLOWED: &[&str] = &["php", "service", "pg", "ext", "clone", "setup", "db", "adopt", "import", "composer", "npm", "artisan", "doctor", "trust", "hosts"];
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
        "composer" => (bin.join(sys::exe("composer")), args[1..].to_vec()),
        "artisan" => (bin.join(sys::exe("php")), std::iter::once("artisan".to_string()).chain(args[1..].iter().cloned()).collect()),
        "npm" if cfg!(windows) => {
            let node_dir = which_dir("node.exe").ok_or("Node.js is not installed")?;
            (node_dir.join("node.exe"), std::iter::once(node_dir.join("node_modules\\npm\\bin\\npm-cli.js").display().to_string()).chain(args[1..].iter().cloned()).collect())
        }
        "npm" => (which_dir("npm").ok_or("Node.js is not installed")?.join("npm"), args[1..].to_vec()),
        _ => (bin.join(sys::exe("tez")), args.to_vec()),
    };
    let child = Command::new(exe).args(rest)
        .current_dir(cwd.map(|p| p.to_path_buf()).unwrap_or_else(paths::home))
        .env("PATH", project::child_path())
        .env("NO_COLOR", "1")
        .stdin(Stdio::null()).stdout(Stdio::from(f.try_clone()?)).stderr(Stdio::from(f))
        .hidden()
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

/// One gh login (gh supports several accounts per host).
#[derive(Clone, Debug, serde::Serialize)]
pub struct GhAccount { pub host: String, pub login: String, pub active: bool, pub protocol: String }

pub fn gh_accounts() -> Vec<GhAccount> {
    let out = Command::new("gh").args(["auth", "status"]).hidden().output();
    let Ok(out) = out else { return Vec::new() };
    // gh prints this on stdout or stderr depending on version.
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    let mut accounts: Vec<GhAccount> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.split_once("Logged in to ").map(|(_, r)| r) {
            let mut parts = rest.split_whitespace();
            let host = parts.next().unwrap_or("github.com").to_string();
            let login = rest.split(" account ").nth(1).and_then(|s| s.split_whitespace().next()).unwrap_or("").to_string();
            accounts.push(GhAccount { host, login, active: false, protocol: "https".into() });
        } else if let Some(v) = t.strip_prefix("- Active account:") {
            if let Some(a) = accounts.last_mut() { a.active = v.trim() == "true"; }
        } else if let Some(v) = t.strip_prefix("- Git operations protocol:") {
            if let Some(a) = accounts.last_mut() { a.protocol = v.trim().to_string(); }
        }
    }
    accounts
}

/// A `Host` entry from ~/.ssh/config: an alias git can clone through (`git@alias:owner/repo`).
#[derive(Clone, Debug, serde::Serialize)]
pub struct SshHost { pub alias: String, pub hostname: String, pub user: String, pub identity: String, pub key_exists: bool }

pub fn ssh_hosts() -> Vec<SshHost> {
    let home = paths::user_home();
    let text = std::fs::read_to_string(home.join(".ssh").join("config")).unwrap_or_default();
    let mut hosts: Vec<SshHost> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') || t.is_empty() { continue; }
        let (k, v) = match t.split_once(|c: char| c.is_whitespace() || c == '=') { Some((k, v)) => (k.to_ascii_lowercase(), v.trim().trim_start_matches('=').trim().to_string()), None => continue };
        match k.as_str() {
            "host" => {
                for alias in v.split_whitespace().filter(|a| !a.contains('*') && !a.contains('?') && !a.starts_with('!')) {
                    hosts.push(SshHost { alias: alias.to_string(), hostname: alias.to_string(), user: "git".into(), identity: String::new(), key_exists: false });
                }
            }
            "hostname" => if let Some(h) = hosts.last_mut() { h.hostname = v; },
            "user" => if let Some(h) = hosts.last_mut() { h.user = v; },
            "identityfile" => if let Some(h) = hosts.last_mut() {
                let p = v.trim_matches('"').replacen("~", &home.display().to_string(), 1);
                let p = if cfg!(windows) { p.replace('/', "\\") } else { p };
                h.key_exists = Path::new(&p).exists();
                h.identity = p;
            },
            _ => {}
        }
    }
    // github.com itself, with the default key, when the config does not name it.
    if !hosts.iter().any(|h| h.alias == "github.com") {
        let key = ["id_ed25519", "id_ecdsa", "id_rsa"].iter().map(|k| home.join(".ssh").join(k)).find(|p| p.exists());
        if let Some(k) = key {
            hosts.insert(0, SshHost { alias: "github.com".into(), hostname: "github.com".into(), user: "git".into(), identity: k.display().to_string(), key_exists: true });
        }
    }
    hosts
}

/// The clone route for an account: its own SSH alias when one is configured (the alias name
/// mentions the login, as in github.com-<login>), else plain github.com over SSH or gh's HTTPS.
pub fn route_for(account: &GhAccount, hosts: &[SshHost]) -> String {
    if account.protocol == "ssh" {
        // Whole parts only: "github.com-acme" names acme, not acmecorp.
        let names_login = |alias: &str| alias.split(|c: char| c == '-' || c == '_' || c == '.' || c == '+').any(|part| part.eq_ignore_ascii_case(&account.login));
        if let Some(h) = hosts.iter().find(|h| h.hostname == account.host && h.alias != account.host && names_login(&h.alias)) {
            return format!("ssh:{}", h.alias);
        }
        // The default key belongs to the account no alias claims.
        if hosts.iter().any(|h| h.alias == account.host) { return format!("ssh:{}", account.host); }
    }
    format!("gh:{}", account.login)
}

pub fn identities() -> Value {
    let accounts = gh_accounts();
    let hosts = ssh_hosts();
    let home = paths::user_home();
    let keys: Vec<String> = std::fs::read_dir(home.join(".ssh")).map(|rd| rd.flatten()
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .filter(|n| n.ends_with(".pub")).collect()).unwrap_or_default();
    let routes: Vec<Value> = accounts.iter().map(|a| json!({ "login": a.login, "route": route_for(a, &hosts) })).collect();
    json!({ "accounts": accounts, "ssh_hosts": hosts, "public_keys": keys, "routes": routes })
}

/// gh for one specific account: its token is handed to gh through GH_TOKEN, never to the UI.
fn gh_as(account: Option<&str>, args: &[&str]) -> R<String> {
    let token = match account {
        Some(login) if !login.is_empty() => {
            if !login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') { bail!("bad account name"); }
            Some(gh(&["auth", "token", "--hostname", "github.com", "--user", login], None)?.trim().to_string())
        }
        _ => None,
    };
    let mut cmd = Command::new("gh");
    cmd.args(args).hidden().stdin(Stdio::null());
    if let Some(t) = token { cmd.env("GH_TOKEN", t); }
    let out = cmd.output().map_err(|_| gh_missing())?;
    if !out.status.success() { bail!("{}", String::from_utf8_lossy(&out.stderr).trim()); }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn gh_missing() -> String {
    let how = match sys::os() { "windows" => "winget install GitHub.cli", "macos" => "brew install gh", _ => "see https://cli.github.com" };
    format!("GitHub CLI (gh) is not installed: {how}")
}

fn gh(args: &[&str], stdin: Option<&str>) -> R<String> {
    let mut cmd = Command::new("gh");
    cmd.args(args).hidden().stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin.is_some() { cmd.stdin(Stdio::piped()); } else { cmd.stdin(Stdio::null()); }
    let mut child = cmd.spawn().map_err(|_| gh_missing())?;
    if let Some(input) = stdin {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(input.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() { bail!("{}", String::from_utf8_lossy(&out.stderr).trim()); }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The signed-in user and the organisations they can see, for one gh account (or the active one).
pub fn github_status(account: Option<&str>) -> Value {
    match gh_as(account, &["api", "user", "--jq", ".login"]) {
        Ok(login) => {
            let orgs: Vec<String> = gh_as(account, &["api", "user/orgs", "--paginate", "--jq", ".[].login"]).unwrap_or_default()
                .lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            json!({ "ok": true, "login": login.trim(), "owners": std::iter::once(login.trim().to_string()).chain(orgs).collect::<Vec<_>>(), "identities": identities() })
        }
        Err(e) => json!({ "ok": false, "error": e.to_string(), "identities": identities() }),
    }
}

pub fn github_repos(owner: &str, account: Option<&str>) -> R<Value> {
    if !owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') { bail!("bad owner"); }
    let out = gh_as(account, &["repo", "list", owner, "--limit", "300", "--json", "name,nameWithOwner,description,isPrivate,updatedAt,primaryLanguage,isArchived,sshUrl,url"])?;
    let v: Value = serde_json::from_str(&out)?;
    Ok(json!({ "repos": v }))
}

/// The token goes straight to gh's own credential store (Windows Credential Manager, the macOS
/// Keychain, or the Linux keyring); tez never keeps it.
pub fn github_login(token: &str) -> R<Value> {
    let t = token.trim();
    if t.is_empty() || t.contains(char::is_whitespace) { bail!("paste a GitHub token (it starts with ghp_ or github_pat_)"); }
    gh(&["auth", "login", "--with-token", "--hostname", "github.com"], Some(t))?;
    Ok(github_status(None))
}
