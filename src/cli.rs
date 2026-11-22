//! The `tez` command line.

use crate::api;
use crate::config::{Config, ServiceConfig};
use crate::daemon;
use crate::fetch;
use crate::paths;
use crate::php;
use crate::project;
use crate::services;
use crate::sites;
use crate::sys;
use crate::util::{self, bold, dim, green, red, yellow, R};
use crate::web;
use crate::bail;
use std::path::{Path, PathBuf};
use std::time::Duration;

const PREPEND_PHP: &str = include_str!("../assets/php/prepend.php");
const HORIZON_PHP: &str = include_str!("../assets/php/horizon-windows.php");
const DUMPS_PHP: &str = include_str!("../assets/php/dumps.php");

pub fn run(args: &[String]) -> i32 {
    match dispatch(args) {
        Ok(()) => 0,
        Err(e) => { eprintln!("{} {e}", red("✗")); 1 }
    }
}

fn dispatch(args: &[String]) -> R<()> {
    let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match a.as_slice() {
        [] | ["help"] | ["-h"] | ["--help"] => { help(); Ok(()) }
        ["version"] | ["--version"] | ["-V"] => { println!("tez {}", env!("CARGO_PKG_VERSION")); Ok(()) }
        ["install", rest @ ..] => install(rest),
        ["daemon"] => daemon::run(),
        ["start"] => start(),
        ["start", "--quiet"] => start_opts(true),
        ["app"] => { if !launch_app(true) { bail!("the desktop app is not installed ({} beside {})", sys::exe("tez-app"), sys::exe("tez")); } Ok(()) }
        ["stop"] => stop(),
        ["restart"] => { stop()?; start() }
        ["restart", name] => proc_action(name, "restart"),
        ["status", "--json"] => { println!("{}", api::call("GET", "/api/status")?); Ok(()) }
        ["status"] | ["ps"] => status(),
        ["open"] | ["dashboard"] => open_dashboard(),
        ["php"] | ["php", "list"] => php_list(),
        ["php", "install", vs @ ..] => php_install(vs),
        ["php", "update", vs @ ..] => php_update(vs),
        ["php", "remove", v] => php_remove(v),
        ["use", v] => use_php(v),
        ["isolate", v] => isolate(Some(v), None),
        ["isolate", v, site] => isolate(Some(v), Some(site)),
        ["unisolate"] => isolate(None, None),
        ["unisolate", site] => isolate(None, Some(site)),
        ["which"] => which(),
        ["ext", "install", name, rest @ ..] => ext_install(name, rest),
        ["ext", "list"] => ext_list(),
        ["xdebug", state] => xdebug(state),
        ["dumps", state] => dumps(state),
        ["park"] => park(None, true),
        ["park", dir] => park(Some(dir), true),
        ["unpark"] => park(None, false),
        ["unpark", dir] => park(Some(dir), false),
        ["link"] => link(None),
        ["link", name] => link(Some(name)),
        ["unlink"] => unlink(None),
        ["unlink", name] => unlink(Some(name)),
        ["sites"] | ["links"] | ["parked"] => sites_list(),
        ["domain", "add", host] => domain(true, host, None),
        ["domain", "add", host, site] => domain(true, host, Some(site)),
        ["domain", "remove", host] => domain(false, host, None),
        ["domain", "remove", host, site] => domain(false, host, Some(site)),
        ["service"] | ["services"] | ["service", "list"] | ["services", "list"] => services_list(),
        ["service", "install", name] => service_install(name, None),
        ["service", "install", name, ver] => service_install(name, Some(ver)),
        ["service", "update", name] => service_update(name),
        ["pg", "ext", "install", ext, rest @ ..] => pg_ext(ext, rest),
        ["pg", "ext"] | ["pg", "ext", "list"] => {
            let cfg = Config::load();
            for (k, sc) in cfg.services.iter().filter(|(k, _)| services::kind(k) == "postgres") {
                let v = services::version_of(k, sc);
                println!("{} {}", bold(&format!("{k} (PostgreSQL {v})")), services::pg_extensions_installed(&v).iter().map(|(n, x)| format!("{n} {x}")).collect::<Vec<_>>().join(", "));
            }
            for (n, about) in services::PG_EXTENSIONS { println!("  {:<12} {}", n, dim(about)); }
            Ok(())
        }
        ["service", "enable", name] => service_enable(name, true),
        ["service", "disable", name] => service_enable(name, false),
        ["service", action @ ("start" | "stop" | "restart"), name] => proc_action(name, action),
        ["db", "create", name] => { services::ensure_database(&Config::load(), name, "", "")?; util::ok(&format!("database {name}")); Ok(()) }
        ["db", "list"] => { println!("{}", services::psql(&Config::load(), "postgres", "SELECT datname FROM pg_database WHERE NOT datistemplate ORDER BY 1")?); Ok(()) }
        ["db", "import", file, db] => db_import(file, db),
        ["db", "pull", container, db, rest @ ..] => db_pull(container, db, rest),
        ["up"] => site_toggle(true),
        ["down"] => site_toggle(false),
        ["adopt"] => adopt(),
        ["import", rest @ ..] => crate::import::run(rest),
        ["clone", repo] => clone(repo, None),
        ["clone", repo, name] => clone(repo, Some(name)),
        ["setup"] => setup(&std::env::current_dir()?),
        ["setup", dir] => setup(&paths::absolute(Path::new(dir))),
        ["autostart", state] => { crate::manage::set_autostart(matches!(*state, "on" | "true" | "1"))?; util::ok(&format!("start at sign-in: {state}")); Ok(()) }
        ["procs"] | ["processes"] => procs(),
        ["logs", name, rest @ ..] => logs(name, rest),
        ["trust", rest @ ..] => trust(rest),
        ["hosts"] => hosts(false),
        ["hosts", "--elevated"] => hosts(true),
        ["doctor", rest @ ..] => doctor(rest),
        ["paths"] => { println!("{}", paths::home().display()); Ok(()) }
        _ => bail!("unknown command `tez {}`. Run `tez help`.", a.join(" ")),
    }
}

