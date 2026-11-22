//! Per-site performance and health for the dashboard, read incrementally every 5 seconds:
//! - requests from each site's Caddy JSON access log (rate, latency percentiles, errors, routes)
//! - queue throughput from the site's Horizon or queue:work output
//! - errors in the site's newest storage/logs/laravel*.log
//! - service stats (PostgreSQL, Redis, Mailpit, Meilisearch), spoken to over TCP where possible.
//!
//! Everything is kept in memory for 24 hours; nothing is written anywhere.

use crate::config::Config;
use crate::daemon::{self, Shared, State};
use crate::metrics::{now, range_secs};
use crate::sys::{self, Hidden};
use crate::{paths, services, sites};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const DAY_MINUTES: u64 = 1440;
const POINTS: u64 = 120;
/// Latency histogram upper bounds in ms; the last bucket holds everything slower.
const BOUNDS: [f64; 20] = [2., 5., 10., 20., 30., 50., 75., 100., 150., 200., 300., 500., 750., 1000., 1500., 2000., 3000., 5000., 10000., 30000.];
const HIST: usize = BOUNDS.len() + 1;

#[derive(Clone, Default)]
struct Minute { m: u64, req: u32, e4xx: u32, e5xx: u32, hist: [u32; HIST] }

#[derive(Clone)]
struct Req { t: u64, route: String, ms: f64 }

#[derive(Clone)]
struct ServerError { t: u64, status: u16, method: String, path: String, ms: f64 }

#[derive(Clone, Default)]
struct QMinute { m: u64, done: u32, failed: u32 }

#[derive(Clone)]
struct QJob { t: u64, name: String, ms: f64, failed: bool }

#[derive(Default)]
struct Queue { minutes: VecDeque<QMinute>, jobs: VecDeque<QJob> }

#[derive(Default)]
struct AppLog { errors: VecDeque<u64>, last: Option<(u64, String)> }

#[derive(Default)]
struct SiteStats { minutes: VecDeque<Minute>, recent: VecDeque<Req>, errors: VecDeque<ServerError>, queue: Option<Queue>, log: AppLog }

#[derive(Default)]
struct Store { sites: HashMap<String, SiteStats>, services: BTreeMap<String, Value> }

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

// ------------------------------------------------------------------ reading logs incrementally

struct Tail { offset: u64, rest: String, skip_first: bool }

/// New complete lines in `path` since the last call. On first sight it starts `back` bytes from
/// the end (0 = only new lines). A file that got shorter was rotated, so reading starts over.
fn read_new(tails: &mut HashMap<PathBuf, Tail>, path: &Path, back: u64) -> Vec<String> {
    let Ok(meta) = std::fs::metadata(path) else { return Vec::new() };
    let len = meta.len();
    let tail = tails.entry(path.to_path_buf()).or_insert_with(|| {
        let offset = len.saturating_sub(back);
        // Starting mid-file (but not at its end): the first piece is probably half a line.
        Tail { offset, rest: String::new(), skip_first: offset > 0 && offset < len }
    });
    if len < tail.offset { tail.offset = 0; tail.rest.clear(); tail.skip_first = false; }
    if len == tail.offset { return Vec::new(); }
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
    if f.seek(SeekFrom::Start(tail.offset)).is_err() { return Vec::new(); }
    let want = (len - tail.offset).min(8 << 20);
    let mut buf = Vec::with_capacity(want as usize);
    if f.take(want).read_to_end(&mut buf).is_err() { return Vec::new(); }
    tail.offset += buf.len() as u64;
    let text = format!("{}{}", std::mem::take(&mut tail.rest), String::from_utf8_lossy(&buf));
    let mut lines: Vec<String> = text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
    tail.rest = lines.pop().unwrap_or_default();
    if tail.skip_first && !lines.is_empty() { lines.remove(0); tail.skip_first = false; }
    lines.retain(|l| !l.trim().is_empty());
    lines
}

pub fn access_log(site: &str) -> PathBuf { paths::logs_dir().join(format!("access-{}.log", site.replace(['/', '\\', ':'], "-"))) }

// ------------------------------------------------------------------ requests

pub fn is_asset(path: &str) -> bool {
    let p = path.split('?').next().unwrap_or(path).to_ascii_lowercase();
    if p.starts_with("/build/") || p.starts_with("/@") || p.starts_with("/favicon") || p.starts_with("/node_modules/") { return true; }
    [".css", ".js", ".mjs", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".woff", ".woff2", ".ttf", ".ico", ".map"].iter().any(|e| p.ends_with(e))
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36 && s.char_indices().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() })
}

