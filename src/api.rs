//! The daemon's HTTP API and dashboard, on 127.0.0.1 only, written on std::net (no framework).
//! The CLI talks to the daemon through the same API.

use crate::config::Config;
use crate::daemon::{self, Shared, State};
use crate::manage;
use crate::paths;
use crate::php;
use crate::project;
use crate::services;
use crate::sites;
use crate::util::{self, R};
use crate::web;
use serde_json::json;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

const DASHBOARD: &str = include_str!("../assets/dashboard.html");

struct Req { method: String, path: String, query: String, body: Vec<u8>, host: String, tez_header: bool }

fn read_req(stream: &mut TcpStream) -> Option<Req> {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut len = 0usize;
    let mut host = String::new();
    let mut tez_header = false;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 { break; }
        let h = h.trim_end();
        if h.is_empty() { break; }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") { len = v.trim().parse().unwrap_or(0); }
            if k.eq_ignore_ascii_case("host") { host = v.trim().to_ascii_lowercase(); }
            if k.eq_ignore_ascii_case("x-tez") { tez_header = true; }
        }
    }
    let mut body = vec![0; len.min(1 << 20)];
    if len > 0 { reader.read_exact(&mut body).ok()?; }
    let (path, query) = match target.split_once('?') { Some((p, q)) => (p.to_string(), q.to_string()), None => (target, String::new()) };
    Some(Req { method, path, query, body, host, tez_header })
}

/// Only our own names may reach the API (blocks DNS rebinding), and every change must carry an
/// `X-Tez` header, which a web page on another origin cannot add without a CORS preflight we never answer.
fn allowed(req: &Req, tld: &str) -> bool {
    let name = req.host.rsplit_once(':').map(|(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { req.host.as_str() }).unwrap_or(&req.host);
    let host_ok = matches!(name, "127.0.0.1" | "localhost" | "[::1]" | "tez.localhost") || name == format!("tez.{tld}");
    host_ok && (req.method == "GET" || req.tez_header)
}

fn respond(stream: &mut TcpStream, status: u16, ctype: &str, body: &[u8]) {
    let reason = match status { 200 => "OK", 400 => "Bad Request", 404 => "Not Found", 403 => "Forbidden", _ => "Error" };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

fn json_ok(stream: &mut TcpStream, v: serde_json::Value) { respond(stream, 200, "application/json", v.to_string().as_bytes()); }
fn json_err(stream: &mut TcpStream, status: u16, msg: &str) { respond(stream, status, "application/json", json!({ "error": msg }).to_string().as_bytes()); }

fn body_json(req: &Req) -> R<serde_json::Value> {
    if req.body.is_empty() { return Ok(json!({})); }
    Ok(serde_json::from_slice(&req.body).map_err(|e| format!("bad JSON: {e}"))?)
}

fn reply(stream: &mut TcpStream, r: R<serde_json::Value>) {
    match r { Ok(v) => json_ok(stream, v), Err(e) => json_err(stream, 400, &e.to_string()) }
}

/// Percent-decoding for query values.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) { out.push(v); i += 3; continue; }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn query_param(q: &str, key: &str) -> Option<String> {
    q.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v.to_string()))
}

/// POST /api/wsl/shutdown: stop WSL and give its memory back (Windows only).
fn wsl_shutdown(stream: &mut TcpStream) {
    #[cfg(windows)]
    match crate::sys::wsl_shutdown() {
        Ok(()) => json_ok(stream, json!({ "ok": true })),
        Err(e) => json_err(stream, 500, &e.to_string()),
    }
    #[cfg(not(windows))]
    json_err(stream, 400, "WSL exists only on Windows");
}

