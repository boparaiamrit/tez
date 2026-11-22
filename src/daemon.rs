//! The tez daemon: keeps every service, PHP pool, the web server and project processes running,
//! restarts what crashes (with backoff), and serves the dashboard and API on 127.0.0.1.

use crate::config::Config;
use crate::paths;
use crate::project;
use crate::services;
use crate::sites;
use crate::spec::Spec;
use crate::sys::{self, Hidden, Job};
use crate::util::R;
use crate::web;
use std::fs::OpenOptions;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State { Starting, Running, Backoff, Stopped, Failed }

pub struct Proc {
    pub spec: Spec,
    child: Option<Child>,
    job: Option<Job>,
    pub state: State,
    pub restarts: u32,
    started: Option<Instant>,
    next_start: Option<Instant>,
    pub last_exit: Option<String>,
    /// False once the user stops it by hand; the daemon then leaves it alone.
    pub wanted: bool,
}

impl Proc {
    fn new(spec: Spec) -> Proc {
        Proc { spec, child: None, job: None, state: State::Stopped, restarts: 0, started: None, next_start: Some(Instant::now()), last_exit: None, wanted: true }
    }
    pub fn pid(&self) -> Option<u32> { self.child.as_ref().map(|c| c.id()) }
    pub fn uptime(&self) -> u64 { self.started.map(|s| s.elapsed().as_secs()).unwrap_or(0) }
    pub fn memory(&self) -> u64 { self.job.as_ref().map(|j| j.memory()).unwrap_or(0) }
    /// CPU time used by this process and everything it started, in nanoseconds.
    pub fn cpu_time(&self) -> Option<u64> { self.job.as_ref().map(|j| j.cpu_time()) }
    pub fn log_path(&self) -> std::path::PathBuf { log_path(&self.spec.name) }
}

pub fn log_path(name: &str) -> std::path::PathBuf { paths::logs_dir().join(format!("{}.log", name.replace(['/', '\\', ':'], "-"))) }

pub struct Daemon {
    pub procs: Vec<Proc>,
    pub started: SystemTime,
    pub notes: Vec<String>,
}

pub type Shared = Arc<Mutex<Daemon>>;

