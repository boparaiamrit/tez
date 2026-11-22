//! Services (Herd Pro's "Services"), all native builds bound to 127.0.0.1, data in ~/.tez/data:
//!
//! * PostgreSQL 14 to 18 (EnterpriseDB builds on Windows, theseus-rs portable builds on macOS
//!   and Linux), with TimescaleDB (several versions side by side), PostGIS and pgvector on Windows
//! * MySQL 8.0 / 8.4, MariaDB 10.11 to 13 (Windows and Linux x86_64), MongoDB 6 to 8.2
//! * Redis (redis-windows builds; compiled from source on macOS and Linux), Mailpit, Meilisearch,
//!   RustFS (S3, in place of MinIO, whose binaries were withdrawn)
//!
//! A service can run several versions at once: the first is named after the service ("postgres"),
//! more are named `<service>@<version>` ("postgres@16") with their own port and data.

use crate::config::{Config, ServiceConfig};
use crate::fetch;
use crate::paths;
use crate::spec::Spec;
use crate::sys::{self, Hidden};
use crate::util::{self, R};
use crate::bail;
use std::path::{Path, PathBuf};

pub struct Info {
    pub name: &'static str,
    pub title: &'static str,
    pub version: &'static str,
    pub port: u16,
    pub about: &'static str,
    /// Versions offered in the dashboard (series; the newest patch is fetched).
    pub versions: &'static [&'static str],
}

pub const CATALOG: &[Info] = &[
    Info { name: "postgres", title: "PostgreSQL", version: "17", port: 5432, about: "PostgreSQL with TimescaleDB, PostGIS, pgvector, psql and pg_dump", versions: &["14", "15", "16", "17", "18"] },
    Info { name: "mysql", title: "MySQL", version: "8.4", port: 3306, about: "MySQL server", versions: &["8.0", "8.4"] },
    Info { name: "mariadb", title: "MariaDB", version: "11.8", port: 3307, about: "MariaDB server", versions: &["10.11", "11.4", "11.8", "12.3"] },
    Info { name: "mongodb", title: "MongoDB", version: "8.0", port: 27017, about: "MongoDB server (for mongodb/laravel-mongodb)", versions: &["6.0", "7.0", "8.0", "8.2"] },
    Info { name: "redis", title: "Redis", version: "8.10.3", port: 6379, about: "Redis for cache, sessions, queues and Horizon", versions: &[] },
    Info { name: "mailpit", title: "Mailpit", version: "1.34.0", port: 1025, about: "Catches every email; web inbox on port 8025", versions: &[] },
    Info { name: "meilisearch", title: "Meilisearch", version: "1.57.0", port: 7700, about: "Search engine for Laravel Scout", versions: &[] },
    Info { name: "rustfs", title: "S3 (RustFS)", version: "1.0.6", port: 9000, about: "S3-compatible storage; console on port 9001", versions: &[] },
];

pub const TIMESCALE_VERSION: &str = "2.32.1";
pub const MAILPIT_UI_PORT: u16 = 8025;
pub const S3_CONSOLE_PORT: u16 = 9001;
pub const S3_KEY: &str = "tez";
pub const S3_SECRET: &str = "tez-secret";

/// PostgreSQL extensions tez can add, beyond the contrib set every build has.
pub const PG_EXTENSIONS: &[(&str, &str)] = &[
    ("timescaledb", "Time-series hypertables; several versions can be installed side by side"),
    ("postgis", "Geography and geometry (the OSGeo bundle: PostGIS, pgRouting, h3 and more)"),
    ("pgvector", "Vector similarity search (the vector type)"),
];

/// "postgres@16" -> "postgres".
pub fn kind(key: &str) -> &str { key.split('@').next().unwrap_or(key) }

/// Whether a service has a build for this OS and CPU; Err says why not.
pub fn available(key: &str) -> Result<(), String> {
    let (os, arch) = (sys::os(), sys::arch());
    let no = |why: &str| Err(format!("{} {why}", info(key).map(|i| i.title).unwrap_or("this service")));
    match (kind(key), os, arch) {
        (_, "windows", _) => Ok(()),
        ("mariadb", "linux", "x86_64") => Ok(()),
        ("mariadb", _, _) => no("publishes builds for Windows and Linux x86_64 only"),
        ("rustfs", "macos", "x86_64") => no("has no build for Intel Macs"),
        ("mongodb", "linux", _) if linux_mongodb_target().is_none() => no("has builds for Ubuntu, Debian, RHEL and Amazon Linux only"),
        ("redis", _, _) if !has_build_tools() => no("is compiled from source on this OS and needs make and a C compiler (xcode-select --install, or apt install build-essential)"),
        _ => Ok(()),
    }
}

/// PostgreSQL extensions tez can add on this OS.
pub fn pg_extensions_available() -> bool { cfg!(windows) }

/// make and a C compiler, to build Redis from source (checked once per process).
fn has_build_tools() -> bool {
    static HAS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HAS.get_or_init(|| {
        let ok = |c: &str, a: &str| std::process::Command::new(c).arg(a).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        // On macOS, running make or cc without the Command Line Tools opens an install dialog;
        // xcode-select -p only asks whether they are there.
        if cfg!(target_os = "macos") { return ok("xcode-select", "-p"); }
        // Redis 8 has C++ parts (fast_float), so a C++ compiler too.
        ok("make", "--version") && (ok("cc", "--version") || ok("gcc", "--version")) && (ok("c++", "--version") || ok("g++", "--version"))
    })
}

/// MongoDB's name for this Linux distribution ("ubuntu2404", "debian12", "rhel93"...).
fn linux_mongodb_target() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(|v| v.trim_matches('"').to_string());
    let id = get("ID")?;
    let like = get("ID_LIKE").unwrap_or_default();
    let ver = get("VERSION_ID").unwrap_or_default();
    let major = ver.split('.').next().unwrap_or("").to_string();
    Some(match id.as_str() {
        "ubuntu" => format!("ubuntu{}", ver.replace('.', "")),
        "debian" => format!("debian{major}"),
        "amzn" => format!("amazon{major}"),
        _ if like.contains("rhel") || like.contains("fedora") || id == "rhel" => match major.as_str() { "8" => "rhel8".into(), "9" => "rhel93".into(), m => format!("rhel{m}") },
        _ if like.contains("ubuntu") => return None,
        _ => return None,
    })
}

pub fn info(key: &str) -> R<&'static Info> {
    let k = kind(key);
    match CATALOG.iter().find(|i| i.name == k) {
        Some(i) => Ok(i),
        None => bail!("unknown service '{k}'; choose one of {}", CATALOG.iter().map(|i| i.name).collect::<Vec<_>>().join(", ")),
    }
}

/// The part of a version that names its folder: PostgreSQL's major, MySQL/MariaDB/MongoDB's series.
pub fn series(kind_name: &str, version: &str) -> String {
    let parts: Vec<&str> = version.split('.').collect();
    match kind(kind_name) {
        "postgres" => parts[0].to_string(),
        "mysql" | "mariadb" | "mongodb" => parts.iter().take(2).copied().collect::<Vec<_>>().join("."),
        _ => version.to_string(),
    }
}

pub fn dir(name: &str, version: &str) -> PathBuf { paths::service_dir(kind(name), &series(name, version)) }

fn first_existing(d: &Path, names: &[&str]) -> PathBuf {
    names.iter().map(|n| d.join(n)).find(|p| p.exists()).unwrap_or_else(|| d.join(names[0]))
}

