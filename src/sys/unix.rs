//! macOS and Linux: process groups and signals, declared by hand (no libc crate).
//!
//! Every process tez starts leads its own process group, so stopping it ends everything it
//! started too (php-fpm workers, postgres backends, node). The daemon stops every group on
//! SIGTERM, SIGINT and SIGHUP, and on start it ends groups a crashed daemon left behind.

#![allow(dead_code)]

use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGKILL: i32 = 9;
const SIGTERM: i32 = 15;
const EPERM: i32 = 1;

unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn signal(sig: i32, handler: usize) -> usize;
    fn setsid() -> i32;
    fn geteuid() -> u32;
}

/// A managed process's group (the child leads it, so the group id is the child's pid).
pub struct Job(i32);

impl Job {
    pub fn attach(child: &Child) -> Option<Job> { Some(Job(child.id() as i32)) }

    pub fn terminate(&self) { signal_group(self.0, SIGKILL); }

    pub fn pids(&self) -> Vec<u32> { ps_table().iter().filter(|r| r.pgid == self.0).map(|r| r.pid).collect() }

    /// Resident memory of every process in the group, in bytes.
    pub fn memory(&self) -> u64 { ps_table().iter().filter(|r| r.pgid == self.0).map(|r| r.rss).sum() }
}

impl Drop for Job {
    /// Like a Windows job closing: whatever the process left running in its group ends with it.
    fn drop(&mut self) { signal_group(self.0, SIGKILL); }
}

fn signal_group(pgid: i32, sig: i32) {
    if pgid <= 1 { return; }
    // SAFETY: kill(2) with a negative pid signals one process group; it has no memory effects.
    unsafe { kill(-pgid, sig); }
}

#[derive(Clone)]
struct Row { pid: u32, pgid: i32, rss: u64 }

static PS: Mutex<Option<(Instant, Vec<Row>)>> = Mutex::new(None);

/// One `ps` call for every process (pid, group, resident KB), reused for a second.
fn ps_table() -> Vec<Row> {
    let mut cache = PS.lock().unwrap();
    if let Some((at, rows)) = cache.as_ref() {
        if at.elapsed() < Duration::from_secs(1) { return rows.clone(); }
    }
    let out = Command::new("ps").args(["-A", "-o", "pid=,pgid=,rss="]).stdin(Stdio::null()).stderr(Stdio::null()).output();
    let rows: Vec<Row> = out.map(|o| String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| {
        let mut it = l.split_whitespace();
        let pid = it.next()?.parse().ok()?;
        let pgid = it.next()?.parse().ok()?;
        let rss: u64 = it.next()?.parse().ok()?;
        Some(Row { pid, pgid, rss: rss * 1024 })
    }).collect()).unwrap_or_default();
    *cache = Some((Instant::now(), rows.clone()));
    rows
}

pub fn process_memory(pid: u32) -> u64 { ps_table().iter().find(|r| r.pid == pid).map(|r| r.rss).unwrap_or(0) }

pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 { return false; }
    // SAFETY: signal 0 only checks that the process exists and may be signalled.
    let r = unsafe { kill(pid as i32, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(EPERM)
}

/// The php and composer shims replace themselves with PHP (exec), so Ctrl+C reaches PHP directly.
pub fn ignore_ctrl_c() {}

/// Nothing to do: Rust opens every file close-on-exec, and children get explicit stdio.
pub fn stop_std_inheritance() {}

/// Bytes free for this user on the volume holding `path` (from `df -Pk`).
pub fn free_bytes(path: &std::path::Path) -> u64 {
    let mut p = path.to_path_buf();
    while !p.exists() { if !p.pop() { break; } }
    let out = Command::new("df").arg("-Pk").arg(&p).stdin(Stdio::null()).stderr(Stdio::null()).output();
    let Ok(out) = out else { return u64::MAX };
    let text = String::from_utf8_lossy(&out.stdout);
    // Filesystem 1024-blocks Used Available Capacity Mounted-on
    text.lines().nth(1).and_then(|l| l.split_whitespace().nth(3)).and_then(|v| v.parse::<u64>().ok()).map(|kb| kb * 1024).unwrap_or(u64::MAX)
}

/// Run a command as root through sudo (it asks for the password on the terminal) and wait.
/// TEZ_HOME is passed on so the root process works on this user's tez, not root's.
pub fn run_elevated(exe: &std::path::Path, args: &[String]) -> crate::util::R<i32> {
    let status = Command::new("sudo")
        .arg("env")
        .arg(format!("TEZ_HOME={}", crate::paths::home().display()))
        .arg(exe)
        .args(args)
        .status()?;
    Ok(status.code().unwrap_or(1))
}

pub fn is_elevated() -> bool {
    // SAFETY: geteuid(2) cannot fail and touches no memory.
    unsafe { geteuid() == 0 }
}

// ------------------------------------------------------------------ the API every OS provides

/// Set up a managed child before it starts: it leads a new process group.
pub fn prepare_child(cmd: &mut Command) { cmd.process_group(0); }

/// End a managed process and everything in its group: SIGTERM, a short grace period, SIGKILL.
pub fn kill_tree(job: Option<Job>, child: Option<Child>) {
    let pgid = job.as_ref().map(|j| j.0).or_else(|| child.as_ref().map(|c| c.id() as i32));
    if let Some(g) = pgid { signal_group(g, SIGTERM); }
    if let Some(mut c) = child {
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if let Ok(Some(_)) = c.try_wait() { break; }
            std::thread::sleep(Duration::from_millis(50));
        }
        if let Some(g) = pgid { signal_group(g, SIGKILL); }
        let _ = c.kill();
        let _ = c.wait();
    }
    drop(job);
}

/// Start a program that outlives this one (the daemon, the tray app) in its own session, so
/// closing the terminal or pressing Ctrl+C in it leaves the program running.
pub fn spawn_detached(cmd: &mut Command) -> std::io::Result<Child> {
    // SAFETY: setsid(2) is async-signal-safe, which is all pre_exec requires.
    unsafe {
        cmd.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    cmd.spawn()
}

/// Open a URL in the default browser.
pub fn open_url(url: &str) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    Command::new(opener).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ())
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: i32) { STOP.store(true, Ordering::SeqCst); }

/// Call `f` (once, on a normal thread) when the daemon gets SIGTERM, SIGINT or SIGHUP.
pub fn on_shutdown_signal(f: fn()) {
    for sig in [SIGTERM, SIGINT, SIGHUP] {
        // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
        unsafe { signal(sig, on_signal as extern "C" fn(i32) as usize); }
    }
    std::thread::spawn(move || loop {
        if STOP.load(Ordering::SeqCst) { f(); return; }
        std::thread::sleep(Duration::from_millis(200));
    });
}

fn groups_file() -> std::path::PathBuf { crate::paths::run_dir().join("groups") }

fn start_time(pid: u32) -> Option<String> {
    let out = Command::new("ps").args(["-o", "lstart=", "-p", &pid.to_string()]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if t.is_empty() { None } else { Some(t) }
}

/// Note the running groups (with each leader's start time), so a later daemon can end them if
/// this one dies without stopping them.
pub fn remember_groups(pids: &[u32]) {
    let mut lines = Vec::new();
    for &p in pids {
        if let Some(t) = start_time(p) { lines.push(format!("{p}\t{t}")); }
    }
    let _ = std::fs::write(groups_file(), lines.join("\n"));
}

/// End groups a previous daemon left running (it crashed or was killed). A group is only ended
/// when its leader is still the same process (same start time), never a reused pid.
pub fn kill_stale_groups() {
    let Ok(text) = std::fs::read_to_string(groups_file()) else { return };
    for line in text.lines() {
        let Some((pid, t)) = line.split_once('\t') else { continue };
        let Ok(pid) = pid.parse::<u32>() else { continue };
        if pid_alive(pid) && start_time(pid).as_deref() == Some(t) {
            signal_group(pid as i32, SIGTERM);
            std::thread::sleep(Duration::from_millis(500));
            signal_group(pid as i32, SIGKILL);
        }
    }
    let _ = std::fs::remove_file(groups_file());
}