fn help() {
    println!(
        "{}  {}\n\n{}",
        bold("tez"),
        dim("fast native Laravel development on Windows, macOS and Linux"),
        r#"Getting started
  tez install [--all] [--php 8.1,8.4] [--services redis,postgres|none]
                                        set up PHP, the web server and services
  tez start | stop | restart            run everything in the background
  tez status [--json]                   what is running, memory, URLs
  tez open                              the dashboard (sites, services, logs, dumps, mail)

Sites (folders in ~/Sites are served at https://<folder>.test)
  tez park [dir] | unpark [dir]         serve every folder inside dir
  tez link [name] | unlink [name]       serve the current folder
  tez sites                             list sites with PHP versions and URLs
  tez domain add admin.app.test [site]  extra domains for multi-domain apps (remove to undo)
  tez trust                             trust HTTPS and add hosts entries (asks for admin once)

PHP
  tez php [list]                        available and installed versions
  tez php install 8.1 8.4 | all         install versions
  tez php update [8.4]                  newest patch release
  tez use 8.4                           default version
  tez isolate 8.1 [site]                pin a site; unisolate to undo
  tez which                             the php binary used in this folder
  tez ext install imagick [--php all]   any PECL extension
  tez xdebug on|off                     tez dumps on|off

Services
  tez services                          PostgreSQL+TimescaleDB, Redis, Mailpit, Meilisearch, S3
  tez service install|update|enable|disable|start|stop|restart <name>
  tez service install postgres 16       a second version runs beside the first (postgres@16)
  tez pg ext install postgis|pgvector|timescaledb [version] [--pg 17]
  tez db create|list | db import <file> <db> | db pull <container> <db> [--user u]

Projects
  tez import /home/me/code/app [name]   copy a project out of WSL (measures first, skips vendor,
                                        node_modules, caches and folders over 1 GB)
  tez adopt                             point a Sail .env at tez, create its database
  tez up | down                         run Horizon/queue, Reverb, Vite, scheduler
  tez procs                             what `tez up` would run, and why
  tez logs <name> [-f] [-n 100]         e.g. tez logs postgres, tez logs mysite/horizon
  tez doctor [--fix]                    check this computer for what slows PHP down"#
    );
}

// ---------------------------------------------------------------- install

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) { (Ok(x), Ok(y)) => x == y, _ => false }
}

fn place_binary(src: &Path, dest: &Path) -> R<()> {
    if dest.exists() {
        if same_file(src, dest) { return Ok(()); }
        if std::fs::remove_file(dest).is_err() {
            // In use (a running php.exe shim or the daemon): Windows still lets us rename it.
            let aside = dest.with_extension(format!("old-{}", std::process::id()));
            std::fs::rename(dest, aside)?;
        }
    }
    std::fs::copy(src, dest)?;
    Ok(())
}

pub fn install_binaries() -> R<()> {
    let me = std::env::current_exe()?;
    let bin = paths::bin();
    let tez = bin.join(sys::exe("tez"));
    if !same_file(&me, &tez) { place_binary(&me, &tez)?; }
    // Windows: hard links (php.exe, composer.exe, exec.exe). macOS/Linux: symlinks to tez; the
    // shell's own `exec` builtin covers Horizon there, so no exec link.
    let shims: &[&str] = if cfg!(windows) { &["php", "composer", "exec"] } else { &["php", "composer"] };
    for name in shims {
        let dest = bin.join(sys::exe(name));
        if (dest.exists() || dest.symlink_metadata().is_ok()) && std::fs::remove_file(&dest).is_err() {
            std::fs::rename(&dest, dest.with_extension(format!("old-{}", std::process::id())))?;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(sys::exe("tez"), &dest)?;
        #[cfg(windows)]
        if std::fs::hard_link(&tez, &dest).is_err() { std::fs::copy(&tez, &dest)?; }
    }
    // Tidy leftovers from earlier upgrades that are no longer in use.
    if let Ok(rd) = std::fs::read_dir(&bin) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().contains(".old-") { let _ = std::fs::remove_file(e.path()); }
        }
    }
    // The desktop app (tray icon and window) ships beside tez.
    if let Some(dir) = me.parent() {
        let app = dir.join(sys::exe("tez-app"));
        let dest = bin.join(sys::exe("tez-app"));
        if app.exists() && !same_file(&app, &dest) {
            let newer = match (app.metadata().and_then(|m| m.modified()), dest.metadata().and_then(|m| m.modified())) { (Ok(a), Ok(b)) => a > b, _ => true };
            if newer { place_binary(&app, &dest)?; }
        }
    }
    let lib = paths::lib_php();
    std::fs::create_dir_all(&lib)?;
    std::fs::write(lib.join("prepend.php"), PREPEND_PHP)?;
    std::fs::write(lib.join("horizon-windows.php"), HORIZON_PHP)?;
    std::fs::write(lib.join("dumps.php"), DUMPS_PHP)?;
    Ok(())
}

fn install_cacert() -> R<()> {
    let base = paths::cache_dir().join("cacert.pem");
    if !base.exists() {
        let text = fetch::get_text("https://curl.se/ca/cacert.pem")?;
        std::fs::write(&base, text)?;
    }
    let mut pem = std::fs::read_to_string(&base)?;
    // Let PHP trust tez's own local HTTPS sites too (Http::get('https://other.test')).
    if let Ok(root) = std::fs::read_to_string(paths::local_ca_root()) {
        pem.push_str("\n# tez local certificate authority\n");
        pem.push_str(&root);
    }
    std::fs::write(paths::cacert(), pem)?;
    Ok(())
}

fn install_composer() -> R<()> {
    let dest = paths::bin().join("composer.phar");
    if dest.exists() { return Ok(()); }
    util::step("Downloading Composer");
    let phar = fetch::download("https://getcomposer.org/download/latest-stable/composer.phar", "composer.phar")?;
    if let Ok(sum) = fetch::get_text("https://getcomposer.org/download/latest-stable/composer.phar.sha256") {
        let sum: String = sum.split_whitespace().next().unwrap_or_default().to_string();
        if sum.len() == 64 { fetch::verify_sha256(&phar, &sum)?; }
    }
    std::fs::copy(phar, dest)?;
    util::ok("Composer");
    Ok(())
}

fn user_path() -> String {
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "[Environment]::GetEnvironmentVariable('Path','User')"])
        .output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

/// macOS/Linux: one guarded line in the shell start-up files that exist (zsh, bash, profile).
fn ensure_on_path_unix() -> R<bool> {
    let bin = paths::bin().display().to_string();
    let line = format!("export PATH=\"{bin}:$PATH\"  # tez");
    let home = paths::user_home();
    let shell = std::env::var("SHELL").unwrap_or_default();
    let mut files: Vec<PathBuf> = [".zshrc", ".bashrc", ".profile"].iter().map(|f| home.join(f)).filter(|p| p.exists()).collect();
    // A new account may have none yet: create the one its shell reads.
    if files.is_empty() { files.push(home.join(if shell.ends_with("zsh") { ".zshrc" } else { ".profile" })); }
    let mut added = false;
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        if text.lines().any(|l| l.contains(&bin) && l.contains("PATH")) { continue; }
        let sep = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
        std::fs::write(&f, format!("{text}{sep}{line}\n"))?;
        added = true;
    }
    Ok(added)
}

fn ensure_on_path() -> R<bool> {
    if !cfg!(windows) { return ensure_on_path_unix(); }
    let bin = paths::bin().display().to_string();
    let current = user_path();
    if current.split(';').any(|p| p.trim_end_matches('\\').eq_ignore_ascii_case(&bin)) { return Ok(false); }
    let new = if current.is_empty() { bin.clone() } else { format!("{bin};{current}") };
    let script = format!("[Environment]::SetEnvironmentVariable('Path', '{}', 'User')", new.replace('\'', "''"));
    let st = std::process::Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]).status()?;
    if !st.success() { bail!("could not add {bin} to your PATH"); }
    Ok(true)
}