pub fn exe(name: &str, version: &str) -> PathBuf {
    let d = dir(name, version);
    let x = sys::exe;
    match kind(name) {
        "postgres" => d.join("bin").join(x("postgres")),
        "mysql" => d.join("bin").join(x("mysqld")),
        "mariadb" => first_existing(&d.join("bin"), &[x("mariadbd").as_str(), x("mysqld").as_str()]),
        "mongodb" => d.join("bin").join(x("mongod")),
        "redis" => d.join(x("redis-server")),
        "mailpit" => d.join(x("mailpit")),
        "meilisearch" => d.join(x("meilisearch")),
        "rustfs" => d.join(x("rustfs")),
        other => d.join(x(other)),
    }
}

pub fn is_installed(name: &str, version: &str) -> bool { exe(name, version).exists() }

pub fn data(name: &str, version: &str) -> PathBuf {
    match kind(name) {
        k @ ("postgres" | "mysql" | "mariadb" | "mongodb") => paths::data_dir(&format!("{k}-{}", series(name, version))),
        k => paths::data_dir(k),
    }
}

/// The config key for a new version of a service: the service's own name unless that already
/// runs another version, then `<service>@<series>`.
pub fn instance_key(cfg: &Config, kind_name: &str, version: &str) -> String {
    match cfg.services.get(kind_name) {
        None => kind_name.to_string(),
        Some(sc) if sc.version.is_empty() || series(kind_name, &sc.version) == series(kind_name, version) => kind_name.to_string(),
        Some(_) => format!("{kind_name}@{}", series(kind_name, version)),
    }
}

/// A port no other tez service uses and nothing else holds.
pub fn free_port(cfg: &Config, base: u16) -> u16 {
    let taken: Vec<u16> = cfg.services.iter().map(|(k, s)| if s.port != 0 { s.port } else { info(k).map(|i| i.port).unwrap_or(0) }).collect();
    (base..base + 100).find(|p| !taken.contains(p) && util::port_free(*p)).unwrap_or(base)
}

// ------------------------------------------------------------------ resolving downloads

fn edb_url(v: &str) -> String { format!("https://get.enterprisedb.com/postgresql/postgresql-{v}-1-windows-x64-binaries.zip") }

/// Portable PostgreSQL builds for macOS and Linux (the ones postgresql_embedded uses).
const THESEUS: &str = "theseus-rs/postgresql-binaries";

fn theseus_triple() -> &'static str {
    match (sys::os(), sys::arch()) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", _) => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => "x86_64-unknown-linux-gnu",
    }
}

/// The newest theseus-rs release tag for a major ("17") or a minor ("17.12"): "17.12.0".
fn theseus_tag(v: &str) -> R<String> {
    let tags = fetch::github_tags(THESEUS)?;
    if tags.iter().any(|t| t == v) { return Ok(v.to_string()); }
    let prefix = format!("{v}.");
    tags.into_iter().filter(|t| t.starts_with(&prefix)).max_by(|a, b| util::cmp_version(a, b))
        .ok_or_else(|| format!("no PostgreSQL {v} build for {} at {THESEUS}", theseus_triple()).into())
}

fn postgres_full_version(v: &str) -> R<String> {
    if !cfg!(windows) { return theseus_tag(v); }
    if v.contains('.') { return Ok(v.to_string()); }
    let major: u32 = v.parse().map_err(|_| format!("'{v}' is not a PostgreSQL version"))?;
    for minor in (0..=20).rev() {
        let full = format!("{major}.{minor}");
        if fetch::exists(&edb_url(&full)) { return Ok(full); }
    }
    bail!("no PostgreSQL {major} build found at EnterpriseDB")
}

/// A full version ("7.0.14") rather than a series ("7.0").
fn is_exact(version: &str) -> bool { version.split('.').count() >= 3 }

/// What to ask a resolver for: the exact version when one was given, else the series.
fn spec_for(k: &str, version: &str) -> String { if is_exact(version) { version.to_string() } else { series(k, version) } }

/// MongoDB's names for this machine: (target, arch, edition).
fn mongodb_target() -> R<(String, &'static str, &'static str)> {
    Ok(match sys::os() {
        "windows" => ("windows".to_string(), "x86_64", "base"),
        "macos" => ("macos".to_string(), if sys::arch() == "aarch64" { "arm64" } else { "x86_64" }, "base"),
        _ => (linux_mongodb_target().ok_or("MongoDB has builds for Ubuntu, Debian, RHEL and Amazon Linux only")?, sys::arch(), "targeted"),
    })
}

fn mongodb_catalog() -> R<serde_json::Value> {
    let body = fetch::get_text("https://downloads.mongodb.org/full.json")?;
    Ok(serde_json::from_str(&body)?)
}

/// The archive URL of one MongoDB release for this machine, if it has one.
fn mongodb_url(v: &serde_json::Value, target: &str, arch: &str, edition: &str) -> Option<String> {
    v["downloads"].as_array().into_iter().flatten()
        .find(|d| d["target"] == target && d["arch"] == arch && d["edition"] == edition)
        .and_then(|d| d["archive"]["url"].as_str()).map(|s| s.to_string())
}

/// A MongoDB release from MongoDB's own catalog: the newest production release of a series
/// ("8.0"), or exactly the version asked for ("8.0.12").
fn mongodb_release(spec: &str) -> R<(String, String)> {
    let json = mongodb_catalog()?;
    let (target, arch, edition) = mongodb_target()?;
    let exact = is_exact(spec);
    let mut best: Option<(String, String)> = None;
    for v in json["versions"].as_array().into_iter().flatten() {
        let ver = v["version"].as_str().unwrap_or_default();
        let wanted = if exact { ver == spec } else { v["production_release"].as_bool().unwrap_or(false) && ver.starts_with(&format!("{spec}.")) };
        if !wanted { continue; }
        if let Some(url) = mongodb_url(v, &target, arch, edition) {
            if best.as_ref().map(|(b, _)| util::cmp_version(ver, b) == std::cmp::Ordering::Greater).unwrap_or(true) { best = Some((ver.to_string(), url)); }
        }
    }
    best.ok_or_else(|| format!("no {target} {arch} build of MongoDB {spec}").into())
}

/// The download URL of one MariaDB release for this machine, from its REST record.
fn mariadb_file(ver: &str, rel: &serde_json::Value) -> Option<String> {
    for f in rel["files"].as_array().into_iter().flatten() {
        let name = f["file_name"].as_str().unwrap_or_default();
        let wanted = if cfg!(windows) { name.ends_with("-winx64.zip") }
            else { name.starts_with(&format!("mariadb-{ver}-")) && name.ends_with("-linux-systemd-x86_64.tar.gz") };
        if wanted { return Some(f["file_download_url"].as_str().unwrap_or_default().replace("http://", "https://")); }
    }
    None
}

/// A MariaDB release from MariaDB's download API: the newest of a series ("11.4"), or exactly
/// the version asked for ("11.4.5").
fn mariadb_release(spec: &str) -> R<(String, String)> {
    let series: String = spec.split('.').take(2).collect::<Vec<_>>().join(".");
    let url = if is_exact(spec) { format!("https://downloads.mariadb.org/rest-api/mariadb/{series}/") } else { format!("https://downloads.mariadb.org/rest-api/mariadb/{series}/latest/") };
    let json: serde_json::Value = serde_json::from_str(&fetch::get_text(&url)?)?;
    if let Some(rels) = json["releases"].as_object() {
        for (ver, rel) in rels {
            if is_exact(spec) && ver != spec { continue; }
            if let Some(url) = mariadb_file(ver, rel) { return Ok((ver.clone(), url)); }
        }
    }
    bail!("no {} build of MariaDB {spec}", sys::os())
}

/// MySQL's archive names for a version on this machine (newest macOS first).
fn mysql_files(ver: &str) -> Vec<String> {
    match (sys::os(), sys::arch()) {
        ("windows", _) => vec![format!("mysql-{ver}-winx64.zip")],
        ("macos", a) => {
            let a = if a == "aarch64" { "arm64" } else { "x86_64" };
            ["15", "14", "13"].iter().map(|m| format!("mysql-{ver}-macos{m}-{a}.tar.gz")).collect()
        }
        (_, a) => vec![format!("mysql-{ver}-linux-glibc2.28-{a}.tar.xz"), format!("mysql-{ver}-linux-glibc2.17-{a}.tar.xz")],
    }
}