/// The route a request belongs to: no query string, numeric and UUID segments as {id}.
pub fn route(path: &str) -> String {
    let p = path.split('?').next().unwrap_or("/");
    let r = p.split('/').map(|s| if !s.is_empty() && (s.chars().all(|c| c.is_ascii_digit()) || is_uuid(s)) { "{id}" } else { s }).collect::<Vec<_>>().join("/");
    if r.is_empty() { "/".into() } else { r }
}

/// One Caddy JSON access-log line: (unix time, method, uri, status, ms).
pub fn parse_access(line: &str) -> Option<(f64, String, String, u16, f64)> {
    let v: Value = serde_json::from_str(line).ok()?;
    let req = &v["request"];
    let method = req["method"].as_str()?.to_string();
    let uri = req["uri"].as_str()?.to_string();
    let status = v["status"].as_u64()? as u16;
    let secs = v["duration"].as_f64().unwrap_or(0.0);
    let ts = v["ts"].as_f64().unwrap_or(now() as f64);
    Some((ts, method, uri, status, secs * 1000.0))
}

fn bucket_of(ms: f64) -> usize { BOUNDS.iter().position(|b| ms <= *b).unwrap_or(BOUNDS.len()) }

/// A percentile (0..1) read from a latency histogram, interpolated inside its bucket.
fn percentile(hist: &[u32; HIST], p: f64) -> f64 {
    let total: u32 = hist.iter().sum();
    if total == 0 { return 0.0; }
    let target = (p * total as f64).ceil().max(1.0);
    let mut cum = 0.0;
    for (i, &n) in hist.iter().enumerate() {
        if n == 0 { continue; }
        let before = cum;
        cum += n as f64;
        if cum >= target {
            let lo = if i == 0 { 0.0 } else { BOUNDS[i - 1] };
            let hi = if i < BOUNDS.len() { BOUNDS[i] } else { BOUNDS[BOUNDS.len() - 1] * 2.0 };
            return lo + (hi - lo) * ((target - before) / n as f64);
        }
    }
    BOUNDS[BOUNDS.len() - 1]
}

fn minute_mut(minutes: &mut VecDeque<Minute>, m: u64) -> &mut Minute {
    let pos = minutes.iter().rposition(|x| x.m <= m);
    match pos {
        Some(i) if minutes[i].m == m => &mut minutes[i],
        Some(i) => { minutes.insert(i + 1, Minute { m, ..Default::default() }); &mut minutes[i + 1] }
        None => { minutes.push_front(Minute { m, ..Default::default() }); &mut minutes[0] }
    }
}

fn add_request(s: &mut SiteStats, ts: f64, method: &str, uri: &str, status: u16, ms: f64) {
    if is_asset(uri) { return; }
    let t = ts as u64;
    let mm = minute_mut(&mut s.minutes, t / 60);
    mm.req += 1;
    if (400..500).contains(&status) { mm.e4xx += 1; }
    if status >= 500 { mm.e5xx += 1; }
    mm.hist[bucket_of(ms)] += 1;
    s.recent.push_back(Req { t, route: format!("{method} {}", route(uri)), ms });
    if status >= 500 {
        s.errors.push_back(ServerError { t, status, method: method.to_string(), path: uri.split('?').next().unwrap_or(uri).to_string(), ms });
        while s.errors.len() > 20 { s.errors.pop_front(); }
    }
}

// ------------------------------------------------------------------ queue output

fn is_date(t: &str) -> bool { t.len() == 10 && t.as_bytes()[4] == b'-' && t.as_bytes()[7] == b'-' && t.chars().filter(|c| c.is_ascii_digit()).count() == 8 }
fn is_time(t: &str) -> bool { t.len() >= 8 && t.as_bytes()[2] == b':' && t.as_bytes()[5] == b':' }

fn parse_duration_ms(t: &str) -> Option<f64> {
    let t = t.replace(',', "");
    if let Some(v) = t.strip_suffix("ms") { return v.parse().ok(); }
    if let Some(v) = t.strip_suffix('s') { return v.parse::<f64>().ok().map(|s| s * 1000.0); }
    None
}