fn install(rest: &[&str]) -> R<()> {
    paths::ensure_dirs()?;
    let fresh = !paths::config_file().exists();
    let mut cfg = Config::load();
    let flag = |name: &str| rest.iter().position(|a| *a == name).and_then(|i| rest.get(i + 1)).copied();
    // `--services redis,postgres` (or `none`) picks the services instead of the defaults.
    let picked: Option<Vec<String>> = flag("--services").map(|l| l.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != "none").collect());
    if let Some(list) = &picked {
        for name in list {
            let i = services::info(name)?;
            services::available(name)?;
            cfg.services.entry(name.clone()).or_insert(ServiceConfig { version: i.version.into(), port: 0, enabled: true, ..Default::default() }).enabled = true;
        }
    } else if fresh {
        for name in ["postgres", "redis", "mailpit"] {
            if services::available(name).is_err() { continue; }
            cfg.services.insert(name.into(), ServiceConfig { version: services::info(name)?.version.into(), port: 0, enabled: true, ..Default::default() });
        }
    }
    let all = rest.contains(&"--all");
    let every_php = || php::available_minors().iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut want_php: Vec<String> = vec![cfg.default_php.clone()];
    if let Some(list) = flag("--php") {
        want_php = if list == "all" { every_php() } else { list.split(',').map(php::normalize).collect::<R<Vec<_>>>()? };
    }
    if all {
        want_php = every_php();
        if picked.is_none() {
            for i in services::CATALOG {
                if services::available(i.name).is_err() { continue; }
                cfg.services.entry(i.name.into()).or_insert(ServiceConfig { version: i.version.into(), port: 0, enabled: true, ..Default::default() });
            }
        }
    }
    for p in &cfg.parked { let _ = std::fs::create_dir_all(p); }
    cfg.save()?;

    util::step(&format!("Installing tez into {}", paths::home().display()));
    install_binaries()?;
    install_cacert()?;
    install_composer()?;
    for v in &want_php {
        if !php::is_installed(v) { php::install(v, &cfg)?; } else { php::write_ini(v, &cfg)?; }
    }
    web::install_server(&cfg)?;
    for (name, sc) in cfg.services.clone() {
        let version = if sc.version.is_empty() { services::info(&name)?.version.to_string() } else { sc.version.clone() };
        if sc.enabled && !services::is_installed(&name, &version) { services::install(&name, Some(&version))?; }
    }
    if ensure_on_path()? {
        util::ok(&format!("Added {} to your PATH (open a new terminal to use `php`, `composer` and `tez`)", paths::bin().display()));
    }
    println!();
    util::ok(&bold("tez is installed"));
    println!("  {}  start everything", green("tez start"));
    println!("  {}  trust HTTPS and add hosts entries (one admin prompt)", green("tez trust"));
    println!("  {}  the dashboard", green("tez open"));
    Ok(())
}

// ---------------------------------------------------------------- daemon control

fn start() -> R<()> { start_opts(false) }

fn start_opts(quiet: bool) -> R<()> {
    if api::running() {
        launch_app(false);
        if quiet { return Ok(()); }
        util::ok("tez is already running");
        return status();
    }
    paths::ensure_dirs()?;
    let me = std::env::current_exe()?;
    let installed = paths::bin().join(sys::exe("tez"));
    if !same_file(&me, &installed) && installed.exists() {
        // Running a newer build from elsewhere (cargo run): refresh the installed copy first.
        install_binaries()?;
    }
    let pid = daemon::spawn_background()?;
    if !api::wait_running(15) {
        bail!("tez did not start; see {}", paths::logs_dir().join("daemon.log").display());
    }
    util::ok(&format!("tez is running (pid {pid})"));
    if launch_app(false) { util::ok("tray icon started (tez open shows the window)"); }
    if quiet { return Ok(()); }
    std::thread::sleep(Duration::from_millis(1500));
    status()
}

fn stop() -> R<()> {
    if !api::running() { util::ok("tez is not running"); return Ok(()); }
    let _ = api::call("POST", "/api/shutdown");
    for _ in 0..200 {
        if !api::running() { util::ok("stopped"); return Ok(()); }
        std::thread::sleep(Duration::from_millis(150));
    }
    bail!("tez did not stop in time")
}

fn reload_if_running() -> R<()> {
    if api::running() {
        let body = api::call("POST", "/api/reload")?;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        if let Some(notes) = v["notes"].as_array() {
            for n in notes { util::warn(n.as_str().unwrap_or_default()); }
        }
    }
    Ok(())
}

fn proc_action(name: &str, action: &str) -> R<()> {
    api::call("POST", &format!("/api/proc/{name}/{action}"))?;
    util::ok(&format!("{action} {name}"));
    Ok(())
}

fn status() -> R<()> {
    let body = api::call("GET", "/api/status")?;
    let v: serde_json::Value = serde_json::from_str(&body)?;
    let empty = Vec::new();
    println!("\n{}", bold("Processes"));
    for p in v["procs"].as_array().unwrap_or(&empty) {
        let state = p["state"].as_str().unwrap_or("?");
        let badge = match state { "running" => green("●"), "backoff" | "starting" => yellow("●"), _ => red("●") };
        let mem = p["memory"].as_u64().unwrap_or(0);
        let port = p["port"].as_u64().map(|x| format!(":{x}")).unwrap_or_default();
        let extra = if state != "running" { p["last_exit"].as_str().map(|s| dim(&format!("  {s}"))).unwrap_or_default() } else { String::new() };
        println!("  {badge} {:<28} {:<10} {:>8} {:>7}{extra}", p["name"].as_str().unwrap_or(""), state, if mem > 0 { util::human_bytes(mem) } else { String::new() }, port);
    }
    let total = v["memory"].as_u64().unwrap_or(0);
    println!("  {}", dim(&format!("total memory {}", util::human_bytes(total))));
    println!("\n{}", bold("Sites"));
    for s in v["sites"].as_array().unwrap_or(&empty) {
        let up = if s["up"].as_bool().unwrap_or(false) { green(" up") } else { String::new() };
        println!("  {:<28} PHP {:<4} {}{up}", s["name"].as_str().unwrap_or(""), s["php"].as_str().unwrap_or(""), s["url"].as_str().unwrap_or(""));
    }
    for n in v["notes"].as_array().unwrap_or(&empty) { util::warn(n.as_str().unwrap_or("")); }
    println!("\n  {} http://127.0.0.1:{}", dim("dashboard"), Config::load().api_port);
    Ok(())
}

const APP_INSTANCE_PORT: u16 = 7978;