/// Where one MySQL version can be downloaded for this machine: the current downloads, else the
/// archives (downloads.mysql.com keeps every release).
fn mysql_url(ver: &str) -> Option<String> {
    let series: String = ver.split('.').take(2).collect::<Vec<_>>().join(".");
    for file in mysql_files(ver) {
        for url in [format!("https://dev.mysql.com/get/Downloads/MySQL-{series}/{file}"), format!("https://downloads.mysql.com/archives/get/p/23/file/{file}")] {
            if fetch::exists(&url) { return Some(url); }
        }
    }
    None
}

/// A MySQL release: the newest patch of a series ("8.4"), or exactly the version asked for
/// ("8.0.40"). dev.mysql.com has no catalog, so probe.
fn mysql_release(spec: &str) -> R<(String, String)> {
    if is_exact(spec) {
        return mysql_url(spec).map(|u| (spec.to_string(), u)).ok_or_else(|| format!("no {} build of MySQL {spec}", sys::os()).into());
    }
    for n in (0..=50).rev() {
        let ver = format!("{spec}.{n}");
        for file in mysql_files(&ver) {
            let url = format!("https://dev.mysql.com/get/Downloads/MySQL-{spec}/{file}");
            if fetch::exists(&url) { return Ok((ver, url)); }
        }
    }
    bail!("no {} build of MySQL {spec}", sys::os())
}

/// The file name at the end of a URL, for the download cache.
fn url_file(url: &str) -> String { url.rsplit('/').next().unwrap_or("download").to_string() }

/// Build Redis from its source release with make (macOS and Linux have no official binaries).
fn build_redis(version: &str, dest: &Path) -> R<()> {
    let tgz = fetch::download(&format!("https://github.com/redis/redis/archive/refs/tags/{version}.tar.gz"), &format!("redis-{version}-src.tar.gz"))?;
    let build = paths::cache_dir().join(format!("redis-build-{version}"));
    let _ = std::fs::remove_dir_all(&build);
    fetch::extract_flat(&tgz, &build, &[])?;
    fetch::ensure_space(&build, 500 << 20)?;
    let jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    util::step(&format!("Compiling Redis {version} from source (about a minute; the log is {})", build.join("tez-build.log").display()));
    let log = std::fs::File::create(build.join("tez-build.log"))?;
    let st = std::process::Command::new("make")
        .arg(format!("-j{jobs}")).arg("BUILD_TLS=no")
        .current_dir(&build)
        .stdin(std::process::Stdio::null()).stdout(log.try_clone()?).stderr(log)
        .status()?;
    if !st.success() {
        bail!("compiling Redis failed:\n{}", util::tail(&build.join("tez-build.log"), 25));
    }
    std::fs::create_dir_all(dest)?;
    for b in ["redis-server", "redis-cli"] {
        std::fs::copy(build.join("src").join(b), dest.join(b))?;
        fetch::make_executable(&dest.join(b))?;
    }
    let _ = std::fs::remove_dir_all(&build);
    Ok(())
}

/// Release asset names for this machine: (mailpit, meilisearch, rustfs).
fn asset_names(version: &str) -> (String, String, String) {
    let (os, arch) = (sys::os(), sys::arch());
    let go_arch = if arch == "aarch64" { "arm64" } else { "amd64" };
    let mailpit = match os { "windows" => "mailpit-windows-amd64.zip".to_string(), "macos" => format!("mailpit-darwin-{go_arch}.tar.gz"), _ => format!("mailpit-linux-{go_arch}.tar.gz") };
    let meili = match (os, arch) {
        ("windows", _) => "meilisearch-windows-amd64.exe".to_string(),
        ("macos", "aarch64") => "meilisearch-macos-apple-silicon".into(),
        ("macos", _) => "meilisearch-macos-amd64".into(),
        (_, "aarch64") => "meilisearch-linux-aarch64".into(),
        _ => "meilisearch-linux-amd64".into(),
    };
    let rustfs = match (os, arch) {
        ("windows", _) => format!("rustfs-windows-x86_64-v{version}.zip"),
        ("macos", _) => format!("rustfs-macos-aarch64-v{version}.zip"),
        (_, "aarch64") => format!("rustfs-linux-aarch64-gnu-v{version}.zip"),
        _ => format!("rustfs-linux-x86_64-gnu-v{version}.zip"),
    };
    (mailpit, meili, rustfs)
}

// ------------------------------------------------------------------ install

