//! Where everything lives. One folder, `~/.tez` (`%USERPROFILE%\.tez` on Windows, or `TEZ_HOME`),
//! holds all of it:
//!
//! ```text
//! bin/        tez, plus php, composer (and exec on Windows): the same binary under other names
//! php/<x.y>/  one folder per PHP version (windows.php.net builds; static builds on macOS/Linux)
//! lib/php/    the PHP files tez loads into every PHP run (Horizon support, dumps)
//! services/   <name>/<version>/ binaries: postgres, redis, mailpit, meilisearch, rustfs, frankenphp
//! data/       service data (databases, mail, search indexes, buckets)
//! config/     tez.json, the generated Caddyfile, certificates, cacert.pem, php overrides
//! logs/       one log per managed process
//! cache/      downloads, kept so reinstalling is instant
//! run/        daemon state
//! ```

use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    if let Ok(h) = std::env::var("TEZ_HOME") {
        if !h.is_empty() { return PathBuf::from(h); }
    }
    user_home().join(".tez")
}

/// The user's home folder: `%USERPROFILE%` on Windows, `$HOME` elsewhere.
pub fn user_home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    PathBuf::from(std::env::var(var).unwrap_or_else(|_| ".".into()))
}

pub fn bin() -> PathBuf { home().join("bin") }
pub fn php_root() -> PathBuf { home().join("php") }
pub fn php_dir(minor: &str) -> PathBuf { php_root().join(minor) }
pub fn lib_php() -> PathBuf { home().join("lib").join("php") }
pub fn services_root() -> PathBuf { home().join("services") }
pub fn service_dir(name: &str, version: &str) -> PathBuf { services_root().join(name).join(version) }
pub fn data_dir(name: &str) -> PathBuf { home().join("data").join(name) }
pub fn config_dir() -> PathBuf { home().join("config") }
pub fn config_file() -> PathBuf { config_dir().join("tez.json") }
pub fn caddyfile() -> PathBuf { config_dir().join("Caddyfile") }
pub fn caddy_storage() -> PathBuf { config_dir().join("caddy") }
pub fn cacert() -> PathBuf { config_dir().join("cacert.pem") }
pub fn logs_dir() -> PathBuf { home().join("logs") }
pub fn cache_dir() -> PathBuf { home().join("cache") }
pub fn run_dir() -> PathBuf { home().join("run") }
pub fn local_ca_root() -> PathBuf { caddy_storage().join("pki").join("authorities").join("local").join("root.crt") }

pub fn ensure_dirs() -> std::io::Result<()> {
    for d in [bin(), php_root(), lib_php(), services_root(), home().join("data"), config_dir(), logs_dir(), cache_dir(), run_dir()] {
        std::fs::create_dir_all(d)?;
    }
    Ok(())
}

/// Forward-slash form of a path, which Caddy, php.ini and PostgreSQL all accept on Windows.
pub fn fwd(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").map(|x| x.to_string()).unwrap_or(s)
}

/// Windows' own tools, by full path, so Git Bash's versions on PATH never get picked instead.
pub fn system32(exe: &str) -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    PathBuf::from(root).join("System32").join(exe)
}

/// A system tool by name: Windows' own copy in System32 (`tool("curl")` is curl.exe there),
/// or the one in /usr/bin or /bin on macOS and Linux, else whatever PATH finds.
pub fn tool(name: &str) -> PathBuf {
    if cfg!(windows) { return system32(&format!("{name}.exe")); }
    for dir in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        let p = Path::new(dir).join(name);
        if p.exists() { return p; }
    }
    PathBuf::from(name)
}

/// Join a forward-slash relative path ("storage/logs") onto `base` with the OS's separator.
pub fn join_rel(base: &Path, rel: &str) -> PathBuf {
    rel.split(['/', '\\']).filter(|p| !p.is_empty()).fold(base.to_path_buf(), |acc, part| acc.join(part))
}

/// Normalise a user-given directory into an absolute path without the `\\?\` prefix.
pub fn absolute(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(p) };
    let canon = std::fs::canonicalize(&abs).unwrap_or(abs);
    let s = canon.to_string_lossy().to_string();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
}
