//! A project's own processes, what Sail ran as extra containers: Horizon (or a queue worker),
//! Reverb, Vite and the scheduler. Detected from composer.json, package.json and .env, and
//! overridable in the project's `tez.json`:
//!
//! ```json
//! { "php": "8.4", "processes": { "horizon": true, "vite": true, "reverb": true, "schedule": false,
//!   "pulse": "php artisan pulse:check" } }
//! ```

use crate::config::Config;
use crate::paths;
use crate::php;
use crate::sites::Site;
use crate::spec::Spec;
use crate::util::{self, R};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, serde::Serialize)]
pub struct Process { pub name: String, pub command: Vec<String>, pub reason: String }

fn composer_requires(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path.join("composer.json")) else { return Vec::new() };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
    let mut out = Vec::new();
    for key in ["require", "require-dev"] {
        if let Some(m) = json[key].as_object() { out.extend(m.keys().cloned()); }
    }
    out
}

fn dev_script(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json["scripts"]["dev"].as_str().map(|s| s.to_string())
}

fn manifest(path: &Path) -> serde_json::Value {
    std::fs::read_to_string(path.join("tez.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(serde_json::Value::Null)
}

/// The processes a project wants, with the reason each was chosen.
pub fn detect(site: &Site) -> Vec<Process> {
    let path = &site.path;
    if !site.laravel { return Vec::new(); }
    let req = composer_requires(path);
    let env = util::read_env(&path.join(".env"));
    let has = |p: &str| req.iter().any(|r| r == p);
    let artisan = |args: &[&str]| -> Vec<String> {
        let mut v = vec!["@php".to_string(), "artisan".into()];
        v.extend(args.iter().map(|s| s.to_string()));
        v
    };
    let queue = env.get("QUEUE_CONNECTION").or(env.get("QUEUE_DRIVER")).cloned().unwrap_or_default();
    let broadcast = env.get("BROADCAST_CONNECTION").or(env.get("BROADCAST_DRIVER")).cloned().unwrap_or_default();
    let mut out: Vec<Process> = Vec::new();
    if has("laravel/horizon") && (queue == "redis" || queue.is_empty()) {
        out.push(Process { name: "horizon".into(), command: artisan(&["horizon"]), reason: "laravel/horizon is installed".into() });
    } else if !queue.is_empty() && queue != "sync" && queue != "null" {
        out.push(Process { name: "queue".into(), command: artisan(&["queue:work", "--tries=3"]), reason: format!("QUEUE_CONNECTION={queue}") });
    }
    if has("laravel/reverb") && broadcast == "reverb" {
        out.push(Process { name: "reverb".into(), command: artisan(&["reverb:start"]), reason: "BROADCAST_CONNECTION=reverb".into() });
    }
    if let Some(dev) = dev_script(path) {
        if dev.contains("vite") {
            out.push(Process { name: "vite".into(), command: vec!["npm".into(), "run".into(), "dev".into()], reason: "package.json has a vite dev script".into() });
        }
    }

    // tez.json overrides (and then the dashboard's per-site switches): true/false toggles a
    // detected or known process; a string adds a custom one.
    let m = manifest(path);
    let mut overrides: Vec<(String, serde_json::Value)> = m["processes"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
    for (k, v) in &site.settings.processes { overrides.push((k.clone(), serde_json::Value::Bool(*v))); }
    {
        for (name, val) in &overrides {
            let name = name.clone();
            let name = &name;
            match val {
                serde_json::Value::Bool(false) => out.retain(|p| &p.name != name),
                serde_json::Value::Bool(true) => {
                    if out.iter().any(|p| &p.name == name) { continue; }
                    let cmd = match name.as_str() {
                        "horizon" => artisan(&["horizon"]),
                        "queue" => artisan(&["queue:work", "--tries=3"]),
                        "reverb" => artisan(&["reverb:start"]),
                        "schedule" => artisan(&["schedule:work"]),
                        "vite" => vec!["npm".into(), "run".into(), "dev".into()],
                        "pulse" => artisan(&["pulse:check"]),
                        _ => continue,
                    };
                    out.push(Process { name: name.clone(), command: cmd, reason: "turned on in tez.json".into() });
                }
                serde_json::Value::String(cmd) => {
                    out.retain(|p| &p.name != name);
                    out.push(Process { name: name.clone(), command: split_command(cmd), reason: "defined in tez.json".into() });
                }
                _ => {}
            }
        }
    }
    out
}

fn split_command(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in cmd.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"') | (None, '\'') => quote = Some(c),
            (None, ' ') => { if !cur.is_empty() { out.push(std::mem::take(&mut cur)); } }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() { out.push(cur); }
    let mut out: Vec<String> = out.into_iter().map(|s| if s == "php" { "@php".to_string() } else { s }).collect();
    if out.is_empty() { out.push("@php".into()); }
    out
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts = [".exe", ".cmd", ".bat", ""];
    for dir in std::env::split_paths(&path) {
        for ext in exts {
            let p = dir.join(format!("{name}{ext}"));
            if p.is_file() { return Some(p); }
        }
    }
    None
}

/// PATH for child processes: tez's bin first, so `php` and `composer` resolve to tez's versions.
pub fn child_path() -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    format!("{};{current}", paths::bin().display())
}

pub fn specs(cfg: &Config, site: &Site) -> R<Vec<Spec>> {
    let php = php::php_exe(&site.php);
    let mut out = Vec::new();
    for p in detect(site) {
        let mut raw = false;
        let (exe, args): (PathBuf, Vec<String>) = if p.command[0] == "@php" {
            (php.clone(), p.command[1..].to_vec())
        } else {
            let found = which(&p.command[0]).ok_or_else(|| format!("{} is not on PATH", p.command[0]))?;
            let is_script = found.extension().map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat")).unwrap_or(false);
            let dir = found.parent().map(|d| d.to_path_buf()).unwrap_or_default();
            // npm and npx ship as .cmd wrappers around node; run node directly (no cmd.exe layer).
            let js = match p.command[0].as_str() {
                "npm" => Some(dir.join("node_modules").join("npm").join("bin").join("npm-cli.js")),
                "npx" => Some(dir.join("node_modules").join("npm").join("bin").join("npx-cli.js")),
                _ => None,
            };
            match js {
                Some(js) if is_script && js.exists() && dir.join("node.exe").exists() => {
                    let mut a = vec![js.display().to_string()];
                    a.extend(p.command[1..].iter().cloned());
                    (dir.join("node.exe"), a)
                }
                _ if is_script => {
                    // cmd.exe /s /c ""C:\path with spaces\tool.cmd" args": its own quoting rules.
                    raw = true;
                    let rest: Vec<String> = p.command[1..].iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() }).collect();
                    (paths::system32("cmd.exe"), vec!["/d".into(), "/s".into(), "/c".into(), format!("\"\"{}\" {}\"", found.display(), rest.join(" "))])
                }
                _ => (found, p.command[1..].to_vec()),
            }
        };
        let mut s = Spec::new(format!("{}/{}", site.name, p.name), format!("site:{}", site.name), exe)
            .args(args)
            .cwd(site.path.clone())
            .env("PATH", child_path())
            .env("TEZ_SITE", site.name.clone());
        s.raw = raw;
        if p.name == "horizon" { s = s.url(format!("{}/horizon", site.url(cfg))); }
        if p.name == "vite" { s = s.env("BROWSER", "none"); }
        out.push(s);
    }
    Ok(out)
}