/// The full version installed in a service's folder (".tez-version"), if any.
pub fn installed_full(name: &str, version: &str) -> Option<String> {
    std::fs::read_to_string(dir(name, version).join(".tez-version")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Download and set up a service version. Returns the version to store in the config.
///
/// An exact version ("7.0.14", "17.4") of a series that already has a folder with another
/// version replaces that folder; data folders are separate and stay. PostgreSQL extensions
/// that were added to the old folder are added again.
pub fn install(name: &str, version: Option<&str>) -> R<String> {
    let info = info(name)?;
    let k = kind(name);
    let want = version.unwrap_or(info.version).trim_start_matches('v').to_string();
    let d = dir(k, &want);
    let exact = if k == "postgres" { want.contains('.') } else { is_exact(&want) };
    let current = installed_full(k, &want);
    let replacing = matches!(k, "postgres" | "mysql" | "mariadb" | "mongodb") && exact && d.exists()
        && current.as_deref().map(|c| c.trim_end_matches(".0") != want.trim_end_matches(".0")).unwrap_or(true);
    if !replacing { return install_inner(name, &want); }
    let pg_exts: Vec<(String, String)> = if k == "postgres" { pg_extensions_installed(&series(k, &want)) } else { Vec::new() };
    let ts_versions = if k == "postgres" { timescale_versions(&d) } else { Vec::new() };
    let old = d.with_extension("old");
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(&d, &old).map_err(|e| format!("{} {} is in use; stop it first ({e})", info.title, series(k, &want)))?;
    match install_inner(name, &want) {
        Ok(v) => {
            let _ = std::fs::remove_dir_all(&old);
            let major = series(k, &want);
            for t in ts_versions { if let Err(e) = install_pg_extension(&major, "timescaledb", Some(&t)) { util::warn(&format!("TimescaleDB {t}: {e}")); } }
            for (ext, ver) in pg_exts.into_iter().filter(|(e, _)| e != "timescaledb") {
                if let Err(e) = install_pg_extension(&major, &ext, Some(&ver)) { util::warn(&format!("{ext} {ver}: {e}")); }
            }
            Ok(v)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&d);
            let _ = std::fs::rename(&old, &d);
            Err(e)
        }
    }
}

fn install_inner(name: &str, version: &str) -> R<String> {
    let info = info(name)?;
    let k = kind(name);
    available(k)?;
    let version = version.to_string();
    let d = dir(k, &version);
    let win = cfg!(windows);
    let stored = match k {
        "postgres" if win => {
            let full = postgres_full_version(&version)?;
            util::step(&format!("Downloading PostgreSQL {full} (EnterpriseDB build)"));
            let zip = fetch::download(&edb_url(&full), &format!("postgresql-{full}-1-windows-x64-binaries.zip"))?;
            util::step("Unpacking (server, client tools and extensions; pgAdmin and docs left out)");
            fetch::extract_flat(&zip, &d, &["pgsql/pgAdmin 4", "pgsql/StackBuilder", "pgsql/doc", "pgsql/symbols", "pgsql/include"])?;
            std::fs::write(d.join(".tez-version"), &full)?;
            let major = series(k, &full);
            if let Err(e) = install_pg_extension(&major, "timescaledb", None) { util::warn(&format!("TimescaleDB: {e}")); }
            init_postgres(&major)?;
            util::ok(&format!("PostgreSQL {full}"));
            return Ok(major);
        }
        "postgres" => {
            let tag = postgres_full_version(&version)?;
            let file = format!("postgresql-{tag}-{}.tar.gz", theseus_triple());
            let url = format!("https://github.com/{THESEUS}/releases/download/{tag}/{file}");
            util::step(&format!("Downloading PostgreSQL {tag} ({file})"));
            let tgz = fetch::download(&url, &file)?;
            if let Ok(sum) = fetch::get_text(&format!("{url}.sha256")) {
                let h = sum.split_whitespace().next().unwrap_or_default().to_string();
                if h.len() == 64 { fetch::verify_sha256(&tgz, &h)?; }
            }
            fetch::extract_flat(&tgz, &d, &[])?;
            for b in ["postgres", "initdb", "pg_ctl", "psql", "pg_dump", "pg_restore"] {
                let p = d.join("bin").join(b);
                if p.exists() { fetch::make_executable(&p)?; }
            }
            std::fs::write(d.join(".tez-version"), &tag)?;
            let major = series(k, &tag);
            init_postgres(&major)?;
            util::ok(&format!("PostgreSQL {tag}"));
            return Ok(major);
        }
        "mongodb" => {
            let (full, url) = mongodb_release(&spec_for(k, &version))?;
            util::step(&format!("Downloading MongoDB {full}"));
            let cached = if win { format!("mongodb-windows-x86_64-{full}.zip") } else { url_file(&url) };
            let zip = fetch::download(&url, &cached)?;
            fetch::extract_flat(&zip, &d, &["*.pdb"])?;
            std::fs::write(d.join(".tez-version"), &full)?;
            series(k, &full)
        }
        "mariadb" => {
            let (full, url) = mariadb_release(&spec_for(k, &version))?;
            util::step(&format!("Downloading MariaDB {full}"));
            let cached = if win { format!("mariadb-{full}-winx64.zip") } else { url_file(&url) };
            let zip = fetch::download(&url, &cached)?;
            fetch::extract_flat(&zip, &d, &["*.pdb", "*/mysql-test", "*/sql-bench"])?;
            std::fs::write(d.join(".tez-version"), &full)?;
            init_mariadb(&series(k, &full))?;
            series(k, &full)
        }
        "mysql" => {
            let (full, url) = mysql_release(&spec_for(k, &version))?;
            util::step(&format!("Downloading MySQL {full}"));
            let cached = if win { format!("mysql-{full}-winx64.zip") } else { url_file(&url) };
            let zip = fetch::download(&url, &cached)?;
            fetch::extract_flat(&zip, &d, &["*.pdb", "*/lib/debug", "*/mysql-test", "*/docs"])?;
            std::fs::write(d.join(".tez-version"), &full)?;
            init_mysql(&series(k, &full))?;
            series(k, &full)
        }
        "redis" if win => {
            let file = format!("Redis-{version}-Windows-x64-msys2.zip");
            let zip = fetch::download(&format!("https://github.com/redis-windows/redis-windows/releases/download/{version}/{file}"), &file)?;
            fetch::extract_flat(&zip, &d, &[])?;
            version.clone()
        }
        "redis" => {
            build_redis(&version, &d)?;
            version.clone()
        }
        "mailpit" => {
            let (asset, _, _) = asset_names(&version);
            let cached = if win { format!("mailpit-{version}-windows-amd64.zip") } else { format!("mailpit-{version}-{asset}") };
            let zip = fetch::download(&format!("https://github.com/axllent/mailpit/releases/download/v{version}/{asset}"), &cached)?;
            fetch::extract(&zip, &d, &[])?;
            fetch::make_executable(&exe(k, &version))?;
            version.clone()
        }
        "meilisearch" => {
            let (_, asset, _) = asset_names(&version);
            let cached = if win { format!("meilisearch-{version}-windows-amd64.exe") } else { format!("{asset}-v{version}") };
            let file = fetch::download(&format!("https://github.com/meilisearch/meilisearch/releases/download/v{version}/{asset}"), &cached)?;
            std::fs::create_dir_all(&d)?;
            std::fs::copy(file, exe(k, &version))?;
            fetch::make_executable(&exe(k, &version))?;
            version.clone()
        }
        "rustfs" => {
            let (_, _, file) = asset_names(&version);
            let zip = fetch::download(&format!("https://github.com/rustfs/rustfs/releases/download/{version}/{file}"), &file)?;
            fetch::extract(&zip, &d, &[])?;
            // Some archives keep the binary in a folder; bring it up beside .tez-version.
            if !exe(k, &version).exists() {
                if let Some(found) = find_file(&d, &sys::exe("rustfs")) { std::fs::rename(found, exe(k, &version))?; }
            }
            fetch::make_executable(&exe(k, &version))?;
            version.clone()
        }
        _ => bail!("cannot install {k}"),
    };
    if !d.join(".tez-version").exists() { std::fs::write(d.join(".tez-version"), &stored)?; }
    std::fs::create_dir_all(data(k, &stored))?;
    util::ok(&format!("{} {} installed", info.title, std::fs::read_to_string(d.join(".tez-version")).unwrap_or(stored.clone())));
    Ok(stored)
}

fn init_postgres(major: &str) -> R<()> {
    let data = data("postgres", major);
    if data.join("PG_VERSION").exists() { return Ok(()); }
    let bin = dir("postgres", major).join("bin");
    std::fs::create_dir_all(data.parent().unwrap())?;
    util::step("Creating the database cluster (superuser `postgres`, local connections trusted)");
    let out = std::process::Command::new(bin.join(sys::exe("initdb"))).hidden()
        .arg("-D").arg(&data)
        .args(["-U", "postgres", "-E", "UTF8", "--locale=C", "--auth=trust"])
        .output()?;
    if !out.status.success() {
        bail!("initdb failed: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
    let conf = data.join("postgresql.conf");
    let mut text = std::fs::read_to_string(&conf)?;
    text.push_str("\n# tez: settings tez manages (port, TimescaleDB) live here\ninclude_if_exists = 'tez.conf'\n");
    std::fs::write(conf, text)?;
    Ok(())
}

fn init_mariadb(s: &str) -> R<()> {
    let data = data("mariadb", s);
    if data.join("mysql").exists() { return Ok(()); }
    let base = dir("mariadb", s);
    util::step("Creating the MariaDB data folder (user root, no password, local only)");
    let out = if cfg!(windows) {
        let tool = first_existing(&base.join("bin"), &["mariadb-install-db.exe", "mysql_install_db.exe"]);
        std::process::Command::new(tool).arg(format!("--datadir={}", data.display())).hidden().output()?
    } else {
        // The Linux archive keeps the script in scripts/; root gets an empty password (no unix_socket).
        let tool = first_existing(&base.join("scripts"), &["mariadb-install-db", "mysql_install_db"]);
        std::process::Command::new(tool)
            .arg("--no-defaults")
            .arg(format!("--basedir={}", base.display()))
            .arg(format!("--datadir={}", data.display()))
            .arg("--auth-root-authentication-method=normal")
            .output()?
    };
    if !out.status.success() { bail!("mariadb-install-db failed: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)); }
    Ok(())
}

fn init_mysql(s: &str) -> R<()> {
    let data = data("mysql", s);
    if data.join("mysql").exists() { return Ok(()); }
    let d = dir("mysql", s);
    util::step("Creating the MySQL data folder (user root, no password, local only)");
    let mut cmd = std::process::Command::new(d.join("bin").join(sys::exe("mysqld")));
    cmd.hidden();
    if !cfg!(windows) { cmd.arg("--no-defaults"); }
    cmd.arg("--initialize-insecure");
    if cfg!(windows) { cmd.arg("--console"); }
    let out = cmd
        .arg(format!("--basedir={}", d.display()))
        .arg(format!("--datadir={}", data.display()))
        .output()?;
    if !out.status.success() { bail!("mysqld --initialize failed: {}", String::from_utf8_lossy(&out.stderr)); }
    Ok(())
}

// ------------------------------------------------------------------ PostgreSQL extensions

/// The first file named `name` anywhere under `dir`.
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() { if let Some(f) = find_file(&p, name) { return Some(f); } }
        else if p.file_name().map(|n| n == name).unwrap_or(false) { return Some(p); }
    }
    None
}

fn copy_tree(src: &Path, dest: &Path) -> R<()> {
    std::fs::create_dir_all(dest)?;
    for e in std::fs::read_dir(src)?.flatten() {
        let p = e.path();
        let to = dest.join(e.file_name());
        if p.is_dir() { copy_tree(&p, &to)?; } else { std::fs::copy(&p, &to)?; }
    }
    Ok(())
}

pub fn timescale_versions(pg: &Path) -> Vec<String> {
    std::fs::read_to_string(pg.join(".tez-timescale")).unwrap_or_default().lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Add an extension to one PostgreSQL major. Returns the version installed.
pub fn install_pg_extension(major: &str, ext: &str, version: Option<&str>) -> R<String> {
    if !pg_extensions_available() {
        bail!("{ext} is not available on {} yet: tez adds TimescaleDB, PostGIS and pgvector to the Windows builds only (every build has the contrib extensions: pgcrypto, hstore, citext, pg_trgm, uuid-ossp...)", sys::os());
    }
    let pg = paths::service_dir("postgres", major);
    if !pg.join("bin").join("postgres.exe").exists() { bail!("PostgreSQL {major} is not installed"); }
    let tmp = paths::cache_dir().join(format!("pgext-{ext}-{major}"));
    let _ = std::fs::remove_dir_all(&tmp);
    let installed = match ext {
        "timescaledb" => {
            let v = version.unwrap_or(TIMESCALE_VERSION).trim_start_matches('v').to_string();
            let url = format!("https://github.com/timescale/timescaledb/releases/download/{v}/timescaledb-postgresql-{major}-windows-amd64.zip");
            let zip = fetch::download(&url, &format!("timescaledb-{v}-postgresql-{major}-windows-amd64.zip"))
                .map_err(|_| format!("TimescaleDB {v} has no Windows build for PostgreSQL {major}"))?;
            fetch::extract(&zip, &tmp, &[])?;
            let mut have = timescale_versions(&pg);
            // The loader (timescaledb.dll) and .control must come from the newest version installed.
            let newest = have.iter().all(|h| util::cmp_version(&v, h) != std::cmp::Ordering::Less);
            for e in std::fs::read_dir(tmp.join("timescaledb"))?.flatten() {
                let p = e.path();
                let n = p.file_name().unwrap().to_string_lossy().to_string();
                if !newest && (n == "timescaledb.dll" || n == "timescaledb.control") { continue; }
                if n.ends_with(".dll") { std::fs::copy(&p, pg.join("lib").join(&n))?; }
                else if n.ends_with(".control") || n.ends_with(".sql") { std::fs::copy(&p, pg.join("share").join("extension").join(&n))?; }
            }
            if !have.contains(&v) { have.push(v.clone()); }
            std::fs::write(pg.join(".tez-timescale"), have.join("\n"))?;
            v
        }
        "postgis" => {
            let bundles = postgis_bundles(major)?;
            let (ver, url) = match version {
                Some(v) => bundles.into_iter().find(|(b, _)| b == v.trim_start_matches('v')).ok_or_else(|| format!("PostGIS {v} has no Windows bundle for PostgreSQL {major}"))?,
                None => bundles.into_iter().next().ok_or_else(|| format!("no PostGIS bundle for PostgreSQL {major}"))?,
            };
            let file = url_file(&url);
            util::step(&format!("Downloading {file}"));
            let zip = fetch::download(&url, &file)?;
            fetch::extract(&zip, &tmp, &[])?;
            let entries: Vec<_> = std::fs::read_dir(&tmp)?.flatten().collect();
            let root = if entries.len() == 1 && entries[0].path().is_dir() { entries[0].path() } else { tmp.clone() };
            copy_tree(&root, &pg)?;
            ver
        }
        "pgvector" | "vector" => {
            // Releases are tagged per PostgreSQL major ("0.8.6_17"), each with vector.v0.8.6-pg17.zip.
            let ver = match version {
                Some(v) => v.split('_').next().unwrap_or(v).trim_start_matches('v').to_string(),
                None => pgvector_versions(major)?.into_iter().next().ok_or_else(|| format!("no pgvector build for PostgreSQL {major}"))?,
            };
            let file = format!("vector.v{ver}-pg{major}.zip");
            let zip = fetch::download(&format!("https://github.com/{PGVECTOR}/releases/download/{ver}_{major}/{file}"), &file)
                .map_err(|_| format!("pgvector {ver} has no Windows build for PostgreSQL {major}"))?;
            fetch::extract(&zip, &tmp, &[])?;
            place_by_type(&tmp, &pg)?;
            ver
        }
        _ => bail!("tez can add {}; every build already has the contrib extensions (pgcrypto, hstore, citext, pg_trgm, uuid-ossp...)", PG_EXTENSIONS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")),
    };
    let _ = std::fs::remove_dir_all(&tmp);
    let mut list: Vec<String> = std::fs::read_to_string(pg.join(".tez-extensions")).unwrap_or_default().lines().map(|s| s.to_string()).filter(|s| !s.starts_with(&format!("{ext}="))).collect();
    list.push(format!("{ext}={installed}"));
    std::fs::write(pg.join(".tez-extensions"), list.join("\n"))?;
    util::ok(&format!("{ext} {installed} for PostgreSQL {major}"));
    Ok(installed)
}

/// Put .dll files in lib/ and .control/.sql in share/extension/, wherever an archive keeps them.
fn place_by_type(src: &Path, pg: &Path) -> R<()> {
    for e in std::fs::read_dir(src)?.flatten() {
        let p = e.path();
        if p.is_dir() { place_by_type(&p, pg)?; continue; }
        let n = p.file_name().unwrap().to_string_lossy().to_string();
        if n.ends_with(".dll") { std::fs::copy(&p, pg.join("lib").join(&n))?; }
        else if n.ends_with(".control") || n.ends_with(".sql") { std::fs::copy(&p, pg.join("share").join("extension").join(&n))?; }
    }
    Ok(())
}

const PGVECTOR: &str = "andreiramani/pgvector_pgsql_windows";
const TIMESCALE: &str = "timescale/timescaledb";

/// Every PostGIS bundle for one PostgreSQL major on Windows (current and archived), newest
/// first: (version, url).
pub fn postgis_bundles(major: &str) -> R<Vec<(String, String)>> {
    let base = format!("https://download.osgeo.org/postgis/windows/pg{major}/");
    let prefix = format!("postgis-bundle-pg{major}-");
    let mut out: Vec<(String, String)> = Vec::new();
    for dir in [base.clone(), format!("{base}archive/")] {
        let Ok(listing) = fetch::get_text(&dir) else { continue };
        for h in fetch::hrefs(&listing) {
            let name = h.rsplit('/').next().unwrap_or(&h).to_string();
            if let Some(ver) = name.strip_prefix(&prefix).and_then(|r| r.strip_suffix("x64.zip")) {
                if !out.iter().any(|(v, _)| v == ver) { out.push((ver.to_string(), format!("{dir}{name}"))); }
            }
        }
    }
    if out.is_empty() { bail!("no PostGIS bundle for PostgreSQL {major}"); }
    out.sort_by(|a, b| util::cmp_version(&b.0, &a.0));
    Ok(out)
}

/// pgvector versions with a Windows build for one PostgreSQL major, newest first.
pub fn pgvector_versions(major: &str) -> R<Vec<String>> {
    let rels = fetch::github_releases(PGVECTOR, 2)?;
    let mut out: Vec<String> = rels.iter().filter_map(|r| {
        let (ver, m) = r.tag.split_once('_')?;
        (m == major && r.assets.iter().any(|a| a == &format!("vector.v{ver}-pg{major}.zip"))).then(|| ver.to_string())
    }).collect();
    out.sort_by(|a, b| util::cmp_version(b, a));
    out.dedup();
    Ok(out)
}

/// One version tez could install for a service on this machine.
pub struct Offer { pub version: String, pub available: bool, pub reason: String }

fn offer(version: impl Into<String>, available: bool, why: &str) -> Offer {
    Offer { version: version.into(), available, reason: if available { String::new() } else { why.to_string() } }
}

/// Every version of a service its publisher lists, newest first, and whether this machine has
/// a build of it. Full patch versions where the source lists them.
pub fn offers(kind_name: &str) -> R<Vec<Offer>> {
    let k = kind(kind_name);
    let info = info(k)?;
    let (os, arch) = (sys::os(), sys::arch());
    let no_build = format!("no {os} {arch} build of this version");
    let win = cfg!(windows);
    let mut list: Vec<Offer> = match k {
        "postgres" => {
            let rels = fetch::github_releases(THESEUS, 3)?;
            let triple = theseus_triple();
            let cands: Vec<(String, String, bool)> = rels.iter().filter(|r| !r.prerelease).filter_map(|r| {
                let major = r.tag.split('.').next()?.to_string();
                if !info.versions.contains(&major.as_str()) { return None; }
                let short = r.tag.split('.').take(2).collect::<Vec<_>>().join(".");
                let has = r.assets.iter().any(|a| a == &format!("postgresql-{}-{triple}.tar.gz", r.tag));
                Some((r.tag.clone(), short, has))
            }).collect();
            if win {
                let found = fetch::par_map(&cands, 16, |(_, short, _)| fetch::exists(&edb_url(short)));
                cands.into_iter().zip(found).map(|((_, short, _), ok)| offer(short, ok, "EnterpriseDB has no Windows zip of this version")).collect()
            } else {
                cands.into_iter().map(|(tag, _, has)| offer(tag, has, &no_build)).collect()
            }
        }
        "mysql" => {
            let mut cands: Vec<String> = Vec::new();
            for s in info.versions { let max = if *s == "8.0" { 50 } else { 25 }; for n in 0..=max { cands.push(format!("{s}.{n}")); } }
            let urls = fetch::par_map(&cands, 16, |v| mysql_url(v));
            cands.into_iter().zip(urls).filter(|(_, u)| u.is_some()).map(|(v, _)| offer(v, true, "")).collect()
        }
        "mariadb" => {
            let json: serde_json::Value = serde_json::from_str(&fetch::get_text("https://downloads.mariadb.org/rest-api/mariadb/")?)?;
            let majors: Vec<String> = json["major_releases"].as_array().into_iter().flatten()
                .filter(|m| m["release_status"] == "Stable")
                .filter_map(|m| m["release_id"].as_str().map(|s| s.to_string()))
                .filter(|m| util::cmp_version(m, "10.11") != std::cmp::Ordering::Less).collect();
            let lists = fetch::par_map(&majors, 8, |m| fetch::get_text(&format!("https://downloads.mariadb.org/rest-api/mariadb/{m}/")).ok());
            let mut out = Vec::new();
            for body in lists.into_iter().flatten() {
                let Ok(j) = serde_json::from_str::<serde_json::Value>(&body) else { continue };
                for (ver, rel) in j["releases"].as_object().into_iter().flatten() {
                    out.push(offer(ver.clone(), mariadb_file(ver, rel).is_some(), &no_build));
                }
            }
            out
        }
        "mongodb" => {
            let json = mongodb_catalog()?;
            let target = mongodb_target();
            json["versions"].as_array().into_iter().flatten()
                .filter(|v| v["production_release"].as_bool().unwrap_or(false))
                .filter_map(|v| {
                    let ver = v["version"].as_str()?.to_string();
                    if util::cmp_version(&ver, "6.0.0") == std::cmp::Ordering::Less { return None; }
                    let ok = match &target { Ok((t, a, e)) => mongodb_url(v, t, a, e).is_some(), Err(_) => false };
                    Some(offer(ver, ok, &no_build))
                }).collect()
        }
        "redis" | "mailpit" | "meilisearch" | "rustfs" => {
            let repo = match k {
                "redis" => if win { "redis-windows/redis-windows" } else { "redis/redis" },
                "mailpit" => "axllent/mailpit",
                "meilisearch" => "meilisearch/meilisearch",
                _ => "rustfs/rustfs",
            };
            let rels = fetch::github_releases(repo, 1)?;
            let stable: Vec<&fetch::GhRelease> = rels.iter().filter(|r| !r.prerelease).collect();
            let pick: Vec<&fetch::GhRelease> = if stable.is_empty() { rels.iter().collect() } else { stable };
            pick.into_iter().take(15).map(|r| {
                let v = r.tag.trim_start_matches('v').to_string();
                let (mp, ms, rf) = asset_names(&v);
                let asset = match k {
                    "redis" if win => Some(format!("Redis-{v}-Windows-x64-msys2.zip")),
                    "redis" => None, // compiled from the source release
                    "mailpit" => Some(mp),
                    "meilisearch" => Some(ms),
                    _ => Some(rf),
                };
                let ok = asset.map(|a| r.assets.contains(&a)).unwrap_or(true);
                offer(v, ok, &no_build)
            }).collect()
        }
        _ => Vec::new(),
    };
    if let Err(why) = available(k) { for o in &mut list { o.available = false; o.reason = why.clone(); } }
    list.sort_by(|a, b| util::cmp_version(&b.version, &a.version));
    list.dedup_by(|a, b| a.version == b.version);
    Ok(list)
}

/// Versions of a PostgreSQL extension for each major: (version, has a build here), newest first.
pub fn pg_extension_offers(ext: &str, majors: &[String]) -> R<Vec<(String, Vec<(String, bool)>)>> {
    let win = cfg!(windows);
    Ok(match ext {
        "timescaledb" => {
            let rels = fetch::github_releases(TIMESCALE, 3)?;
            let rels: Vec<&fetch::GhRelease> = rels.iter().filter(|r| !r.prerelease && r.tag.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)).collect();
            majors.iter().map(|m| {
                let asset = format!("timescaledb-postgresql-{m}-windows-amd64.zip");
                let mut v: Vec<(String, bool)> = if win {
                    rels.iter().filter(|r| r.assets.contains(&asset)).map(|r| (r.tag.clone(), true)).collect()
                } else {
                    rels.iter().take(10).map(|r| (r.tag.clone(), false)).collect()
                };
                v.sort_by(|a, b| util::cmp_version(&b.0, &a.0));
                (m.clone(), v)
            }).collect()
        }
        "postgis" if win => {
            let lists = fetch::par_map(majors, 6, |m| postgis_bundles(m).unwrap_or_default());
            majors.iter().cloned().zip(lists).map(|(m, l)| (m, l.into_iter().map(|(v, _)| (v, true)).collect())).collect()
        }
        "pgvector" if win => {
            let rels = fetch::github_releases(PGVECTOR, 2)?;
            majors.iter().map(|m| {
                let mut v: Vec<(String, bool)> = rels.iter().filter_map(|r| {
                    let (ver, mm) = r.tag.split_once('_')?;
                    (mm == m && r.assets.iter().any(|a| a == &format!("vector.v{ver}-pg{m}.zip"))).then(|| (ver.to_string(), true))
                }).collect();
                v.sort_by(|a, b| util::cmp_version(&b.0, &a.0));
                v.dedup();
                (m.clone(), v)
            }).collect()
        }
        _ => majors.iter().map(|m| (m.clone(), Vec::new())).collect(),
    })
}

pub fn pg_extensions_installed(major: &str) -> Vec<(String, String)> {
    let pg = paths::service_dir("postgres", major);
    let mut v: Vec<(String, String)> = std::fs::read_to_string(pg.join(".tez-extensions")).unwrap_or_default().lines()
        .filter_map(|l| l.split_once('=').map(|(a, b)| (a.to_string(), b.to_string()))).collect();
    if pg.join("lib").join("timescaledb.dll").exists() && !v.iter().any(|(n, _)| n == "timescaledb") {
        v.push(("timescaledb".into(), TIMESCALE_VERSION.into()));
    }
    let ts = timescale_versions(&pg);
    if !ts.is_empty() { if let Some(e) = v.iter_mut().find(|(n, _)| n == "timescaledb") { e.1 = ts.join(", "); } }
    v
}

// ------------------------------------------------------------------ settings and runners

/// Settings each service exposes in the dashboard: key, default, what it does.
pub fn options_for(name: &str) -> &'static [(&'static str, &'static str, &'static str)] {
    match kind(name) {
        "postgres" => &[
            ("max_connections", "200", "Connections allowed at once"),
            ("shared_buffers", "512MB", "Memory PostgreSQL keeps for caching data"),
            ("work_mem", "16MB", "Memory per sort or hash step"),
            ("maintenance_work_mem", "256MB", "Memory for indexes and VACUUM"),
            ("effective_cache_size", "4GB", "How much the planner assumes the OS caches"),
            ("synchronous_commit", "off", "off: faster commits; a crash can lose the last moments of writes, never corrupt data"),
            ("timescaledb.max_background_workers", "8", "TimescaleDB background jobs"),
            ("log_min_duration_statement", "-1", "Log queries slower than this many ms (-1 is off)"),
            ("timezone", "UTC", "Server time zone"),
        ],
        "mysql" | "mariadb" => &[
            ("innodb_buffer_pool_size", "512M", "Memory for caching tables and indexes"),
            ("max_connections", "200", "Connections allowed at once"),
            ("character-set-server", "utf8mb4", "Default character set"),
            ("collation-server", "utf8mb4_unicode_ci", "Default collation"),
            ("sql_mode", "", "Empty keeps the server default; e.g. STRICT_TRANS_TABLES"),
            ("innodb_flush_log_at_trx_commit", "2", "2: faster commits for development (1 is fully durable)"),
        ],
        "mongodb" => &[
            ("wiredTigerCacheSizeGB", "1", "Memory for MongoDB's cache, in GB"),
        ],
        "redis" => &[
            ("maxmemory", "0", "Memory limit, e.g. 2gb (0 is no limit)"),
            ("maxmemory-policy", "noeviction", "What happens at the limit (queues need noeviction)"),
            ("save", "3600 1 300 100 60 10000", "Snapshot rules; empty turns snapshots off"),
            ("appendonly", "no", "Write every change to a log as well"),
            ("databases", "16", "Number of databases"),
        ],
        "meilisearch" => &[
            ("master-key", "", "Master key; empty means no authentication (development)"),
            ("max-indexing-memory", "", "e.g. 2 GiB; empty lets Meilisearch decide"),
        ],
        _ => &[],
    }
}

fn opt(sc: &ServiceConfig, name: &str, key: &str) -> String {
    sc.options.get(key).cloned().unwrap_or_else(|| options_for(name).iter().find(|(k, _, _)| *k == key).map(|(_, d, _)| d.to_string()).unwrap_or_default())
}

fn write_postgres_conf(major: &str, port: u16, sc: &ServiceConfig) -> R<()> {
    let data = data("postgres", major);
    let has_ts = dir("postgres", major).join("lib").join("timescaledb.dll").exists();
    let mut s = format!("# Written by tez on every start; change these in the dashboard (Services).\nlisten_addresses = '127.0.0.1'\nport = {port}\nlog_destination = 'stderr'\nlogging_collector = off\n");
    // Portable builds may default the socket to /var/run/postgresql, which needs root; tez
    // connects over TCP, so keep the socket beside the data.
    if !cfg!(windows) { s.push_str(&format!("unix_socket_directories = '{}'\n", paths::fwd(&data))); }
    for (k, _, _) in options_for("postgres") {
        if k.starts_with("timescaledb.") && !has_ts { continue; }
        let v = opt(sc, "postgres", k);
        let quoted = if v.chars().all(|c| c.is_ascii_digit() || c == '-') { v } else { format!("'{}'", v.replace('\'', "''")) };
        s.push_str(&format!("{k} = {quoted}\n"));
    }
    for (k, v) in &sc.options {
        if !options_for("postgres").iter().any(|(kk, _, _)| kk == k) { s.push_str(&format!("{k} = '{}'\n", v.replace('\'', "''"))); }
    }
    if has_ts { s.push_str("shared_preload_libraries = 'timescaledb'\ntimescaledb.telemetry_level = off\n"); }
    std::fs::write(data.join("tez.conf"), s)?;
    Ok(())
}

fn write_redis_conf(port: u16, sc: &ServiceConfig) -> R<PathBuf> {
    let d = paths::data_dir("redis");
    std::fs::create_dir_all(&d)?;
    let conf = d.join("redis.conf");
    let mut s = format!("# Written by tez on every start; change these in the dashboard (Services).\nbind 127.0.0.1\nport {port}\nprotected-mode no\ndir ./\ndaemonize no\nloglevel notice\n");
    for (k, _, _) in options_for("redis") {
        let v = opt(sc, "redis", k);
        if *k == "save" && v.trim().is_empty() { s.push_str("save \"\"\n"); } else { s.push_str(&format!("{k} {v}\n")); }
    }
    std::fs::write(&conf, s)?;
    Ok(conf)
}

pub fn port_of(key: &str, sc: &ServiceConfig) -> u16 { if sc.port == 0 { info(key).map(|i| i.port).unwrap_or(0) } else { sc.port } }
pub fn version_of(key: &str, sc: &ServiceConfig) -> String { if sc.version.is_empty() { info(key).map(|i| i.version.to_string()).unwrap_or_default() } else { sc.version.clone() } }

/// What the daemon runs for an enabled, installed service (`key` is "postgres" or "postgres@16").
pub fn spec(key: &str, sc: &ServiceConfig) -> R<Spec> {
    let k = kind(key);
    let version = version_of(key, sc);
    let port = port_of(key, sc);
    let exe = exe(k, &version);
    let data = data(k, &version);
    std::fs::create_dir_all(&data)?;
    let base = dir(k, &version);
    let s = Spec::new(key, "service", exe.clone()).port(port).cwd(data.clone());
    Ok(match k {
        "postgres" => {
            write_postgres_conf(&version, port, sc)?;
            s.args(["-D".to_string(), paths::fwd(&data)])
                .stop_with(base.join("bin").join(sys::exe("pg_ctl")), vec!["stop".into(), "-D".into(), paths::fwd(&data), "-m".into(), "fast".into(), "-w".into(), "-t".into(), "20".into()])
        }
        "mysql" | "mariadb" => {
            let mut a: Vec<String> = Vec::new();
            let ini = data.join("my.ini");
            if k == "mariadb" && ini.exists() { a.push(format!("--defaults-file={}", ini.display())); } else { a.push("--no-defaults".into()); }
            a.push(format!("--basedir={}", base.display()));
            a.push(format!("--datadir={}", data.display()));
            a.push(format!("--port={port}"));
            a.push("--bind-address=127.0.0.1".into());
            if cfg!(windows) {
                a.push("--console".into());
            } else {
                // Its own socket (not /tmp/mysql.sock, which another MySQL may own).
                a.push(format!("--socket={}", data.join("mysqld.sock").display()));
            }
            if k == "mysql" { a.push("--mysqlx=OFF".into()); }
            for (ok, _, _) in options_for(k) {
                let v = opt(sc, k, ok);
                if !v.is_empty() { a.push(format!("--{ok}={v}")); }
            }
            let admin = first_existing(&base.join("bin"), &[sys::exe("mariadb-admin").as_str(), sys::exe("mysqladmin").as_str()]);
            s.args(a).stop_with(admin, vec!["-u".into(), "root".into(), "-h".into(), "127.0.0.1".into(), format!("--port={port}"), "shutdown".into()])
        }
        "mongodb" => s.args([
            "--dbpath".to_string(), paths::fwd(&data),
            "--port".into(), port.to_string(),
            "--bind_ip".into(), "127.0.0.1".into(),
            "--wiredTigerCacheSizeGB".into(), opt(sc, k, "wiredTigerCacheSizeGB"),
        ]),
        "redis" => {
            let conf = write_redis_conf(port, sc)?;
            s.args(["redis.conf".to_string()])
                .cwd(conf.parent().unwrap().to_path_buf())
                .stop_with(base.join(sys::exe("redis-cli")), vec!["-p".into(), port.to_string(), "shutdown".into()])
        }
        "mailpit" => s
            .args([
                "--smtp".to_string(), format!("127.0.0.1:{port}"),
                "--listen".into(), format!("127.0.0.1:{MAILPIT_UI_PORT}"),
                "--database".into(), paths::fwd(&data.join("mailpit.db")),
            ])
            .url(format!("http://127.0.0.1:{MAILPIT_UI_PORT}")),
        "meilisearch" => {
            let mut a = vec![
                "--db-path".to_string(), paths::fwd(&data.join("db")),
                "--dump-dir".into(), paths::fwd(&data.join("dumps")),
                "--http-addr".into(), format!("127.0.0.1:{port}"),
                "--env".into(), "development".into(),
                "--no-analytics".into(),
            ];
            let key = opt(sc, k, "master-key");
            if !key.is_empty() { a.push("--master-key".into()); a.push(key); }
            let mem = opt(sc, k, "max-indexing-memory");
            if !mem.is_empty() { a.push("--max-indexing-memory".into()); a.push(mem); }
            s.args(a).url(format!("http://127.0.0.1:{port}"))
        }
        "rustfs" => {
            std::fs::create_dir_all(data.join("volume"))?;
            s.args([
                "server".to_string(),
                "--address".into(), format!("127.0.0.1:{port}"),
                "--console-enable".into(),
                "--console-address".into(), format!("127.0.0.1:{S3_CONSOLE_PORT}"),
                "--access-key".into(), S3_KEY.into(),
                "--secret-key".into(), S3_SECRET.into(),
                paths::fwd(&data.join("volume")),
            ])
            .url(format!("http://127.0.0.1:{S3_CONSOLE_PORT}"))
        }
        _ => bail!("no runner for {k}"),
    })
}

// ------------------------------------------------------------------ databases

/// psql against tez's main PostgreSQL.
pub fn psql(cfg: &Config, db: &str, sql: &str) -> R<String> {
    let sc = cfg.services.get("postgres").cloned().unwrap_or_default();
    let version = version_of("postgres", &sc);
    let port = port_of("postgres", &sc);
    let out = std::process::Command::new(dir("postgres", &version).join("bin").join(sys::exe("psql"))).hidden()
        .args(["-h", "127.0.0.1", "-p", &port.to_string(), "-U", "postgres", "-d", db, "-v", "ON_ERROR_STOP=1", "-tAc", sql])
        .output()?;
    if !out.status.success() { bail!("{}", String::from_utf8_lossy(&out.stderr).trim()); }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn has_timescale(cfg: &Config) -> bool {
    let sc = cfg.services.get("postgres").cloned().unwrap_or_default();
    dir("postgres", &version_of("postgres", &sc)).join("lib").join("timescaledb.dll").exists()
}

/// Create a role and database the way a project's .env expects them (idempotent). Like the
/// TimescaleDB Docker image, the extension is created in template1 and in the new database.
pub fn ensure_database(cfg: &Config, database: &str, user: &str, password: &str) -> R<()> {
    let q = |s: &str| s.replace('\'', "''");
    let ident = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    if !user.is_empty() && user != "postgres" {
        let exists = psql(cfg, "postgres", &format!("SELECT 1 FROM pg_roles WHERE rolname = '{}'", q(user)))?;
        if exists.is_empty() {
            psql(cfg, "postgres", &format!("CREATE ROLE {} LOGIN SUPERUSER PASSWORD '{}'", ident(user), q(password)))?;
        }
    }
    if has_timescale(cfg) { let _ = psql(cfg, "template1", "CREATE EXTENSION IF NOT EXISTS timescaledb"); }
    let exists = psql(cfg, "postgres", &format!("SELECT 1 FROM pg_database WHERE datname = '{}'", q(database)))?;
    if exists.is_empty() {
        let owner = if user.is_empty() { "postgres".to_string() } else { user.to_string() };
        psql(cfg, "postgres", &format!("CREATE DATABASE {} OWNER {}", ident(database), ident(&owner)))?;
    }
    if has_timescale(cfg) { let _ = psql(cfg, database, "CREATE EXTENSION IF NOT EXISTS timescaledb"); }
    Ok(())
}

/// The mysql/mariadb client for one instance.
pub fn mysql_exec(cfg: &Config, key: &str, sql: &str) -> R<String> {
    let sc = cfg.services.get(key).cloned().unwrap_or_default();
    let version = version_of(key, &sc);
    let bin = dir(kind(key), &version).join("bin");
    let client = first_existing(&bin, &[sys::exe("mariadb").as_str(), sys::exe("mysql").as_str()]);
    let out = std::process::Command::new(client).hidden()
        .args(["-u", "root", "-h", "127.0.0.1", &format!("--port={}", port_of(key, &sc)), "-N", "-e", sql])
        .output()?;
    if !out.status.success() { bail!("{}", String::from_utf8_lossy(&out.stderr).trim()); }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn ensure_mysql_database(cfg: &Config, key: &str, database: &str, user: &str, password: &str) -> R<()> {
    let q = |s: &str| s.replace('\\', "\\\\").replace('\'', "\\'");
    let ident = |s: &str| format!("`{}`", s.replace('`', "``"));
    mysql_exec(cfg, key, &format!("CREATE DATABASE IF NOT EXISTS {} CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci", ident(database)))?;
    if !user.is_empty() && user != "root" {
        mysql_exec(cfg, key, &format!(
            "CREATE USER IF NOT EXISTS '{u}'@'localhost' IDENTIFIED BY '{p}'; CREATE USER IF NOT EXISTS '{u}'@'127.0.0.1' IDENTIFIED BY '{p}'; GRANT ALL PRIVILEGES ON *.* TO '{u}'@'localhost'; GRANT ALL PRIVILEGES ON *.* TO '{u}'@'127.0.0.1'; FLUSH PRIVILEGES",
            u = q(user), p = q(password)
        ))?;
    }
    Ok(())
}

/// The config key of the running instance of a kind, if any (prefers the default instance).
pub fn first_instance(cfg: &Config, kind_name: &str) -> Option<String> {
    if cfg.services.contains_key(kind_name) { return Some(kind_name.to_string()); }
    cfg.services.keys().find(|k| kind(k) == kind_name).cloned()
}