/// A Laravel queue worker line ("2026-11-22 07:14:31 App\Jobs\X ...... 186.42ms DONE"):
/// (job class, duration in ms, failed). RUNNING lines and anything else give None.
pub fn parse_queue(line: &str) -> Option<(String, f64, bool)> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let failed = match *tokens.last()? { "DONE" => false, "FAIL" | "FAILED" => true, _ => return None };
    let mut i = 0;
    if tokens.get(i).is_some_and(|t| is_date(t)) { i += 1; }
    if tokens.get(i).is_some_and(|t| is_time(t)) { i += 1; }
    let class = *tokens.get(i)?;
    if class.chars().all(|c| c == '.') || i + 1 >= tokens.len() { return None; }
    let ms = tokens.len().checked_sub(2).and_then(|j| tokens.get(j)).and_then(|t| parse_duration_ms(t)).unwrap_or(0.0);
    Some((class.to_string(), ms, failed))
}

fn add_job(q: &mut Queue, t: u64, name: String, ms: f64, failed: bool) {
    let m = t / 60;
    match q.minutes.back_mut() {
        Some(x) if x.m == m => { if failed { x.failed += 1 } else { x.done += 1 } }
        _ => q.minutes.push_back(QMinute { m, done: u32::from(!failed), failed: u32::from(failed) }),
    }
    q.jobs.push_back(QJob { t, name, ms, failed });
}

// ------------------------------------------------------------------ the app's own log

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// "[2026-11-22 07:14:31] local.ERROR: message ..." → (unix time, taken as UTC, Laravel's default; message).
pub fn parse_log_error(line: &str) -> Option<(Option<u64>, String)> {
    if !line.starts_with('[') { return None; }
    let close = line.find("] ")?;
    let after = &line[close + 2..];
    let colon = after.find(": ")?;
    let level = after[..colon].rsplit('.').next()?;
    if !matches!(level, "ERROR" | "CRITICAL" | "EMERGENCY") || after[..colon].contains(' ') { return None; }
    let stamp = &line[1..close];
    let t = (|| {
        let (date, time) = stamp.get(..19)?.split_once(' ')?;
        let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
        let (y, mo, da) = (d.next()??, d.next()??, d.next()??);
        let mut h = time.split(':').map(|x| x.parse::<i64>().ok());
        let (hh, mi, ss) = (h.next()??, h.next()??, h.next()??);
        Some((days_from_civil(y, mo, da) * 86400 + hh * 3600 + mi * 60 + ss) as u64)
    })();
    let msg: String = after[colon + 2..].chars().take(300).collect();
    Some((t, msg.trim().to_string()))
}

fn newest_laravel_log(site: &Path) -> Option<PathBuf> {
    let dir = site.join("storage").join("logs");
    std::fs::read_dir(dir).ok()?.flatten()
        .filter(|e| { let n = e.file_name().to_string_lossy().to_string(); n.starts_with("laravel") && n.ends_with(".log") })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

// ------------------------------------------------------------------ services

fn tcp(port: u16) -> Option<TcpStream> {
    let s = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(500)).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    Some(s)
}

/// A small HTTP GET to a local service; returns the body as JSON.
fn http_json(port: u16, path: &str, bearer: Option<&str>) -> Option<Value> {
    let mut s = tcp(port)?;
    let auth = bearer.filter(|k| !k.is_empty()).map(|k| format!("Authorization: Bearer {k}\r\n")).unwrap_or_default();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Accept: application/json\r\nConnection: close\r\n\r\n").as_bytes()).ok()?;
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).to_string();
    let (head, body) = text.split_once("\r\n\r\n")?;
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") { return None; }
    let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") { dechunk(body) } else { body.to_string() };
    serde_json::from_str(&body).ok()
}

fn dechunk(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some((size, after)) = rest.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 || after.len() < n { break; }
        out.push_str(&after[..n]);
        rest = after[n..].trim_start_matches("\r\n");
    }
    out
}