/// Point a Sail/Docker .env at tez's native services. The original is kept as `.env.sail`.
pub fn adopt(cfg: &Config, site: &Site) -> R<Vec<String>> {
    let env_path = site.path.join(".env");
    if !env_path.exists() {
        let example = site.path.join(".env.example");
        if example.exists() { std::fs::copy(&example, &env_path)?; }
    }
    let env = util::read_env(&env_path);
    let backup = site.path.join(".env.sail");
    if !backup.exists() { std::fs::copy(&env_path, &backup)?; }
    let mut changes: Vec<(&str, String)> = Vec::new();
    let svc_port = |n: &str, d: u16| cfg.services.get(n).map(|s| if s.port == 0 { d } else { s.port }).unwrap_or(d);
    let set = |changes: &mut Vec<(&'static str, String)>, k: &'static str, v: String| {
        if env.get(k).map(|x| x != &v).unwrap_or(true) { changes.push((k, v)); }
    };
    set(&mut changes, "APP_URL", site.url(cfg));
    let conn = env.get("DB_CONNECTION").cloned().unwrap_or_default();
    if conn == "pgsql" {
        set(&mut changes, "DB_HOST", "127.0.0.1".into());
        set(&mut changes, "DB_PORT", svc_port("postgres", 5432).to_string());
    }
    if env.contains_key("REDIS_HOST") {
        set(&mut changes, "REDIS_HOST", "127.0.0.1".into());
        set(&mut changes, "REDIS_PORT", svc_port("redis", 6379).to_string());
    }
    if env.contains_key("MEILISEARCH_HOST") || env.get("SCOUT_DRIVER").map(|d| d == "meilisearch").unwrap_or(false) {
        set(&mut changes, "MEILISEARCH_HOST", format!("http://127.0.0.1:{}", svc_port("meilisearch", 7700)));
    }
    let mailer = env.get("MAIL_MAILER").cloned().unwrap_or_default();
    if mailer == "smtp" || env.get("MAIL_HOST").map(|h| h == "mailpit" || h == "mailhog").unwrap_or(false) {
        set(&mut changes, "MAIL_HOST", "127.0.0.1".into());
        set(&mut changes, "MAIL_PORT", svc_port("mailpit", 1025).to_string());
    }
    if env.get("AWS_ENDPOINT").map(|e| e.contains("minio") || e.contains("9000")).unwrap_or(false) {
        set(&mut changes, "AWS_ENDPOINT", format!("http://127.0.0.1:{}", svc_port("rustfs", 9000)));
        set(&mut changes, "AWS_USE_PATH_STYLE_ENDPOINT", "true".into());
    }
    if env.contains_key("REVERB_HOST") && env.get("REVERB_HOST").map(|h| h == "reverb" || h == "0.0.0.0").unwrap_or(false) {
        set(&mut changes, "REVERB_HOST", "localhost".into());
    }
    let mut report: Vec<String> = changes.iter().map(|(k, v)| format!("{k}={v}")).collect();
    if !changes.is_empty() { util::write_env_keys(&env_path, &changes)?; }

    if conn == "pgsql" {
        let db = env.get("DB_DATABASE").cloned().unwrap_or_else(|| site.name.replace(['-', '.'], "_"));
        let user = env.get("DB_USERNAME").cloned().unwrap_or_default();
        let pass = env.get("DB_PASSWORD").cloned().unwrap_or_default();
        crate::services::ensure_database(cfg, &db, &user, &pass)?;
        let testing = format!("{db}_testing");
        let _ = crate::services::ensure_database(cfg, &testing, &user, &pass);
    }
    report.extend(optimize(site));
    Ok(report)
}

/// Windows-specific speed-ups a project needs. Blade Icons (used by Filament) lists thousands of
/// SVG files on every request unless its manifest is cached; listing folders is slow on Windows,
/// so this alone takes a Filament page from ~800 ms to ~100 ms.
pub fn optimize(site: &Site) -> Vec<String> {
    let mut done = Vec::new();
    let vendor = site.path.join("vendor");
    if vendor.join("blade-ui-kit").join("blade-icons").exists() {
        let out = std::process::Command::new(php::php_exe(&site.php))
            .args(["artisan", "icons:cache"])
            .current_dir(&site.path)
            .output();
        if out.map(|o| o.status.success()).unwrap_or(false) { done.push("cached the Blade icon manifest (icons:cache)".to_string()); }
    }
    crate::import::ensure_laravel_dirs(&site.path);
    done
}

pub fn summary(site: &Site) -> BTreeMap<String, String> {
    detect(site).into_iter().map(|p| (p.name, p.reason)).collect()
}
