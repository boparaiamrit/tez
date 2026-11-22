//! Resource history for the dashboard: memory per group and CPU for everything tez runs,
//! sampled every 5 seconds and kept for 24 hours in memory, plus this computer's numbers.
//!
//! Sampling never starts a process on Windows (Job Object accounting and Win32 calls only);
//! on macOS and Linux it costs one `ps` call per sample.

use crate::daemon::Shared;
use crate::paths;
use crate::sys;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const EVERY: u64 = 5;
const KEEP: usize = (24 * 3600 / EVERY) as usize;
const SPARK: usize = 180; // 15 minutes of samples
const SPARK_POINTS: usize = 60;
const BUCKETS: u64 = 180;

#[derive(Clone, Copy, Default)]
struct Sample { t: u64, php: u64, sites: u64, services: u64, web: u64, cpu: f64 }

#[derive(Default)]
struct ProcHist { last: Option<(Instant, u64)>, cpu: f64, memory: u64, spark: VecDeque<u64> }

#[derive(Default, Clone)]
struct Machine { total: u64, available: u64, wsl: Option<u64>, disk_free: u64, disk_total: u64, cache: u64 }

#[derive(Default)]
struct Store {
    samples: VecDeque<Sample>,
    procs: HashMap<String, ProcHist>,
    machine: Machine,
    mem_at: Option<Instant>,
    wsl_at: Option<Instant>,
    disk_at: Option<Instant>,
    cache_at: Option<Instant>,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

pub fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

pub fn cpus() -> usize { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) }

fn round1(x: f64) -> f64 { (x * 10.0).round() / 10.0 }

fn due(at: Option<Instant>, every: Duration) -> bool { at.map(|a| a.elapsed() >= every).unwrap_or(true) }

/// Sample every 5 seconds for as long as the daemon runs.
pub fn start(shared: Shared) {
    std::thread::spawn(move || loop {
        sample(&shared);
        std::thread::sleep(Duration::from_secs(EVERY));
    });
}

fn sample(shared: &Shared) {
    // Read what is running, then let go of the daemon before doing anything slow.
    let rows: Vec<(String, String, u64, Option<u64>)> = {
        let d = shared.lock().unwrap();
        d.procs.iter().map(|p| (p.spec.name.clone(), p.spec.group.clone(), p.memory(), p.cpu_time())).collect()
    };
    let at = Instant::now();
    let ncpu = cpus() as f64;

    // Machine numbers on their own clocks, computed outside the store's lock.
    let (mem_due, wsl_due, disk_due, cache_due) = {
        let s = STORE.lock().unwrap();
        let mem_every = if cfg!(target_os = "macos") { Duration::from_secs(30) } else { Duration::from_secs(EVERY) };
        (due(s.mem_at, mem_every), due(s.wsl_at, Duration::from_secs(30)), due(s.disk_at, Duration::from_secs(30)), due(s.cache_at, Duration::from_secs(300)))
    };
    let mem = if mem_due { Some(sys::memory_status()) } else { None };
    let wsl = if wsl_due { Some(sys::wsl_memory()) } else { None };
    let disk = if disk_due { Some(sys::disk_space(&paths::home())) } else { None };
    let cache = if cache_due { Some(cache_size(&paths::cache_dir())) } else { None };

    let mut s = STORE.lock().unwrap();
    if let Some((total, available)) = mem { s.machine.total = total; s.machine.available = available; s.mem_at = Some(at); }
    if let Some(w) = wsl { s.machine.wsl = w; s.wsl_at = Some(at); }
    if let Some((free, total)) = disk { s.machine.disk_free = free; s.machine.disk_total = total; s.disk_at = Some(at); }
    if let Some(c) = cache { s.machine.cache = c; s.cache_at = Some(at); }

    let names: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    s.procs.retain(|k, _| names.contains(k));
    let mut sm = Sample { t: now(), ..Default::default() };
    for (name, group, memory, cpu_time) in rows {
        let h = s.procs.entry(name).or_default();
        let cpu = match (cpu_time, h.last) {
            (Some(c), Some((t0, c0))) if c >= c0 => {
                let wall = at.duration_since(t0).as_nanos() as f64;
                if wall > 0.0 { ((c - c0) as f64 / wall / ncpu * 100.0).clamp(0.0, 100.0) } else { 0.0 }
            }
            _ => 0.0,
        };
        h.last = cpu_time.map(|c| (at, c));
        h.cpu = cpu;
        h.memory = memory;
        h.spark.push_back(memory);
        while h.spark.len() > SPARK { h.spark.pop_front(); }
        match group.as_str() {
            "php" => sm.php += memory,
            "service" => sm.services += memory,
            "web" => sm.web += memory,
            g if g.starts_with("site:") => sm.sites += memory,
            _ => {}
        }
        sm.cpu += cpu;
    }
    sm.cpu = sm.cpu.min(100.0);
    s.samples.push_back(sm);
    while s.samples.len() > KEEP { s.samples.pop_front(); }
}

/// This process's current CPU share of the whole computer, in percent (one decimal).
pub fn cpu_of(name: &str) -> f64 { STORE.lock().unwrap().procs.get(name).map(|h| round1(h.cpu)).unwrap_or(0.0) }