pub fn status_json(shared: &Shared) -> serde_json::Value {
    let cfg = Config::load();
    let d = shared.lock().unwrap();
    let procs: Vec<_> = d.procs.iter().map(|p| json!({
        "name": p.spec.name, "group": p.spec.group, "state": p.state, "pid": p.pid(),
        "restarts": p.restarts, "uptime": p.uptime(), "port": p.spec.port, "url": p.spec.url,
        "memory": p.memory(), "cpu": crate::metrics::cpu_of(&p.spec.name), "last_exit": p.last_exit, "wanted": p.wanted,
    })).collect();
    let total_mem: u64 = d.procs.iter().map(|p| p.memory()).sum();
    let notes = d.notes.clone();
    drop(d);
    let (http, https, _) = web::effective_ports(&cfg);
    let site_list: Vec<_> = sites::all(&cfg).into_iter().map(|s| {
        let procs = project::summary(&s);
        json!({
            "name": s.name, "path": s.path, "php": s.php, "url": s.url(&cfg), "linked": s.linked,
            "laravel": s.laravel, "up": s.settings.up, "isolated": s.settings.php.is_some(),
            "processes": procs, "localhost": format!("https://{}.localhost{}", s.name, if https == 443 { String::new() } else { format!(":{https}") }),
        })
    }).collect();
    let php_list: Vec<_> = php::installed().into_iter().map(|(m, v)| json!({ "minor": m, "version": v, "default": m == cfg.default_php })).collect();
    let svc_list = crate::manage::service_instances(&cfg);
    json!({
        "tld": cfg.tld, "http_port": http, "https_port": https, "web_server": web::server(&cfg), "procs": procs, "sites": site_list,
        "php": php_list, "services": svc_list, "notes": notes, "memory": total_mem, "home": paths::home(),
        "xdebug": cfg.xdebug, "version": env!("CARGO_PKG_VERSION"),
    })
}

