//! Small shared helpers: the error type, output, versions, ports and .env files.

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::time::Duration;

pub type R<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[macro_export]
macro_rules! bail {
    ($($t:tt)*) => { return Err(format!($($t)*).into()) };
}

fn color(code: &str, s: &str) -> String {
    if std::io::stdout().is_terminal() { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
}
pub fn green(s: &str) -> String { color("32", s) }
pub fn red(s: &str) -> String { color("31", s) }
pub fn yellow(s: &str) -> String { color("33", s) }
pub fn dim(s: &str) -> String { color("2", s) }
pub fn bold(s: &str) -> String { color("1", s) }

pub fn step(msg: &str) { println!("{} {msg}", color("36", "›")); }
pub fn ok(msg: &str) { println!("{} {msg}", green("✓")); }
pub fn warn(msg: &str) { println!("{} {msg}", yellow("!")); }

/// Compare dotted versions numerically ("8.10" > "8.9"). Non-numeric parts compare as 0.
pub fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    let pa: Vec<u64> = a.split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect();
    let pb: Vec<u64> = b.split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect();
    for i in 0..pa.len().max(pb.len()) {
        let (x, y) = (pa.get(i).copied().unwrap_or(0), pb.get(i).copied().unwrap_or(0));
        if x != y { return x.cmp(&y); }
    }
    std::cmp::Ordering::Equal
}

pub fn is_prerelease(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    l.contains("alpha") || l.contains("beta") || l.contains("rc") || l.contains("dev")
}

/// True when something already listens on 127.0.0.1:port.
pub fn port_open(port: u16) -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
}

/// True when we could bind 127.0.0.1:port ourselves right now.
pub fn port_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Parse a dotenv file into ordered key/value pairs (quotes removed, comments skipped).
pub fn read_env(path: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else { return map };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            let mut v = v.trim().to_string();
            if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2) || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2) {
                v = v[1..v.len() - 1].to_string();
            } else if let Some(i) = v.find(" #") {
                v.truncate(i);
                v = v.trim().to_string();
            }
            map.insert(k.trim().to_string(), v);
        }
    }
    map
}

/// Set keys in a dotenv file, keeping every other line exactly as it was.
pub fn write_env_keys(path: &Path, updates: &[(&str, String)]) -> R<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    for (key, value) in updates {
        let quoted = if value.contains(' ') || value.contains('#') { format!("\"{value}\"") } else { value.clone() };
        let mut found = false;
        for line in lines.iter_mut() {
            let t = line.trim_start();
            if t.starts_with(&format!("{key}=")) {
                *line = format!("{key}={quoted}");
                found = true;
            }
        }
        if !found { lines.push(format!("{key}={quoted}")); }
    }
    // Keep the file's own line endings; new files get the OS's.
    let nl = if text.contains("\r\n") { "\r\n" } else if text.contains('\n') { "\n" } else { crate::sys::NL };
    let mut out = lines.join(nl);
    out.push_str(nl);
    std::fs::write(path, out)?;
    Ok(())
}

/// Last `n` lines of a text file.
pub fn tail(path: &Path, n: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let want = (n as u64 * 400).min(len);
    let _ = f.seek(SeekFrom::Start(len - want));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Lower-case a folder name into a host name label.
pub fn host_label(name: &str) -> String {
    let mut s: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '-' })
        .collect();
    while s.contains("--") { s = s.replace("--", "-"); }
    s.trim_matches('-').to_string()
}

/// "2026-11-22 14:03:07" in UTC, without a date library (civil-from-days, Howard Hinnant).
pub fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

pub fn human_bytes(b: u64) -> String {
    if b >= 1 << 30 { format!("{:.1} GB", b as f64 / (1u64 << 30) as f64) }
    else if b >= 1 << 20 { format!("{:.0} MB", b as f64 / (1u64 << 20) as f64) }
    else { format!("{} KB", b / 1024) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions_compare_numerically() {
        assert_eq!(cmp_version("8.10.0", "8.9.9"), std::cmp::Ordering::Greater);
        assert_eq!(cmp_version("17.12", "17.12.0"), std::cmp::Ordering::Equal);
        assert!(is_prerelease("6.0.0RC1"));
        assert!(!is_prerelease("6.3.0"));
    }
    #[test]
    fn relative_paths_join_per_os() {
        let p = crate::paths::join_rel(Path::new("base"), "storage/framework/cache");
        assert_eq!(p, Path::new("base").join("storage").join("framework").join("cache"));
    }
    #[test]
    fn labels_are_hostname_safe() {
        assert_eq!(host_label("Acme Shop_App"), "acme-shop-app");
        assert_eq!(host_label("admin.acmeshop.com"), "admin.acmeshop.com");
    }
    #[test]
    fn env_round_trip() {
        let dir = std::env::temp_dir().join(format!("tez-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(".env");
        std::fs::write(&p, "APP_NAME=\"My App\"\nDB_HOST=pgsql # docker\n# c\nREDIS_HOST=redis\n").unwrap();
        let e = read_env(&p);
        assert_eq!(e["APP_NAME"], "My App");
        assert_eq!(e["DB_HOST"], "pgsql");
        write_env_keys(&p, &[("DB_HOST", "127.0.0.1".into()), ("NEW_KEY", "x".into())]).unwrap();
        let e = read_env(&p);
        assert_eq!(e["DB_HOST"], "127.0.0.1");
        assert_eq!(e["NEW_KEY"], "x");
        assert_eq!(e["REDIS_HOST"], "redis");
        std::fs::remove_dir_all(&dir).ok();
    }
}