/// Redis INFO over RESP.
fn redis_info(port: u16, password: &str) -> Option<String> {
    let mut s = tcp(port)?;
    let cmd = |args: &[&str]| { let mut c = format!("*{}\r\n", args.len()); for a in args { c.push_str(&format!("${}\r\n{a}\r\n", a.len())); } c };
    if !password.is_empty() {
        s.write_all(cmd(&["AUTH", password]).as_bytes()).ok()?;
        let mut b = [0u8; 64];
        let n = s.read(&mut b).ok()?;
        if !b[..n].starts_with(b"+OK") { return None; }
    }
    s.write_all(cmd(&["INFO"]).as_bytes()).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 { break; }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf);
        if let Some(rest) = text.strip_prefix('$') {
            if let Some((len, body)) = rest.split_once("\r\n") {
                if let Ok(len) = len.parse::<usize>() { if body.len() >= len { return Some(body[..len].to_string()); } }
            }
        } else if text.starts_with('-') { return None; }
    }
    None
}

pub fn parse_redis_info(info: &str) -> Value {
    let get = |k: &str| info.lines().find_map(|l| l.strip_prefix(&format!("{k}:"))).map(|v| v.trim().to_string());
    let num = |k: &str| get(k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let keys: u64 = info.lines().filter(|l| l.starts_with("db") && l.contains(":keys="))
        .filter_map(|l| l.split("keys=").nth(1)).filter_map(|v| v.split(',').next()).filter_map(|v| v.parse::<u64>().ok()).sum();
    let (hits, misses) = (num("keyspace_hits"), num("keyspace_misses"));
    let hit_rate = if hits + misses == 0 { Value::Null } else { json!(((hits as f64 / (hits + misses) as f64) * 1000.0).round() / 1000.0) };
    json!({ "used_memory": num("used_memory"), "keys": keys, "ops_per_sec": num("instantaneous_ops_per_sec"), "hit_rate": hit_rate })
}

fn postgres_stats(key: &str, cfg: &Config) -> Option<Value> {
    let sc = cfg.services.get(key).cloned().unwrap_or_default();
    let version = services::version_of(key, &sc);
    let port = services::port_of(key, &sc);
    let psql = services::dir(key, &version).join("bin").join(sys::exe("psql"));
    let out = std::process::Command::new(psql)
        .args(["-h", "127.0.0.1", "-p", &port.to_string(), "-U", "postgres", "-d", "postgres", "-tA", "-F", "|",
            "-c", "select datname, pg_database_size(datname) from pg_database where not datistemplate order by 2 desc",
            "-c", "select count(*) from pg_stat_activity where state = 'active' and pid <> pg_backend_pid()"])
        .hidden().stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    if !out.status.success() { return None; }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut dbs = Vec::new();
    let mut active = 0u64;
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match l.split_once('|') {
            Some((name, size)) => dbs.push(json!({ "name": name, "size_bytes": size.parse::<u64>().unwrap_or(0) })),
            None => active = l.parse().unwrap_or(0),
        }
    }
    Some(json!({ "databases": dbs, "connections": active }))
}

fn service_stats(key: &str, cfg: &Config, url_port: Option<u16>) -> Option<Value> {
    let sc = cfg.services.get(key).cloned().unwrap_or_default();
    let port = services::port_of(key, &sc);
    let opt = |k: &str| sc.options.get(k).cloned().unwrap_or_default();
    let mut v = match services::kind(key) {
        "postgres" => postgres_stats(key, cfg)?,
        "redis" => parse_redis_info(&redis_info(port, &opt("requirepass"))?),
        "mailpit" => {
            let r = http_json(url_port?, "/api/v1/messages?limit=1", None)?;
            json!({ "total": r["total"].as_u64().unwrap_or(0), "unread": r["unread"].as_u64().unwrap_or(0) })
        }
        "meilisearch" => {
            let r = http_json(port, "/stats", Some(&opt("master-key")))?;
            let idx = r["indexes"].as_object().cloned().unwrap_or_default();
            let docs: u64 = idx.values().filter_map(|i| i["numberOfDocuments"].as_u64()).sum();
            json!({ "indexes": idx.len(), "documents": docs, "database_size": r["databaseSize"].as_u64().unwrap_or(0) })
        }
        _ => return None,
    };
    v["t"] = json!(now());
    Some(v)
}

// ------------------------------------------------------------------ the reader thread