fn handle(mut stream: TcpStream, shared: Shared) -> bool {
    let Some(req) = read_req(&mut stream) else { return false };
    if !allowed(&req, &Config::load().tld) {
        json_err(&mut stream, 403, "forbidden");
        return false;
    }
    let segs: Vec<&str> = req.path.trim_matches('/').split('/').collect();
    match (req.method.as_str(), segs.as_slice()) {
        ("GET", [""]) | ("GET", ["index.html"]) => respond(&mut stream, 200, "text/html; charset=utf-8", DASHBOARD.as_bytes()),
        ("GET", ["api", "status"]) => json_ok(&mut stream, status_json(&shared)),
        ("GET", ["api", "ping"]) => json_ok(&mut stream, json!({ "ok": true, "pid": std::process::id() })),
        ("POST", ["api", "reload"]) => match daemon::reload(&shared) {
            Ok(notes) => json_ok(&mut stream, json!({ "ok": true, "notes": notes })),
            Err(e) => json_err(&mut stream, 500, &e.to_string()),
        },
        ("POST", ["api", "proc", name @ .., action]) => {
            let name = name.join("/");
            let mut d = shared.lock().unwrap();
            let found = match *action {
                "restart" => d.restart(&name),
                "stop" => d.set_wanted(&name, false),
                "start" => d.set_wanted(&name, true),
                _ => false,
            };
            drop(d);
            if found { json_ok(&mut stream, json!({ "ok": true })) } else { json_err(&mut stream, 404, "no such process") }
        }
        ("POST", ["api", "site", name, action @ ("up" | "down")]) => {
            let mut cfg = Config::load();
            if sites::find(&cfg, name).is_none() { json_err(&mut stream, 404, "no such site"); return false; }
            match *action {
                "up" => cfg.site_mut(name).up = true,
                "down" => cfg.site_mut(name).up = false,
                _ => { json_err(&mut stream, 400, "unknown action"); return false; }
            }
            if let Err(e) = cfg.save() { json_err(&mut stream, 500, &e.to_string()); return false; }
            match daemon::reload(&shared) {
                Ok(_) => json_ok(&mut stream, json!({ "ok": true })),
                Err(e) => json_err(&mut stream, 500, &e.to_string()),
            }
        }
        // ---- removing a site (see remove.rs)
        ("GET", ["api", "site", name, "delete-preview"]) => reply(&mut stream, crate::remove::preview(&Config::load(), Some(&shared), name)),
        ("POST", ["api", "site", name, "delete"]) => reply(&mut stream, body_json(&req).and_then(|b| crate::remove::delete(Some(&shared), name, &b))),
        ("GET", ["api", "sites", "hidden"]) => json_ok(&mut stream, crate::remove::hidden_list(&Config::load())),
        ("POST", ["api", "sites", "unhide"]) => reply(&mut stream, body_json(&req).and_then(|b| crate::remove::unhide(Some(&shared), b["path"].as_str().unwrap_or_default()))),
        ("GET", ["api", "logs", name @ ..]) => {
            let n = query_param(&req.query, "n").and_then(|v| v.parse().ok()).unwrap_or(200usize);
            let name = name.join("/");
            let text = util::tail(&daemon::log_path(&name), n);
            respond(&mut stream, 200, "text/plain; charset=utf-8", text.as_bytes());
        }
        ("GET", ["api", "sitelog", name]) => {
            let cfg = Config::load();
            let n = query_param(&req.query, "n").and_then(|v| v.parse().ok()).unwrap_or(200usize);
            let text = match sites::find(&cfg, name) {
                Some(s) => latest_laravel_log(&s.path).map(|p| util::tail(&p, n)).unwrap_or_else(|| "No log yet.".into()),
                None => "No such site.".into(),
            };
            respond(&mut stream, 200, "text/plain; charset=utf-8", text.as_bytes());
        }
        ("GET", ["api", "dumps"]) => {
            let n = query_param(&req.query, "n").and_then(|v| v.parse().ok()).unwrap_or(100usize);
            let text = util::tail(&paths::logs_dir().join("dumps.jsonl"), n);
            respond(&mut stream, 200, "application/x-ndjson; charset=utf-8", text.as_bytes());
        }
        ("POST", ["api", "dumps"]) => {
            // Each dump is one JSON line from lib/php/dumps.php.
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(paths::logs_dir().join("dumps.jsonl")) {
                let mut line = req.body.clone();
                line.retain(|b| *b != b'\n' && *b != b'\r');
                line.push(b'\n');
                let _ = f.write_all(&line);
            }
            json_ok(&mut stream, json!({ "ok": true }));
        }
        ("DELETE", ["api", "dumps"]) => {
            let _ = std::fs::write(paths::logs_dir().join("dumps.jsonl"), b"");
            json_ok(&mut stream, json!({ "ok": true }));
        }
        // ---- resource and site metrics (see metrics.rs and stats.rs)
        ("GET", ["api", "metrics"]) => json_ok(&mut stream, crate::metrics::json(&query_param(&req.query, "range").unwrap_or_default())),
        ("GET", ["api", "sites", "metrics"]) => json_ok(&mut stream, crate::stats::sites_json(&query_param(&req.query, "range").unwrap_or_default())),
        ("GET", ["api", "services", "metrics"]) => json_ok(&mut stream, crate::stats::services_json()),
        ("POST", ["api", "cache", "clear"]) => json_ok(&mut stream, json!({ "ok": true, "freed": crate::metrics::clear_cache() })),
        ("POST", ["api", "wsl", "shutdown"]) => wsl_shutdown(&mut stream),
        // ---- settings, jobs, GitHub (see manage.rs)
        ("GET", ["api", "settings"]) => json_ok(&mut stream, manage::settings_get()),
        ("POST", ["api", "settings"]) => reply(&mut stream, body_json(&req).and_then(|b| manage::settings_set(&shared, &b))),
        ("GET", ["api", "php"]) => json_ok(&mut stream, manage::php_get()),
        ("GET", ["api", "php", "pecl"]) => reply(&mut stream, manage::pecl_catalog()),
        ("GET", ["api", "php", minor, "pecl", ext]) => reply(&mut stream, crate::catalog::pecl_versions(minor, ext)),
        ("POST", ["api", "php"]) => reply(&mut stream, body_json(&req).and_then(|b| manage::php_install_version(&b))),
        ("GET", ["api", "catalog"]) => json_ok(&mut stream, crate::catalog::get(query_param(&req.query, "refresh").is_some())),
        ("POST", ["api", "php", minor]) => reply(&mut stream, body_json(&req).and_then(|b| manage::php_set(&shared, minor, &b))),
        ("GET", ["api", "node"]) => json_ok(&mut stream, crate::node::api_get()),
        ("POST", ["api", "node"]) => reply(&mut stream, body_json(&req).and_then(|b| crate::node::api_set(&shared, &b))),
        ("GET", ["api", "services"]) => json_ok(&mut stream, manage::services_get()),
        ("POST", ["api", "service", name]) => reply(&mut stream, body_json(&req).and_then(|b| manage::service_set(&shared, name, &b))),
        ("GET", ["api", "site", name]) => reply(&mut stream, manage::site_get(name)),
        ("POST", ["api", "site", name]) => reply(&mut stream, body_json(&req).and_then(|b| manage::site_set(&shared, name, &b))),
        ("POST", ["api", "site", name, "env"]) => reply(&mut stream, body_json(&req).and_then(|b| manage::env_set(name, &b))),
        ("GET", ["api", "site", name, "file"]) => {
            let rel = query_param(&req.query, "path").map(|p| decode(&p)).unwrap_or_default();
            match manage::file_get(name, &rel) {
                Ok(t) => respond(&mut stream, 200, "text/plain; charset=utf-8", t.as_bytes()),
                Err(e) => json_err(&mut stream, 400, &e.to_string()),
            }
        }
        ("POST", ["api", "site", name, "file"]) => reply(&mut stream, body_json(&req).and_then(|b| {
            manage::file_set(name, b["path"].as_str().unwrap_or_default(), b["content"].as_str().unwrap_or_default())
        })),
        ("GET", ["api", "site", name, "config", key]) => reply(&mut stream, manage::effective_config(name, key)),
        ("GET", ["api", "jobs"]) => json_ok(&mut stream, manage::jobs_get()),
        ("POST", ["api", "jobs"]) => reply(&mut stream, body_json(&req).and_then(|b| manage::job_start(&b))),
        ("GET", ["api", "jobs", id, "log"]) => {
            let text = manage::job_log(id.parse().unwrap_or(0));
            respond(&mut stream, 200, "text/plain; charset=utf-8", text.as_bytes());
        }
        ("GET", ["api", "github"]) => {
            let account = query_param(&req.query, "account");
            json_ok(&mut stream, manage::github_status(account.as_deref()))
        }
        ("GET", ["api", "github", "identities"]) => json_ok(&mut stream, manage::identities()),
        ("GET", ["api", "github", "repos", owner]) => {
            let account = query_param(&req.query, "account");
            reply(&mut stream, manage::github_repos(owner, account.as_deref()))
        }
        ("POST", ["api", "github", "login"]) => reply(&mut stream, body_json(&req).and_then(|b| manage::github_login(b["token"].as_str().unwrap_or_default()))),
        ("POST", ["api", "shutdown"]) => {
            json_ok(&mut stream, json!({ "ok": true }));
            return true;
        }
        _ => json_err(&mut stream, 404, "not found"),
    }
    false
}