pub fn range_secs(range: &str) -> (u64, &'static str) {
    match range { "15m" => (900, "15m"), "24h" => (86400, "24h"), _ => (3600, "1h") }
}

/// GET /api/metrics?range=15m|1h|24h
pub fn json(range: &str) -> Value {
    let (secs, label) = range_secs(range);
    let s = STORE.lock().unwrap();
    let now = now();
    let start = now.saturating_sub(secs);
    let width = (secs / BUCKETS).max(EVERY);
    let mut series: Vec<Value> = Vec::new();
    let mut acc: Option<(u64, u64, u64, u64, u64, u64, u64, f64, u64)> = None; // idx, t, php, sites, services, web, n, cpu, -
    let flush = |a: (u64, u64, u64, u64, u64, u64, u64, f64, u64), out: &mut Vec<Value>| {
        let n = a.6.max(1);
        out.push(json!({ "t": a.1 / n, "php": a.2 / n, "sites": a.3 / n, "services": a.4 / n, "web": a.5 / n, "cpu": round1(a.7 / n as f64) }));
    };
    for x in s.samples.iter().filter(|x| x.t >= start) {
        let idx = (x.t - start) / width;
        match acc.as_mut() {
            Some(a) if a.0 == idx => { a.1 += x.t; a.2 += x.php; a.3 += x.sites; a.4 += x.services; a.5 += x.web; a.6 += 1; a.7 += x.cpu; }
            _ => {
                if let Some(a) = acc.take() { flush(a, &mut series); }
                acc = Some((idx, x.t, x.php, x.sites, x.services, x.web, 1, x.cpu, 0));
            }
        }
    }
    if let Some(a) = acc.take() { flush(a, &mut series); }
    let mut procs = serde_json::Map::new();
    for (name, h) in &s.procs {
        procs.insert(name.clone(), json!({ "cpu": round1(h.cpu), "memory": h.memory, "spark": downsample(&h.spark, SPARK_POINTS) }));
    }
    let m = &s.machine;
    json!({
        "now": now, "range": label,
        "machine": { "total": m.total, "available": m.available, "wsl": m.wsl, "disk_free": m.disk_free, "disk_total": m.disk_total, "cache": m.cache, "cpus": cpus() },
        "series": series,
        "procs": procs,
    })
}

/// Average consecutive values so at most `points` remain (oldest first).
pub fn downsample(v: &VecDeque<u64>, points: usize) -> Vec<u64> {
    if v.len() <= points { return v.iter().copied().collect(); }
    let per = v.len().div_ceil(points);
    let all: Vec<u64> = v.iter().copied().collect();
    all.chunks(per).map(|c| c.iter().sum::<u64>() / c.len() as u64).collect()
}

// ------------------------------------------------------------------ the download cache

fn dir_size(p: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(p) else { return 0 };
    if !meta.is_dir() { return meta.len(); }
    std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| dir_size(&e.path())).sum()).unwrap_or(0)
}

/// Bytes in the download cache. The OPcache file cache lives in it too (cache/opcache) and is
/// not counted: PHP needs it, and clearing it would only slow the next requests down.
pub fn cache_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map(|rd| rd.flatten().filter(|e| e.file_name() != "opcache").map(|e| dir_size(&e.path())).sum()).unwrap_or(0)
}

/// Delete the download cache (never cache/opcache, never anything touched in the last two
/// minutes, which may be a download in progress). Returns the bytes freed.
pub fn clear_dir(dir: &Path) -> u64 {
    let mut freed = 0;
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    for e in rd.flatten() {
        if e.file_name() == "opcache" { continue; }
        let path = e.path();
        let recent = std::fs::symlink_metadata(&path).and_then(|m| m.modified()).ok()
            .and_then(|t| t.elapsed().ok()).map(|age| age < Duration::from_secs(120)).unwrap_or(true);
        if recent { continue; }
        let size = dir_size(&path);
        let gone = if std::fs::symlink_metadata(&path).map(|m| m.is_dir()).unwrap_or(false) { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        if gone.is_ok() { freed += size; }
    }
    freed
}

/// POST /api/cache/clear
pub fn clear_cache() -> u64 {
    let dir = paths::cache_dir();
    let freed = clear_dir(&dir);
    let size = cache_size(&dir);
    let mut s = STORE.lock().unwrap();
    s.machine.cache = size;
    s.cache_at = Some(Instant::now());
    freed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_keeps_opcache_and_fresh_files() {
        let dir = std::env::temp_dir().join(format!("tez-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("opcache").join("8.4")).unwrap();
        std::fs::write(dir.join("opcache").join("8.4").join("x.bin"), vec![0u8; 100]).unwrap();
        std::fs::write(dir.join("php-8.4.zip"), vec![0u8; 1000]).unwrap();
        assert_eq!(cache_size(&dir), 1000);
        // Fresh files are left alone (a download may be running).
        assert_eq!(clear_dir(&dir), 0);
        assert!(dir.join("php-8.4.zip").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn downsampling_averages() {
        let v: VecDeque<u64> = (0..180).collect();
        let d = downsample(&v, 60);
        assert_eq!(d.len(), 60);
        assert_eq!(d[0], 1);
        let short: VecDeque<u64> = (0..10).collect();
        assert_eq!(downsample(&short, 60).len(), 10);
    }
}