pub fn start(shared: Shared) {
    std::thread::spawn(move || {
        let mut tails: HashMap<PathBuf, Tail> = HashMap::new();
        let mut log_at: HashMap<String, Instant> = HashMap::new();
        let mut svc_at: HashMap<String, Instant> = HashMap::new();
        loop {
            collect(&shared, &mut tails, &mut log_at, &mut svc_at);
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

fn collect(shared: &Shared, tails: &mut HashMap<PathBuf, Tail>, log_at: &mut HashMap<String, Instant>, svc_at: &mut HashMap<String, Instant>) {
    let cfg = Config::load();
    let site_list = sites::all(&cfg);
    let running: Vec<(String, Option<u16>)> = {
        let d = shared.lock().unwrap();
        d.procs.iter().filter(|p| p.spec.group == "service" && p.state == State::Running)
            .map(|p| (p.spec.name.clone(), p.spec.url.as_deref().and_then(|u| u.rsplit(':').next()).and_then(|x| x.trim_end_matches('/').parse().ok())))
            .collect()
    };
    let t_now = now();

    // Read everything new before touching the store.
    struct SiteRead { name: String, access: Vec<String>, queue: Option<Vec<String>>, log: Option<Vec<(Option<u64>, String)>> }
    let mut reads = Vec::new();
    for site in &site_list {
        // Up to 4 MB of history on first sight: the access log has its own timestamps.
        let access = read_new(tails, &access_log(&site.name), 4 << 20);
        let qlog = ["horizon", "queue"].iter().map(|p| daemon::log_path(&format!("{}/{p}", site.name))).find(|p| p.exists());
        let queue = qlog.map(|p| read_new(tails, &p, 0));
        let log = if log_at.get(&site.name).map(|a| a.elapsed() >= Duration::from_secs(15)).unwrap_or(true) {
            log_at.insert(site.name.clone(), Instant::now());
            newest_laravel_log(&site.path).map(|p| read_new(tails, &p, 256 << 10).iter().filter_map(|l| parse_log_error(l)).collect())
        } else { None };
        reads.push(SiteRead { name: site.name.clone(), access, queue, log });
    }
    let mut svc_updates: Vec<(String, Option<Value>)> = Vec::new();
    for (key, url_port) in &running {
        let every = if services::kind(key) == "postgres" { 30 } else { 15 };
        if svc_at.get(key).map(|a| a.elapsed() >= Duration::from_secs(every)).unwrap_or(true) {
            svc_at.insert(key.clone(), Instant::now());
            svc_updates.push((key.clone(), service_stats(key, &cfg, *url_port)));
        }
    }

    let mut store = STORE.lock().unwrap();
    let names: Vec<String> = site_list.iter().map(|s| s.name.clone()).collect();
    store.sites.retain(|k, _| names.contains(k));
    for r in reads {
        let s = store.sites.entry(r.name).or_default();
        for l in &r.access {
            if let Some((ts, method, uri, status, ms)) = parse_access(l) { add_request(s, ts, &method, &uri, status, ms); }
        }
        match r.queue {
            Some(lines) => {
                let q = s.queue.get_or_insert_with(Queue::default);
                for l in &lines { if let Some((name, ms, failed)) = parse_queue(l) { add_job(q, t_now, name, ms, failed); } }
            }
            None => s.queue = None,
        }
        if let Some(entries) = r.log {
            for (t, msg) in entries {
                let t = t.unwrap_or(t_now);
                s.log.errors.push_back(t);
                if s.log.last.as_ref().map(|(lt, _)| t >= *lt).unwrap_or(true) { s.log.last = Some((t, msg)); }
            }
        }
        // Forget what is too old.
        let day = t_now / 60 - DAY_MINUTES;
        while s.minutes.front().is_some_and(|m| m.m < day) { s.minutes.pop_front(); }
        while s.recent.front().is_some_and(|r| r.t + 3600 < t_now) || s.recent.len() > 50_000 { s.recent.pop_front(); }
        if let Some(q) = s.queue.as_mut() {
            while q.minutes.front().is_some_and(|m| m.m < day) { q.minutes.pop_front(); }
            while q.jobs.front().is_some_and(|j| j.t + 3600 < t_now) || q.jobs.len() > 50_000 { q.jobs.pop_front(); }
        }
        s.log.errors.retain(|t| t + 3600 >= t_now);
    }
    let keys: Vec<String> = running.iter().map(|(k, _)| k.clone()).collect();
    store.services.retain(|k, _| keys.contains(k));
    for (k, v) in svc_updates {
        match v { Some(v) => { store.services.insert(k, v); } None => { store.services.remove(&k); } }
    }
}

// ------------------------------------------------------------------ the API

fn r1(x: f64) -> f64 { (x * 10.0).round() / 10.0 }

fn p95_of(mut v: Vec<f64>) -> f64 {
    if v.is_empty() { return 0.0; }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[((v.len() as f64 * 0.95).ceil() as usize).saturating_sub(1).min(v.len() - 1)]
}

/// GET /api/sites/metrics?range=15m|1h|24h
pub fn sites_json(range: &str) -> Value {
    let (secs, _) = range_secs(range);
    let t_now = now();
    let now_m = t_now / 60;
    let span = secs / 60;
    let width = span.div_ceil(POINTS).max(1);
    let first = now_m + 1 - span;
    let store = STORE.lock().unwrap();
    let mut out = serde_json::Map::new();
    for (name, s) in &store.sites {
        // The last five minutes.
        let mut hist5 = [0u32; HIST];
        let (mut req5, mut e5) = (0u32, 0u32);
        for m in s.minutes.iter().filter(|m| m.m + 5 > now_m) {
            req5 += m.req;
            e5 += m.e5xx;
            for (i, n) in m.hist.iter().enumerate() { hist5[i] += n; }
        }
        let mut series = Vec::new();
        let mut b = first;
        while b <= now_m {
            let end = b + width;
            let mut h = [0u32; HIST];
            let (mut req, mut e4, mut e5b) = (0u32, 0u32, 0u32);
            for m in s.minutes.iter().filter(|m| m.m >= b && m.m < end) {
                req += m.req; e4 += m.e4xx; e5b += m.e5xx;
                for (i, n) in m.hist.iter().enumerate() { h[i] += n; }
            }
            series.push(json!({ "t": b * 60, "req": req, "p50_ms": r1(percentile(&h, 0.5)), "p95_ms": r1(percentile(&h, 0.95)), "e4xx": e4, "e5xx": e5b }));
            b = end;
        }
        let mut by_route: HashMap<&str, Vec<f64>> = HashMap::new();
        for r in s.recent.iter().filter(|r| r.t + 3600 >= t_now) { by_route.entry(r.route.as_str()).or_default().push(r.ms); }
        let mut routes: Vec<(String, String, usize, f64, f64)> = by_route.into_iter().map(|(k, v)| {
            let (method, path) = k.split_once(' ').unwrap_or(("GET", k));
            let max = v.iter().cloned().fold(0.0, f64::max);
            (method.to_string(), path.to_string(), v.len(), p95_of(v), max)
        }).collect();
        routes.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
        routes.truncate(10);
        let queue = s.queue.as_ref().map(|q| {
            let recent: u32 = q.minutes.iter().filter(|m| m.m + 5 > now_m).map(|m| m.done + m.failed).sum();
            let failed_1h = q.jobs.iter().filter(|j| j.failed && j.t + 3600 >= t_now).count();
            let mut qs = Vec::new();
            let mut b = first;
            while b <= now_m {
                let end = b + width;
                let (d, f) = q.minutes.iter().filter(|m| m.m >= b && m.m < end).fold((0, 0), |a, m| (a.0 + m.done, a.1 + m.failed));
                qs.push(json!({ "t": b * 60, "done": d, "failed": f }));
                b = end;
            }
            let mut by_job: BTreeMap<&str, (usize, usize, f64, f64)> = BTreeMap::new();
            for j in q.jobs.iter().filter(|j| j.t + 3600 >= t_now) {
                let e = by_job.entry(j.name.as_str()).or_default();
                e.0 += 1;
                if j.failed { e.1 += 1; }
                e.2 += j.ms;
                e.3 = e.3.max(j.ms);
            }
            let mut jobs: Vec<Value> = by_job.into_iter().map(|(n, (c, f, sum, max))| json!({ "name": n, "count": c, "failed": f, "avg_ms": r1(sum / c.max(1) as f64), "max_ms": r1(max) })).collect();
            jobs.sort_by(|a, b| b["count"].as_u64().cmp(&a["count"].as_u64()));
            jobs.truncate(20);
            json!({ "jpm": r1(recent as f64 / 5.0), "failed_1h": failed_1h, "series": qs, "jobs": jobs })
        });
        out.insert(name.clone(), json!({
            "rpm": r1(req5 as f64 / 5.0),
            "p95_ms": r1(percentile(&hist5, 0.95)),
            "err_rate": if req5 == 0 { 0.0 } else { (e5 as f64 / req5 as f64 * 1000.0).round() / 1000.0 },
            "series": series,
            "routes": routes.into_iter().map(|(m, p, c, p95, max)| json!({ "method": m, "path": p, "count": c, "p95_ms": r1(p95), "max_ms": r1(max) })).collect::<Vec<_>>(),
            "errors": s.errors.iter().rev().map(|e| json!({ "t": e.t, "status": e.status, "method": e.method, "path": e.path, "ms": r1(e.ms) })).collect::<Vec<_>>(),
            "queue": queue,
            "log": { "errors_1h": s.log.errors.len(), "last": s.log.last.as_ref().map(|(t, m)| json!({ "t": t, "message": m })) },
        }));
    }
    json!({ "sites": out })
}

/// GET /api/services/metrics
pub fn services_json() -> Value {
    let store = STORE.lock().unwrap();
    Value::Object(store.services.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_and_assets() {
        assert_eq!(route("/users/42/posts?page=2"), "/users/{id}/posts");
        assert_eq!(route("/o/123e4567-e89b-12d3-a456-426614174000"), "/o/{id}");
        assert_eq!(route("/"), "/");
        assert!(is_asset("/build/assets/app-1234.js"));
        assert!(is_asset("/@vite/client"));
        assert!(is_asset("/favicon.ico"));
        assert!(!is_asset("/login"));
    }

    #[test]
    fn caddy_lines() {
        let l = r#"{"level":"info","ts":1795330000.25,"logger":"http.log.access.log0","msg":"handled request","request":{"remote_ip":"127.0.0.1","proto":"HTTP/2.0","method":"GET","host":"acmeshop.localhost","uri":"/login?x=1"},"duration":0.0123,"size":5120,"status":200}"#;
        let (ts, m, u, s, ms) = parse_access(l).unwrap();
        assert_eq!((m.as_str(), u.as_str(), s), ("GET", "/login?x=1", 200));
        assert!((ms - 12.3).abs() < 0.01 && ts > 1.0);
    }

    #[test]
    fn queue_lines() {
        assert_eq!(parse_queue("  2026-11-22 07:14:31 App\\Jobs\\SyncPortfolio ........ 186.42ms DONE"), Some(("App\\Jobs\\SyncPortfolio".into(), 186.42, false)));
        assert_eq!(parse_queue("07:16:12 App\\Jobs\\Import ..... 2,031.55ms DONE"), Some(("App\\Jobs\\Import".into(), 2031.55, false)));
        assert_eq!(parse_queue("2026-11-22 07:16:12 App\\Jobs\\Import ..... 1.5s FAIL"), Some(("App\\Jobs\\Import".into(), 1500.0, true)));
        assert_eq!(parse_queue("2026-11-22 07:14:31 App\\Jobs\\SyncPortfolio ........ RUNNING"), None);
        assert_eq!(parse_queue("INFO  Horizon started successfully."), None);
    }

    #[test]
    fn log_errors() {
        let (t, m) = parse_log_error("[2026-11-22 07:14:31] local.ERROR: Undefined variable $x {\"exception\":\"...\"}").unwrap();
        assert_eq!(t, Some(days_from_civil(2026, 11, 22) as u64 * 86400 + 7 * 3600 + 14 * 60 + 31));
        assert!(m.starts_with("Undefined variable"));
        assert!(parse_log_error("[2026-11-22 07:14:31] local.INFO: fine").is_none());
        assert!(parse_log_error("#0 /var/www/vendor/...").is_none());
    }

    #[test]
    fn histogram_percentiles() {
        let mut h = [0u32; HIST];
        for ms in [5.0, 8.0, 9.0, 40.0, 900.0] { h[bucket_of(ms)] += 1; }
        let p50 = percentile(&h, 0.5);
        assert!(p50 > 5.0 && p50 <= 10.0, "{p50}");
        assert!(percentile(&h, 0.95) > 750.0);
    }

    #[test]
    fn redis_info_fields() {
        let v = parse_redis_info("# Memory\r\nused_memory:1024\r\ninstantaneous_ops_per_sec:7\r\nkeyspace_hits:3\r\nkeyspace_misses:1\r\n# Keyspace\r\ndb0:keys=10,expires=0,avg_ttl=0\r\ndb1:keys=5,expires=0\r\n");
        assert_eq!(v["used_memory"], 1024);
        assert_eq!(v["keys"], 15);
        assert_eq!(v["hit_rate"], 0.75);
    }
}