/// Start the desktop app (tray icon). With `show`, bring its window forward.
fn launch_app(show: bool) -> bool {
    let app = paths::bin().join(sys::exe("tez-app"));
    if !app.exists() { return false; }
    // No display (a server, CI): there is no tray to show.
    if cfg!(target_os = "linux") && std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() { return false; }
    let running = !util::port_free(APP_INSTANCE_PORT);
    if running {
        // Any connection to the app's instance port asks it to show its window.
        if show { let _ = std::net::TcpStream::connect(("127.0.0.1", APP_INSTANCE_PORT)); }
        return true;
    }
    let mut cmd = std::process::Command::new(&app);
    if !show { cmd.arg("--background"); }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    sys::spawn_detached(&mut cmd).is_ok()
}

fn open_dashboard() -> R<()> {
    if !api::running() { start()?; }
    if launch_app(true) { return Ok(()); }
    let url = format!("http://127.0.0.1:{}", Config::load().api_port);
    let edge = PathBuf::from(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe");
    if cfg!(windows) && edge.exists() {
        std::process::Command::new(edge).arg(format!("--app={url}")).spawn()?;
    } else {
        sys::open_url(&url)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- php

fn php_list() -> R<()> {
    let cfg = Config::load();
    let installed = php::installed();
    let releases = php::releases().unwrap_or_default();
    println!("{}", bold("PHP versions"));
    for minor in php::MINORS {
        let inst = installed.iter().find(|(m, _)| m == minor).map(|(_, v)| v.clone());
        let latest = releases.iter().find(|r| r.minor == *minor).map(|r| r.version.clone()).unwrap_or_default();
        let mark = if *minor == cfg.default_php { green(" (default)") } else { String::new() };
        let state = match &inst {
            Some(v) if !latest.is_empty() && v != &latest => yellow(&format!("{v} installed, {latest} available")),
            Some(v) => green(&format!("{v} installed")),
            None if !php::available_minors().contains(minor) => dim(&format!("no {} build", sys::os())),
            None => dim(&format!("{latest} available")),
        };
        let embedded = String::new();
        println!("  {minor:<5} {state}{mark}{embedded}");
    }
    Ok(())
}

fn php_install(vs: &[&str]) -> R<()> {
    let cfg = Config::load();
    let list: Vec<String> = if vs.is_empty() || vs == ["all"] { php::available_minors().iter().map(|s| s.to_string()).collect() } else { vs.iter().map(|v| php::normalize(v)).collect::<R<_>>()? };
    for v in list { php::install(&v, &cfg)?; }
    reload_if_running()
}

fn php_update(vs: &[&str]) -> R<()> {
    let cfg = Config::load();
    let list: Vec<String> = if vs.is_empty() { php::installed().into_iter().map(|(m, _)| m).collect() } else { vs.iter().map(|v| php::normalize(v)).collect::<R<_>>()? };
    let was_running = api::running();
    if was_running { util::step("Stopping tez while PHP is replaced"); stop()?; }
    let mut result = Ok(());
    for v in list { if let Err(e) = php::install(&v, &cfg) { result = Err(e); break; } }
    if was_running { start()?; }
    result.map(|_| ())
}

fn php_remove(v: &str) -> R<()> {
    let v = php::normalize(v)?;
    let cfg = Config::load();
    if cfg.default_php == v { bail!("PHP {v} is the default; `tez use` another version first"); }
    if api::running() { stop()?; }
    std::fs::remove_dir_all(paths::php_dir(&v))?;
    util::ok(&format!("removed PHP {v}"));
    Ok(())
}

fn use_php(v: &str) -> R<()> {
    let v = php::normalize(v)?;
    let mut cfg = Config::load();
    if !php::is_installed(&v) { php::install(&v, &cfg)?; }
    cfg.default_php = v.clone();
    cfg.save()?;
    reload_if_running()?;
    util::ok(&format!("PHP {v} is the default"));
    Ok(())
}

fn current_site(cfg: &Config) -> R<sites::Site> {
    let cwd = std::env::current_dir()?;
    match sites::for_dir(cfg, &cwd) {
        Some(s) => Ok(s),
        None => bail!("{} is not a site. Run `tez link` here, or move it into a parked folder ({}).", cwd.display(), cfg.parked.join(", ")),
    }
}

fn isolate(v: Option<&str>, site: Option<&str>) -> R<()> {
    let mut cfg = Config::load();
    let name = match site { Some(s) => s.to_string(), None => current_site(&cfg)?.name };
    if sites::find(&cfg, &name).is_none() { bail!("no site named {name}"); }
    match v {
        Some(v) => {
            let v = php::normalize(v)?;
            if !php::is_installed(&v) { php::install(&v, &cfg)?; }
            cfg.site_mut(&name).php = Some(v.clone());
            cfg.save()?;
            util::ok(&format!("{name} uses PHP {v}"));
        }
        None => {
            cfg.site_mut(&name).php = None;
            cfg.save()?;
            util::ok(&format!("{name} follows the default PHP again"));
        }
    }
    reload_if_running()
}

fn which() -> R<()> {
    let cfg = Config::load();
    let v = sites::php_for_dir(&cfg, &std::env::current_dir()?);
    println!("{}", php::php_exe(&v).display());
    Ok(())
}

fn ext_install(name: &str, rest: &[&str]) -> R<()> {
    let mut cfg = Config::load();
    let targets: Vec<String> = match rest {
        ["--php", "all"] => php::installed().into_iter().map(|(m, _)| m).collect(),
        ["--php", v] => vec![php::normalize(v)?],
        _ => vec![cfg.default_php.clone()],
    };
    for m in &targets { php::pecl_install(m, name, false)?; }
    if !cfg.extensions.iter().any(|e| e == name) && !php::PECL_DEFAULT.contains(&name) { cfg.extensions.push(name.to_string()); }
    cfg.save()?;
    php::write_all_inis(&cfg)?;
    if api::running() { let _ = api::call("POST", "/api/proc/php/restart"); }
    Ok(())
}

fn ext_list() -> R<()> {
    for (minor, _) in php::installed() {
        let out = std::process::Command::new(php::php_exe(&minor)).arg("-m").output()?;
        let mods: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.starts_with('[') && !l.trim().is_empty()).map(|s| s.trim().to_string()).collect();
        println!("{} {}", bold(&format!("PHP {minor}")), dim(&mods.join(" ")));
        let warn = String::from_utf8_lossy(&out.stderr);
        if !warn.trim().is_empty() { util::warn(warn.trim()); }
    }
    Ok(())
}

fn xdebug(state: &str) -> R<()> {
    let mut cfg = Config::load();
    cfg.xdebug = matches!(state, "on" | "enable" | "true" | "1");
    cfg.save()?;
    php::write_all_inis(&cfg)?;
    if api::running() { let _ = api::call("POST", "/api/proc/php/restart"); }
    util::ok(&format!("Xdebug {}", if cfg.xdebug { "on (start a debug session with the XDEBUG_TRIGGER cookie or ?XDEBUG_TRIGGER=1; port 9003)" } else { "off" }));
    Ok(())
}

fn dumps(state: &str) -> R<()> {
    let flag = paths::run_dir().join("dumps-on");
    if matches!(state, "on" | "enable" | "true" | "1") {
        std::fs::write(&flag, b"1")?;
        util::ok("Dumps on: dump() and dd() also appear in the dashboard's Dumps tab");
    } else {
        let _ = std::fs::remove_file(&flag);
        util::ok("Dumps off");
    }
    Ok(())
}

// ---------------------------------------------------------------- sites

fn park(dir: Option<&str>, on: bool) -> R<()> {
    let mut cfg = Config::load();
    let d = paths::absolute(Path::new(dir.unwrap_or(".")));
    let ds = d.display().to_string();
    if on {
        if !cfg.parked.iter().any(|p| p.eq_ignore_ascii_case(&ds)) { cfg.parked.push(ds.clone()); }
        util::ok(&format!("every folder in {ds} is now a site"));
    } else {
        cfg.parked.retain(|p| !p.eq_ignore_ascii_case(&ds));
        util::ok(&format!("{ds} is no longer parked"));
    }
    cfg.save()?;
    hosts_hint(&cfg);
    reload_if_running()
}

fn link(name: Option<&str>) -> R<()> {
    let mut cfg = Config::load();
    let cwd = paths::absolute(Path::new("."));
    let name = util::host_label(name.unwrap_or(&cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
    if sites::RESERVED.contains(&name.as_str()) { bail!("`{name}` is reserved for the dashboard; pass another name: tez link <name>"); }
    cfg.links.insert(name.clone(), cwd.display().to_string());
    cfg.save()?;
    let site = sites::find(&cfg, &name).unwrap();
    util::ok(&format!("{} → {}  (PHP {})", site.url(&cfg), cwd.display(), site.php));
    hosts_hint(&cfg);
    reload_if_running()
}

fn unlink(name: Option<&str>) -> R<()> {
    let mut cfg = Config::load();
    let name = match name { Some(n) => n.to_string(), None => current_site(&cfg)?.name };
    if cfg.links.remove(&name).is_none() { bail!("{name} is not a linked site"); }
    cfg.save()?;
    util::ok(&format!("unlinked {name}"));
    reload_if_running()
}

fn sites_list() -> R<()> {
    let cfg = Config::load();
    let list = sites::all(&cfg);
    if list.is_empty() { println!("No sites yet. Put projects in {} or run `tez link` in a project.", cfg.parked.join(", ")); }
    for s in list {
        let pin = if s.settings.php.is_some() { dim(" (isolated)") } else { String::new() };
        let kind = if s.linked { "linked" } else { "parked" };
        println!("  {:<30} PHP {}{pin:<12} {:<45} {}", bold(&s.name), s.php, s.url(&cfg), dim(&format!("{kind}  {}", s.path.display())));
    }
    Ok(())
}

// ---------------------------------------------------------------- services

fn services_list() -> R<()> {
    let cfg = Config::load();
    println!("{}", bold("Services"));
    for i in services::CATALOG {
        let sc = cfg.services.get(i.name);
        let version = sc.map(|s| s.version.clone()).filter(|v| !v.is_empty()).unwrap_or(i.version.to_string());
        let installed = services::is_installed(i.name, &version);
        let enabled = sc.map(|s| s.enabled).unwrap_or(false);
        let port = sc.map(|s| s.port).filter(|p| *p > 0).unwrap_or(i.port);
        let state = if installed && enabled { green("enabled") } else if installed { yellow("installed, disabled") }
            else if services::available(i.name).is_err() { dim(&format!("no {} build", sys::os())) } else { dim("not installed") };
        println!("  {:<12} {:<9} {:<22} :{port:<6} {}", i.name, version, state, dim(i.about));
    }
    Ok(())
}

fn service_install(name: &str, ver: Option<&str>) -> R<()> {
    let mut cfg = Config::load();
    let info = services::info(name)?;
    let kind = services::kind(name).to_string();
    let want = ver.unwrap_or(info.version).to_string();
    // A second version of a service runs beside the first, on its own port: postgres@16.
    let key = if name.contains('@') { name.to_string() } else { services::instance_key(&cfg, &kind, &want) };
    let v = services::install(&kind, Some(&want))?;
    let port = match cfg.services.get(&key) { Some(sc) if sc.port != 0 => sc.port, _ if key == kind => 0, _ => services::free_port(&cfg, info.port + 1) };
    let e = cfg.services.entry(key.clone()).or_default();
    e.version = v;
    e.enabled = true;
    e.port = port;
    cfg.save()?;
    if key != kind { util::ok(&format!("runs as `{key}` on port {port}")); }
    reload_if_running()
}

fn pg_ext(ext: &str, rest: &[&str]) -> R<()> {
    let cfg = Config::load();
    let (mut pg, mut version) = (None, None);
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == "--pg" { pg = rest.get(i + 1).map(|s| s.to_string()); i += 2; } else { version = Some(rest[i]); i += 1; }
    }
    let pg = pg.unwrap_or_else(|| services::version_of("postgres", &cfg.services.get("postgres").cloned().unwrap_or_default()));
    services::install_pg_extension(&services::series("postgres", &pg), ext, version)?;
    if ext == "timescaledb" { if api::running() { let _ = api::call("POST", "/api/proc/postgres/restart"); } }
    println!("  {}", dim(&format!("enable it in a database with: CREATE EXTENSION {};", if ext == "pgvector" { "vector" } else { ext })));
    Ok(())
}

fn service_update(name: &str) -> R<()> {
    let mut cfg = Config::load();
    let sc = cfg.services.get(name).cloned().unwrap_or_default();
    let latest = match name {
        "redis" => fetch::github_latest_tag(if cfg!(windows) { "redis-windows/redis-windows" } else { "redis/redis" })?,
        "mailpit" => fetch::github_latest_tag("axllent/mailpit")?,
        "meilisearch" => fetch::github_latest_tag("meilisearch/meilisearch")?,
        "rustfs" => fetch::github_latest_tag("rustfs/rustfs")?,
        "postgres" => if sc.version.is_empty() { "17".into() } else { sc.version.split('.').next().unwrap_or("17").to_string() },
        _ => bail!("unknown service {name}"),
    };
    let latest = latest.trim_start_matches('v').to_string();
    if latest == sc.version && name != "postgres" { util::ok(&format!("{name} {latest} is the newest")); return Ok(()); }
    let running = api::running();
    if running { let _ = api::call("POST", &format!("/api/proc/{name}/stop")); }
    if name == "postgres" {
        // Same major, newer minor: binaries are replaced in place; the data folder stays.
        let _ = std::fs::remove_file(services::exe("postgres", &latest));
    }
    let v = services::install(name, Some(&latest))?;
    cfg.services.entry(name.into()).or_default().version = v.clone();
    cfg.save()?;
    reload_if_running()?;
    if running { let _ = api::call("POST", &format!("/api/proc/{name}/start")); }
    util::ok(&format!("{name} is now {v}"));
    Ok(())
}

fn service_enable(name: &str, on: bool) -> R<()> {
    services::info(name)?;
    let mut cfg = Config::load();
    cfg.services.entry(name.into()).or_default().enabled = on;
    cfg.save()?;
    reload_if_running()?;
    util::ok(&format!("{name} {}", if on { "enabled" } else { "disabled" }));
    Ok(())
}

fn pg_bin(cfg: &Config, exe: &str) -> PathBuf {
    let v = cfg.services.get("postgres").map(|s| s.version.clone()).filter(|v| !v.is_empty()).unwrap_or("17".into());
    services::dir("postgres", &v).join("bin").join(exe)
}

fn pg_port(cfg: &Config) -> String { cfg.services.get("postgres").map(|s| s.port).filter(|p| *p > 0).unwrap_or(5432).to_string() }

fn db_import(file: &str, db: &str) -> R<()> {
    let cfg = Config::load();
    services::ensure_database(&cfg, db, "", "")?;
    let custom = !file.ends_with(".sql");
    let status = if custom {
        std::process::Command::new(pg_bin(&cfg, &sys::exe("pg_restore")))
            .args(["-h", "127.0.0.1", "-p", &pg_port(&cfg), "-U", "postgres", "-d", db, "--no-owner", "--no-acl", file]).status()?
    } else {
        std::process::Command::new(pg_bin(&cfg, &sys::exe("psql")))
            .args(["-h", "127.0.0.1", "-p", &pg_port(&cfg), "-U", "postgres", "-d", db, "-f", file]).status()?
    };
    if !status.success() { util::warn("the import reported errors (often harmless: roles or extensions that already exist)"); }
    util::ok(&format!("imported {file} into {db}"));
    Ok(())
}

/// Copy a database out of a running Docker/Sail container into tez's PostgreSQL.
fn db_pull(container: &str, db: &str, rest: &[&str]) -> R<()> {
    let user = rest.iter().position(|a| *a == "--user").and_then(|i| rest.get(i + 1)).copied().unwrap_or("postgres");
    let target = rest.iter().find(|a| !a.starts_with("--") && Some(**a) != Some(user)).copied().unwrap_or(db);
    let dump = paths::cache_dir().join(format!("{container}-{db}.dump"));
    util::step(&format!("Dumping {db} from container {container}"));
    let out = std::process::Command::new("docker")
        .args(["exec", container, "pg_dump", "-U", user, "-Fc", db])
        .stdout(std::fs::File::create(&dump)?)
        .status()?;
    if !out.success() { bail!("pg_dump inside {container} failed"); }
    db_import(&dump.display().to_string(), target)
}

// ---------------------------------------------------------------- projects

fn site_toggle(up: bool) -> R<()> {
    let cfg = Config::load();
    let site = current_site(&cfg)?;
    if !api::running() { start()?; }
    if up { for d in project::optimize(&site) { util::ok(&d); } }
    api::call("POST", &format!("/api/site/{}/{}", site.name, if up { "up" } else { "down" }))?;
    if up {
        for (name, why) in project::summary(&site) { util::ok(&format!("{}/{name}  {}", site.name, dim(&why))); }
        println!("  {} tez logs {}/<process> -f", dim("logs:"), site.name);
    } else {
        util::ok(&format!("{} processes stopped", site.name));
    }
    Ok(())
}

fn adopt() -> R<()> {
    let cfg = Config::load();
    let site = current_site(&cfg)?;
    if !api::running() { start()?; }
    // Wait for PostgreSQL to accept connections before creating databases.
    for _ in 0..60 {
        if services::psql(&cfg, "postgres", "SELECT 1").is_ok() { break; }
        std::thread::sleep(Duration::from_millis(500));
    }
    let changes = project::adopt(&cfg, &site)?;
    util::ok(&format!("{} now uses tez's services (original saved as .env.sail)", site.name));
    for c in changes { println!("  {}", dim(&c)); }
    hosts_hint(&Config::load());
    reload_if_running()
}

/// Clone a GitHub repository into the first parked folder and set it up.
fn clone(repo: &str, name: Option<&str>) -> R<()> {
    let cfg = Config::load();
    let repo = repo.trim().trim_end_matches(".git").trim_start_matches("https://github.com/").to_string();
    if !repo.contains('/') { bail!("give the repository as owner/name"); }
    let folder = name.map(|s| s.to_string()).unwrap_or_else(|| repo.rsplit('/').next().unwrap_or("site").to_string());
    let parked = cfg.parked.first().cloned().ok_or("no parked folder; run `tez park <dir>` first")?;
    let dest = PathBuf::from(&parked).join(&folder);
    if dest.exists() { bail!("{} already exists", dest.display()); }
    fetch::ensure_space(Path::new(&parked), 500 << 20)?;
    util::step(&format!("Cloning {repo} into {}", dest.display()));
    let gh = std::process::Command::new("gh").args(["repo", "clone", &repo]).arg(&dest).status();
    let ok = match gh {
        Ok(s) => s.success(),
        Err(_) => std::process::Command::new("git").args(["clone", &format!("https://github.com/{repo}.git")]).arg(&dest).status()?.success(),
    };
    if !ok { bail!("clone failed"); }
    util::ok("cloned");
    reload_if_running()?;
    setup(&dest)
}

/// Make a freshly cloned or copied Laravel project run on tez, step by step.
fn setup(dir: &Path) -> R<()> {
    let cfg = Config::load();
    let site = sites::for_dir(&cfg, dir).ok_or_else(|| format!("{} is not a site (put it in a parked folder or `tez link` it)", dir.display()))?;
    let php = php::php_exe(&site.php);
    if !php.exists() { php::install(&site.php, &cfg)?; }
    let run = |label: &str, exe: &Path, args: &[&str]| -> bool {
        util::step(label);
        let st = std::process::Command::new(exe).args(args).current_dir(&site.path).env("PATH", project::child_path()).status();
        match st { Ok(s) if s.success() => true, _ => { util::warn(&format!("{label} did not finish; continuing")); false } }
    };
    crate::import::ensure_laravel_dirs(&site.path);
    if site.path.join("composer.json").exists() {
        run("composer install", &paths::bin().join(sys::exe("composer")), &["install", "--no-interaction", "--no-progress"]);
    }
    if site.laravel {
        let env = site.path.join(".env");
        if !env.exists() && site.path.join(".env.example").exists() {
            std::fs::copy(site.path.join(".env.example"), &env)?;
            util::ok(".env created from .env.example");
        }
        if util::read_env(&env).get("APP_KEY").map(|k| k.is_empty()).unwrap_or(true) {
            run("php artisan key:generate", &php, &["artisan", "key:generate", "--force"]);
        }
        if !api::running() { start()?; }
        for _ in 0..60 { if services::psql(&cfg, "postgres", "SELECT 1").is_ok() { break; } std::thread::sleep(Duration::from_millis(500)); }
        match project::adopt(&cfg, &site) {
            Ok(changes) => { util::ok(".env points at tez's services"); for c in changes { println!("  {}", dim(&c)); } }
            Err(e) => util::warn(&format!("adopt: {e}")),
        }
    }
    if site.path.join("package.json").exists() {
        let npm_name = if cfg!(windows) { "npm.cmd" } else { "npm" };
        if let Some(npm) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|d| d.join(npm_name)).find(|p| p.exists()) {
            run("npm install", &npm, &["install", "--no-audit", "--no-fund"]);
        } else { util::warn("Node.js is not installed; skipping npm install"); }
    }
    if site.laravel {
        run("php artisan migrate", &php, &["artisan", "migrate", "--force"]);
        if !site.path.join("public").join("storage").exists() { run("php artisan storage:link", &php, &["artisan", "storage:link"]); }
        for d in project::optimize(&site) { util::ok(&d); }
    }
    util::ok(&format!("{} is ready at {}", site.name, site.url(&cfg)));
    println!("  {}", dim("start Horizon, Reverb and Vite with `tez up` in that folder, or from the dashboard"));
    Ok(())
}

fn procs() -> R<()> {
    let cfg = Config::load();
    let site = current_site(&cfg)?;
    for p in project::detect(&site) {
        println!("  {:<10} {:<40} {}", bold(&p.name), p.command.join(" ").replace("@php", "php"), dim(&p.reason));
    }
    Ok(())
}

fn logs(name: &str, rest: &[&str]) -> R<()> {
    let n = rest.iter().position(|a| *a == "-n").and_then(|i| rest.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(80usize);
    let path = if name == "daemon" { paths::logs_dir().join("daemon.log") } else { daemon::log_path(name) };
    if !path.exists() { bail!("no log for {name} at {}", path.display()); }
    println!("{}", util::tail(&path, n));
    if rest.contains(&"-f") {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(&path)?;
        let mut pos = f.seek(SeekFrom::End(0))?;
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let len = f.metadata()?.len();
            if len < pos { pos = 0; }
            if len > pos {
                f.seek(SeekFrom::Start(pos))?;
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;
                print!("{}", String::from_utf8_lossy(&buf));
                pos = len;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- trust, hosts, doctor

fn hosts_file() -> PathBuf {
    if cfg!(windows) { paths::system32("drivers").join("etc").join("hosts") } else { PathBuf::from("/etc/hosts") }
}

fn admin_word() -> &'static str { if cfg!(windows) { "one admin prompt" } else { "asks for your password once (sudo)" } }

fn wanted_hosts(cfg: &Config) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for s in sites::all(cfg) {
        v.push(s.host(&cfg.tld));
        // *.localhost resolves by itself; wildcards cannot go in a hosts file.
        v.extend(s.settings.domains.iter().map(|d| d.to_ascii_lowercase()).filter(|d| !d.ends_with(".localhost") && d != "localhost" && !d.contains('*')));
    }
    v.push(format!("tez.{}", cfg.tld));
    v.dedup();
    v
}

fn domain(add: bool, host: &str, site: Option<&str>) -> R<()> {
    let mut cfg = Config::load();
    let name = match site { Some(s) => s.to_string(), None => current_site(&cfg)?.name };
    if sites::find(&cfg, &name).is_none() { bail!("no site named {name}"); }
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() || host.contains(['/', ' ', ',']) { bail!("'{host}' is not a host name"); }
    let entry = cfg.site_mut(&name);
    entry.domains.retain(|d| d != &host);
    if add { entry.domains.push(host.clone()); }
    cfg.save()?;
    util::ok(&format!("{name} {} {host}", if add { "now answers on" } else { "no longer answers on" }));
    if add { hosts_hint(&cfg); }
    reload_if_running()
}

fn missing_hosts(cfg: &Config) -> Vec<String> {
    let text = std::fs::read_to_string(hosts_file()).unwrap_or_default().to_ascii_lowercase();
    let present: Vec<String> = text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .flat_map(|l| l.split_whitespace().skip(1).map(|s| s.to_string()).collect::<Vec<_>>())
        .collect();
    wanted_hosts(cfg).into_iter().filter(|h| !present.contains(h)).collect()
}

fn hosts_hint(cfg: &Config) {
    let missing = missing_hosts(cfg);
    if !missing.is_empty() {
        util::warn(&format!("{} not in the hosts file yet: run `tez hosts` ({}). Until then use https://<site>.localhost", missing.join(", "), admin_word()));
    }
}

fn write_hosts_block(cfg: &Config) -> R<()> {
    let path = hosts_file();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let nl = sys::NL;
    let mut out = String::new();
    let mut skipping = false;
    for line in text.lines() {
        if line.trim() == "# tez-begin" { skipping = true; continue; }
        if line.trim() == "# tez-end" { skipping = false; continue; }
        if !skipping { out.push_str(line); out.push_str(nl); }
    }
    let others: String = out.to_ascii_lowercase();
    let mut block = format!("# tez-begin{nl}");
    for h in wanted_hosts(cfg) {
        let already = others.lines().filter(|l| !l.trim_start().starts_with('#')).any(|l| l.split_whitespace().skip(1).any(|x| x == h));
        if !already { block.push_str(&format!("127.0.0.1 {h}{nl}")); }
    }
    block.push_str(&format!("# tez-end{nl}"));
    let trimmed = out.trim_end().to_string();
    std::fs::write(&path, format!("{trimmed}{nl}{block}"))?;
    Ok(())
}

fn hosts(elevated: bool) -> R<()> {
    let cfg = Config::load();
    if missing_hosts(&cfg).is_empty() && !elevated { util::ok("hosts file already has every site"); return Ok(()); }
    if sys::is_elevated() {
        write_hosts_block(&cfg)?;
        util::ok("hosts file updated");
        return Ok(());
    }
    util::step(if cfg!(windows) { "Asking Windows for permission to edit the hosts file" } else { "Editing /etc/hosts needs root: sudo asks for your password" });
    let code = sys::run_elevated(&paths::bin().join(sys::exe("tez")), &["hosts".into(), "--elevated".into()])?;
    if code != 0 { bail!("the hosts file was not changed"); }
    util::ok("hosts file updated");
    Ok(())
}

/// Add tez's root certificate to the system's trust store (runs as root / elevated).
fn trust_root(root: &Path) -> R<()> {
    if cfg!(windows) {
        let st = std::process::Command::new(paths::system32("certutil.exe")).args(["-addstore", "-f", "Root"]).arg(root).output()?;
        if !st.status.success() { bail!("certutil could not add the certificate: {}", String::from_utf8_lossy(&st.stdout)); }
    } else if cfg!(target_os = "macos") {
        let st = std::process::Command::new("security")
            .args(["add-trusted-cert", "-d", "-r", "trustRoot", "-k", "/Library/Keychains/System.keychain"]).arg(root).output()?;
        if !st.status.success() { bail!("security could not add the certificate: {}", String::from_utf8_lossy(&st.stderr)); }
    } else {
        // Debian/Ubuntu first, then Fedora/RHEL; whichever layout this system has.
        let mut done = false;
        for (file, tool) in crate::manage::LINUX_CA_FILES {
            let p = Path::new(file);
            if !p.parent().map(|d| d.exists()).unwrap_or(false) { continue; }
            std::fs::copy(root, p)?;
            let st = std::process::Command::new(tool).output()?;
            if !st.status.success() { bail!("{tool} failed: {}", String::from_utf8_lossy(&st.stderr)); }
            done = true;
            break;
        }
        if !done { bail!("no system certificate folder found (expected /usr/local/share/ca-certificates or /etc/pki/ca-trust/source/anchors)"); }
    }
    Ok(())
}

fn trust(rest: &[&str]) -> R<()> {
    let cfg = Config::load();
    let root = paths::local_ca_root();
    if !root.exists() {
        if !api::running() { start()?; }
        for _ in 0..40 { if root.exists() { break; } std::thread::sleep(Duration::from_millis(250)); }
        if !root.exists() { bail!("the local certificate authority was not created yet; open https://tez.localhost once and retry"); }
    }
    if rest.contains(&"--elevated") || sys::is_elevated() {
        trust_root(&root)?;
        write_hosts_block(&cfg)?;
        return Ok(());
    }
    util::step(if cfg!(windows) { "Asking Windows for permission to trust tez's local HTTPS certificate and update the hosts file" }
        else { "Trusting tez's local HTTPS certificate and updating /etc/hosts needs root: sudo asks for your password" });
    let code = sys::run_elevated(&paths::bin().join(sys::exe("tez")), &["trust".into(), "--elevated".into()])?;
    if code != 0 { bail!("trust was not completed (the prompt was declined?)"); }
    install_cacert()?;
    php::write_all_inis(&cfg)?;
    let who = match sys::os() { "windows" => "Windows, Edge and Chrome", "macos" => "macOS, Safari and Chrome", _ => "the system (curl, Chrome after a restart)" };
    util::ok(&format!("HTTPS for every site is trusted by {who}; PHP trusts it too"));
    println!("  {}", dim(&format!("Firefox: set security.enterprise_roots.enabled = true. Node: NODE_EXTRA_CA_CERTS={}", root.display())));
    Ok(())
}

/// `tez doctor` on macOS and Linux: disk, ports, PATH and build tools.
fn doctor_unix(cfg: &Config) -> R<()> {
    let free = sys::free_bytes(&paths::home());
    if free < 10 << 30 { util::warn(&format!("{} free on the disk holding {}", util::human_bytes(free), paths::home().display())); }
    else { util::ok(&format!("{} free", util::human_bytes(free))); }
    for (port, what) in [(cfg.http_port, "HTTP"), (cfg.https_port, "HTTPS")] {
        if !util::port_free(port) && !(api::running() && util::port_open(2019)) {
            let why = if port < 1024 && cfg!(target_os = "linux") && !sys::is_elevated() { "needs root on Linux" } else { "is in use" };
            util::warn(&format!("port {port} ({what}) {why}; tez uses {} instead", if what == "HTTP" { 8088 } else { 8443 }));
        }
    }
    let php = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|d| d.join("php")).find(|p| p.exists());
    match php {
        Some(p) if p.starts_with(paths::bin()) => util::ok("`php` is tez's php"),
        Some(p) => util::warn(&format!("`php` resolves to {}. Put {} first in PATH (open a new terminal after `tez install`).", p.display(), paths::bin().display())),
        None => util::warn(&format!("`php` is not on PATH yet; open a new terminal after `tez install` (or add {})", paths::bin().display())),
    }
    if let Err(e) = services::available("redis") { util::warn(&e); }
    Ok(())
}

fn doctor(rest: &[&str]) -> R<()> {
    let cfg = Config::load();
    println!("{}", bold("tez doctor"));
    if !cfg!(windows) {
        doctor_unix(&cfg)?;
        if rest.contains(&"--fix") { util::ok("nothing to fix automatically on this OS"); }
        return Ok(());
    }
    // Disk space.
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", "(Get-PSDrive C).Free"])
        .output()?;
    let free: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0);
    if free < 50 << 30 {
        util::warn(&format!("C: has {} free. SSDs slow down when nearly full, and a Dev Drive needs 50 GB.", util::human_bytes(free)));
    } else { util::ok(&format!("C: has {} free", util::human_bytes(free))); }
    // Ports.
    for (port, what) in [(cfg.http_port, "HTTP"), (cfg.https_port, "HTTPS")] {
        if !util::port_free(port) && !(api::running() && util::port_open(2019)) {
            let owner = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-Command", &format!("(Get-NetTCPConnection -LocalPort {port} -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1 | ForEach-Object {{ (Get-Process -Id $_.OwningProcess).Name }})")])
                .output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
            util::warn(&format!("port {port} ({what}) is held by {}. {}", if owner.is_empty() { "another program" } else { &owner },
                if owner.contains("wslrelay") { "That is a web server inside WSL (sudo systemctl disable --now nginx in Ubuntu frees it)." } else { "" }));
        }
    }
    // WSL.
    if let Ok(cfgtext) = std::fs::read_to_string(PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join(".wslconfig")) {
        if let Some(line) = cfgtext.lines().find(|l| l.trim_start().starts_with("memory")) {
            util::warn(&format!("WSL is capped at `{}`; with projects on tez, WSL can be shut down (`wsl --shutdown`) to free that memory", line.trim()));
        }
    }
    // Defender.
    let rt = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", "(Get-MpComputerStatus).RealTimeProtectionEnabled"])
        .output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    if rt.eq_ignore_ascii_case("true") {
        util::warn("Microsoft Defender scans every file PHP opens. `tez doctor --fix` excludes the tez folder and your parked folders (admin prompt).");
    }
    // PATH order.
    let out = std::process::Command::new(paths::system32("where.exe")).arg("php").output()?;
    let first = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or_default().to_string();
    if !first.to_ascii_lowercase().starts_with(&paths::bin().display().to_string().to_ascii_lowercase()) {
        util::warn(&format!("`php` resolves to {first}. Put {} first in PATH (open a new terminal after `tez install`).", paths::bin().display()));
    } else { util::ok("`php` is tez's php"); }
    // Dev Drive.
    let dd = std::process::Command::new(paths::system32("fsutil.exe")).args(["devdrv", "query"]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    if dd.contains("enabled") { util::ok("Dev Drive is available: with 50 GB free, a Dev Drive for ~\\Sites makes file access faster still"); }

    if rest.contains(&"--fix") {
        let mut dirs = vec![paths::home().display().to_string()];
        dirs.extend(cfg.parked.iter().cloned());
        dirs.extend(cfg.links.values().cloned());
        let list = dirs.iter().map(|d| format!("'{}'", d.replace('\'', "''"))).collect::<Vec<_>>().join(",");
        let script = format!("Add-MpPreference -ExclusionPath @({list})");
        util::step("Adding Defender exclusions for tez and your sites (admin prompt)");
        let code = sys::run_elevated(Path::new("powershell.exe"), &["-NoProfile".into(), "-Command".into(), script])?;
        if code == 0 { util::ok("Defender no longer scans tez or your site folders"); } else { util::warn("Defender exclusions were not added"); }
    }
    Ok(())
}