fn start(p: &mut Proc) {
    let log = p.log_path();
    let _ = std::fs::create_dir_all(paths::logs_dir());
    let open = || OpenOptions::new().create(true).append(true).open(&log);
    let (Ok(out), Ok(err)) = (open(), open()) else {
        p.state = State::Failed;
        p.last_exit = Some(format!("cannot open log {}", log.display()));
        return;
    };
    {
        use std::io::Write;
        let mut f = open().unwrap();
        let _ = writeln!(f, "\n==== tez: starting {} at {} UTC ====", p.spec.name, crate::util::utc_now());
    }
    let mut cmd = Command::new(&p.spec.exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if p.spec.raw { for a in &p.spec.args { cmd.raw_arg(a); } } else { cmd.args(&p.spec.args); }
    }
    #[cfg(not(windows))]
    cmd.args(&p.spec.args);
    cmd.current_dir(&p.spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    sys::prepare_child(&mut cmd);
    for (k, v) in &p.spec.env { cmd.env(k, v); }
    match cmd.spawn() {
        Ok(child) => {
            let job = Job::attach(&child);
            p.child = Some(child);
            p.job = job;
            p.state = State::Running;
            p.started = Some(Instant::now());
            p.next_start = None;
        }
        Err(e) => {
            p.state = State::Backoff;
            p.last_exit = Some(format!("could not start {}: {e}", p.spec.exe.display()));
            p.restarts += 1;
            p.next_start = Some(Instant::now() + backoff(p.restarts));
        }
    }
}

fn backoff(n: u32) -> Duration { Duration::from_secs((1u64 << n.min(5)).min(30)) }

fn stop(p: &mut Proc) {
    if let Some((exe, args)) = p.spec.stop.clone() {
        if p.child.is_some() {
            let _ = Command::new(exe).args(args).hidden()
                .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    }
    if let Some(child) = p.child.as_mut() {
        let deadline = Instant::now() + Duration::from_secs(if p.spec.stop.is_some() { 15 } else { 0 });
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() { break; }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    sys::kill_tree(p.job.take(), p.child.take());
    p.state = State::Stopped;
    p.started = None;
}

impl Daemon {
    /// Make the running set match `desired`: start new, stop removed, restart changed.
    pub fn reconcile(&mut self, desired: Vec<Spec>) {
        let names: Vec<String> = desired.iter().map(|s| s.name.clone()).collect();
        let mut i = 0;
        while i < self.procs.len() {
            if !names.contains(&self.procs[i].spec.name) {
                let mut p = self.procs.remove(i);
                stop(&mut p);
            } else { i += 1; }
        }
        for spec in desired {
            match self.procs.iter_mut().find(|p| p.spec.name == spec.name) {
                Some(p) if p.spec == spec => {}
                Some(p) => {
                    stop(p);
                    p.spec = spec;
                    p.restarts = 0;
                    if p.wanted { p.next_start = Some(Instant::now()); }
                }
                None => self.procs.push(Proc::new(spec)),
            }
        }
        // Services first, then PHP pools, the web server, then project processes.
        let rank = |g: &str| match g { "service" => 0, "php" => 1, "web" => 2, _ => 3 };
        self.procs.sort_by_key(|p| rank(&p.spec.group));
    }

    /// Pids of the running processes (each leads its own group on macOS and Linux).
    pub fn running_pids(&self) -> Vec<u32> { self.procs.iter().filter_map(|p| p.pid()).collect() }

    pub fn tick(&mut self) {
        let now = Instant::now();
        for p in self.procs.iter_mut() {
            if let Some(child) = p.child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    let ran = p.started.map(|s| s.elapsed()).unwrap_or_default();
                    p.child = None;
                    p.job = None;
                    p.started = None;
                    p.last_exit = Some(format!("exited with code {}", status.code().unwrap_or(-1)));
                    if ran > Duration::from_secs(60) { p.restarts = 0; }
                    p.restarts += 1;
                    if p.wanted {
                        p.state = if p.restarts > 8 { State::Failed } else { State::Backoff };
                        // A process that keeps failing stays stopped until someone restarts it; no endless loop.
                        p.next_start = if p.state == State::Failed { None } else { Some(now + backoff(p.restarts)) };
                    } else {
                        p.state = State::Stopped;
                    }
                }
            }
            if p.child.is_none() && p.wanted {
                if let Some(at) = p.next_start {
                    if now >= at { start(p); }
                }
            }
        }
    }

    pub fn stop_all(&mut self) {
        // Reverse order: project processes, web, PHP, then services.
        for p in self.procs.iter_mut().rev() { stop(p); }
    }

    pub fn find(&mut self, name: &str) -> Option<&mut Proc> { self.procs.iter_mut().find(|p| p.spec.name == name) }

    /// A name matches one process, a whole site ("acmeshop"), all PHP pools ("php") or one
    /// version's pool ("php-8.4").
    fn matches(p: &Proc, name: &str) -> bool {
        p.spec.name == name
            || p.spec.group == format!("site:{name}")
            || (name == "php" && p.spec.group == "php")
            || (p.spec.group == "php" && p.spec.name.starts_with(&format!("{name}-")))
    }

    pub fn set_wanted(&mut self, name: &str, wanted: bool) -> bool {
        let matched: Vec<usize> = self.procs.iter().enumerate()
            .filter(|(_, p)| Self::matches(p, name))
            .map(|(i, _)| i).collect();
        for &i in &matched {
            let p = &mut self.procs[i];
            p.wanted = wanted;
            if wanted { if p.child.is_none() { p.restarts = 0; p.next_start = Some(Instant::now()); } } else { stop(p); }
        }
        !matched.is_empty()
    }

    pub fn restart(&mut self, name: &str) -> bool {
        let mut any = false;
        for p in self.procs.iter_mut().filter(|p| Self::matches(p, name)) {
            stop(p);
            p.wanted = true;
            p.restarts = 0;
            p.next_start = Some(Instant::now());
            any = true;
        }
        any
    }
}

/// Everything that should be running for this configuration.
pub fn plan(cfg: &Config) -> R<(Vec<Spec>, Vec<String>)> {
    let mut specs = Vec::new();
    let mut notes = Vec::new();
    for (name, sc) in &cfg.services {
        if !sc.enabled { continue; }
        let info = match services::info(name) { Ok(i) => i, Err(e) => { notes.push(e.to_string()); continue; } };
        let version = if sc.version.is_empty() { info.version.to_string() } else { sc.version.clone() };
        if !services::is_installed(name, &version) { notes.push(format!("{} is enabled but not installed (tez service install {name})", info.title)); continue; }
        match services::spec(name, sc) { Ok(s) => specs.push(s), Err(e) => notes.push(format!("{name}: {e}")) }
    }
    specs.extend(web::cgi_specs(cfg));
    if web::frankenphp_exe().exists() {
        web::write_caddyfile(cfg)?;
        let (_, _, port_notes) = web::effective_ports(cfg);
        notes.extend(port_notes);
        specs.push(web::spec(cfg));
    } else {
        notes.push("the web server is not installed (tez install)".into());
    }
    for site in sites::all(cfg) {
        if !site.settings.up { continue; }
        match project::specs(cfg, &site) { Ok(v) => specs.extend(v), Err(e) => notes.push(format!("{}: {e}", site.name)) }
    }
    Ok((specs, notes))
}

/// Re-read the configuration and apply it.
pub fn reload(shared: &Shared) -> R<Vec<String>> {
    let cfg = Config::load();
    crate::php::write_all_inis(&cfg)?;
    let (specs, notes) = plan(&cfg)?;
    let mut d = shared.lock().unwrap();
    let web_before = d.procs.iter().find(|p| p.spec.name == "web").map(|p| p.spec.clone());
    let web_running = d.procs.iter().any(|p| p.spec.name == "web" && p.state == State::Running);
    d.reconcile(specs);
    d.notes = notes.clone();
    let web_after = d.procs.iter().find(|p| p.spec.name == "web").map(|p| p.spec.clone());
    drop(d);
    // Same server, new sites: apply the Caddyfile in place.
    if web_running && web_before == web_after && web_after.is_some() {
        if let Err(e) = web::reload() {
            let mut d = shared.lock().unwrap();
            d.notes.push(e.to_string());
            d.restart("web");
        }
    }
    Ok(notes)
}

/// SIGTERM, SIGINT or SIGHUP: shut down the same way `tez stop` does.
fn shutdown_from_signal() {
    let _ = crate::api::call_port(Config::load().api_port, "POST", "/api/shutdown", Duration::from_secs(5));
}

pub fn run() -> R<()> {
    paths::ensure_dirs()?;
    let cfg = Config::load();
    // Anything a crashed daemon left behind would hold the ports this one needs.
    sys::kill_stale_groups();
    let pidfile = paths::run_dir().join("daemon.pid");
    std::fs::write(&pidfile, std::process::id().to_string())?;
    web::reset_ports();
    crate::php::write_all_inis(&cfg)?;
    let (specs, notes) = plan(&cfg)?;
    let shared: Shared = Arc::new(Mutex::new(Daemon { procs: Vec::new(), started: SystemTime::now(), notes }));
    shared.lock().unwrap().reconcile(specs);
    sys::on_shutdown_signal(shutdown_from_signal);
    crate::metrics::start(shared.clone());
    crate::stats::start(shared.clone());

    let ticker = shared.clone();
    std::thread::spawn(move || {
        let mut last: Vec<u32> = Vec::new();
        loop {
            let pids = { let mut d = ticker.lock().unwrap(); d.tick(); d.running_pids() };
            if pids != last { sys::remember_groups(&pids); last = pids; }
            std::thread::sleep(Duration::from_millis(250));
        }
    });

    crate::api::serve(shared.clone(), cfg.api_port)?;
    shared.lock().unwrap().stop_all();
    sys::remember_groups(&[]);
    let _ = std::fs::remove_file(pidfile);
    Ok(())
}

/// Start the daemon in the background, detached from this terminal.
pub fn spawn_background() -> R<u32> {
    let exe = std::env::current_exe()?;
    let installed = paths::bin().join(sys::exe("tez"));
    let exe = if installed.exists() { installed } else { exe };
    let log = OpenOptions::new().create(true).append(true).open(paths::logs_dir().join("daemon.log"))?;
    let mut cmd = Command::new(&exe);
    cmd.arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .current_dir(paths::home());
    // Windows: break away from the terminal's job (VS Code, Windows Terminal) so closing it keeps
    // tez running. macOS/Linux: a new session, so the terminal's Ctrl+C and hangup never reach it.
    let child: Child = sys::spawn_detached(&mut cmd)?;
    Ok(child.id())
}