fn latest_laravel_log(project: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = project.join("storage").join("logs");
    std::fs::read_dir(dir).ok()?.flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "log").unwrap_or(false))
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
}

/// Blocks until a shutdown request arrives.
pub fn serve(shared: Shared, port: u16) -> R<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("cannot listen on 127.0.0.1:{port}: {e}"))?;
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let accept_shared = shared.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let s = accept_shared.clone();
            let tx = tx.clone();
            std::thread::spawn(move || { if handle(stream, s) { let _ = tx.send(()); } });
        }
    });
    let _ = rx.recv();
    Ok(())
}

/// Call the running daemon. Returns the response body.
pub fn call(method: &str, path: &str) -> R<String> {
    let cfg = Config::load();
    call_port(cfg.api_port, method, path, Duration::from_secs(120))
}

/// Call the running daemon with a JSON body. Returns the response body.
pub fn call_json(method: &str, path: &str, body: &serde_json::Value) -> R<String> {
    let cfg = Config::load();
    call_body(cfg.api_port, method, path, &body.to_string(), Duration::from_secs(300))
}

pub fn call_port(port: u16, method: &str, path: &str, timeout: Duration) -> R<String> {
    call_body(port, method, path, "", timeout)
}

fn call_body(port: u16, method: &str, path: &str, body: &str, timeout: Duration) -> R<String> {
    let mut s = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(400))
        .map_err(|_| "tez is not running (start it with `tez start`)".to_string())?;
    s.set_read_timeout(Some(timeout))?;
    write!(s, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Tez: cli\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    if !head.starts_with("HTTP/1.1 200") {
        let msg = serde_json::from_str::<serde_json::Value>(body).ok().and_then(|v| v["error"].as_str().map(|s| s.to_string())).unwrap_or_else(|| body.to_string());
        crate::bail!("{msg}");
    }
    Ok(body.to_string())
}

pub fn running() -> bool { call_port(Config::load().api_port, "GET", "/api/ping", Duration::from_secs(2)).is_ok() }

pub fn wait_running(secs: u64) -> bool {
    let until = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < until {
        if running() { return true; }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

#[allow(dead_code)]
pub fn state_label(s: State) -> &'static str {
    match s { State::Starting => "starting", State::Running => "running", State::Backoff => "restarting", State::Stopping => "stopping", State::Stopped => "stopped", State::Failed => "failed" }
}
