//! Services (Herd Pro's "Services"), all native Windows builds bound to 127.0.0.1, data in ~/.tez/data:
//!
//! * PostgreSQL 14 to 18 (EnterpriseDB builds) with TimescaleDB (any version with a Windows build;
//!   several can sit side by side), PostGIS and pgvector
//! * MySQL 8.0 / 8.4, MariaDB 10.11 to 13, MongoDB 6 to 8.2
//! * Redis, Mailpit, Meilisearch, RustFS (S3, in place of MinIO, whose binaries were withdrawn)
//!
//! A service can run several versions at once: the first is named after the service ("postgres"),
//! more are named `<service>@<version>` ("postgres@16") with their own port and data.

use crate::config::{Config, ServiceConfig};
use crate::fetch;
use crate::paths;
use crate::spec::Spec;
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
    match kind(name) {
        "postgres" => d.join("bin").join("postgres.exe"),
        "mysql" => d.join("bin").join("mysqld.exe"),
        "mariadb" => first_existing(&d.join("bin"), &["mariadbd.exe", "mysqld.exe"]),
        "mongodb" => d.join("bin").join("mongod.exe"),
        "redis" => d.join("redis-server.exe"),
        "mailpit" => d.join("mailpit.exe"),
        "meilisearch" => d.join("meilisearch.exe"),
        "rustfs" => d.join("rustfs.exe"),
        other => d.join(format!("{other}.exe")),
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

fn postgres_full_version(v: &str) -> R<String> {
    if v.contains('.') { return Ok(v.to_string()); }
    let major: u32 = v.parse().map_err(|_| format!("'{v}' is not a PostgreSQL version"))?;
    for minor in (0..=20).rev() {
        let full = format!("{major}.{minor}");
        if fetch::exists(&edb_url(&full)) { return Ok(full); }
    }
    bail!("no PostgreSQL {major} build found at EnterpriseDB")
}

/// Newest production MongoDB in a series, from MongoDB's own release catalog.
fn mongodb_release(series: &str) -> R<(String, String)> {
    let body = fetch::get_text("https://downloads.mongodb.org/full.json")?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    let mut best: Option<(String, String)> = None;
    for v in json["versions"].as_array().into_iter().flatten() {
        let ver = v["version"].as_str().unwrap_or_default();
        if !v["production_release"].as_bool().unwrap_or(false) || !ver.starts_with(&format!("{series}.")) { continue; }
        let url = v["downloads"].as_array().into_iter().flatten()
            .find(|d| d["target"] == "windows" && d["arch"] == "x86_64" && d["edition"] == "base")
            .and_then(|d| d["archive"]["url"].as_str()).map(|s| s.to_string());
        if let Some(url) = url {
            if best.as_ref().map(|(b, _)| util::cmp_version(ver, b) == std::cmp::Ordering::Greater).unwrap_or(true) { best = Some((ver.to_string(), url)); }
        }
    }
    best.ok_or_else(|| format!("no Windows build of MongoDB {series}").into())
}

/// Newest MariaDB in a series, from MariaDB's download API.
fn mariadb_release(series: &str) -> R<(String, String)> {
    let body = fetch::get_text(&format!("https://downloads.mariadb.org/rest-api/mariadb/{series}/latest/"))?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    if let Some(rels) = json["releases"].as_object() {
        for (ver, rel) in rels {
            for f in rel["files"].as_array().into_iter().flatten() {
                let name = f["file_name"].as_str().unwrap_or_default();
                if name.ends_with("-winx64.zip") {
                    let url = f["file_download_url"].as_str().unwrap_or_default().replace("http://", "https://");
                    return Ok((ver.clone(), url));
                }
            }
        }
    }
    bail!("no Windows build of MariaDB {series}")
}

/// Newest MySQL patch in a series (dev.mysql.com has no catalog, so probe).
fn mysql_release(series: &str) -> R<(String, String)> {
    for n in (0..=50).rev() {
        let ver = format!("{series}.{n}");
        let url = format!("https://dev.mysql.com/get/Downloads/MySQL-{series}/mysql-{ver}-winx64.zip");
        if fetch::exists(&url) { return Ok((ver, url)); }
    }
    bail!("no Windows build of MySQL {series}")
}

// ------------------------------------------------------------------ install

/// Download and set up a service version. Returns the version to store in the config.
pub fn install(name: &str, version: Option<&str>) -> R<String> {
    let info = info(name)?;
    let k = kind(name);
    let version = version.unwrap_or(info.version).trim_start_matches('v').to_string();
    let d = dir(k, &version);
    let stored = match k {
        "postgres" => {
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
        "mongodb" => {
            let (full, url) = mongodb_release(&series(k, &version))?;
            util::step(&format!("Downloading MongoDB {full}"));
            let zip = fetch::download(&url, &format!("mongodb-windows-x86_64-{full}.zip"))?;
            fetch::extract_flat(&zip, &d, &["*.pdb"])?;
            std::fs::write(d.join(".tez-version"), &full)?;
            series(k, &full)
        }
        "mariadb" => {
            let (full, url) = mariadb_release(&series(k, &version))?;
            util::step(&format!("Downloading MariaDB {full}"));
            let zip = fetch::download(&url, &format!("mariadb-{full}-winx64.zip"))?;
            fetch::extract_flat(&zip, &d, &["*.pdb", "*/mysql-test", "*/sql-bench"])?;
            std::fs::write(d.join(".tez-version"), &full)?;
            init_mariadb(&series(k, &full))?;
            series(k, &full)
        }
        "mysql" => {
            let (full, url) = mysql_release(&series(k, &version))?;
            util::step(&format!("Downloading MySQL {full}"));
            let zip = fetch::download(&url, &format!("mysql-{full}-winx64.zip"))?;
            fetch::extract_flat(&zip, &d, &["*.pdb", "*/lib/debug", "*/mysql-test", "*/docs"])?;
            std::fs::write(d.join(".tez-version"), &full)?;
            init_mysql(&series(k, &full))?;
            series(k, &full)
        }
        "redis" => {
            let file = format!("Redis-{version}-Windows-x64-msys2.zip");
            let zip = fetch::download(&format!("https://github.com/redis-windows/redis-windows/releases/download/{version}/{file}"), &file)?;
            fetch::extract_flat(&zip, &d, &[])?;
            version.clone()
        }
        "mailpit" => {
            let zip = fetch::download(&format!("https://github.com/axllent/mailpit/releases/download/v{version}/mailpit-windows-amd64.zip"), &format!("mailpit-{version}-windows-amd64.zip"))?;
            fetch::extract(&zip, &d, &[])?;
            version.clone()
        }
        "meilisearch" => {
            let file = fetch::download(&format!("https://github.com/meilisearch/meilisearch/releases/download/v{version}/meilisearch-windows-amd64.exe"), &format!("meilisearch-{version}-windows-amd64.exe"))?;
            std::fs::create_dir_all(&d)?;
            std::fs::copy(file, d.join("meilisearch.exe"))?;
            version.clone()
        }
        "rustfs" => {
            let file = format!("rustfs-windows-x86_64-v{version}.zip");
            let zip = fetch::download(&format!("https://github.com/rustfs/rustfs/releases/download/{version}/{file}"), &file)?;
            fetch::extract(&zip, &d, &[])?;
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
    let out = std::process::Command::new(bin.join("initdb.exe"))
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
    let bin = dir("mariadb", s).join("bin");
    let tool = first_existing(&bin, &["mariadb-install-db.exe", "mysql_install_db.exe"]);
    util::step("Creating the MariaDB data folder (user root, no password, local only)");
    let out = std::process::Command::new(tool).arg(format!("--datadir={}", data.display())).output()?;
    if !out.status.success() { bail!("mariadb-install-db failed: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)); }
    Ok(())
}

fn init_mysql(s: &str) -> R<()> {
    let data = data("mysql", s);
    if data.join("mysql").exists() { return Ok(()); }
    let d = dir("mysql", s);
    util::step("Creating the MySQL data folder (user root, no password, local only)");
    let out = std::process::Command::new(d.join("bin").join("mysqld.exe"))
        .args(["--initialize-insecure", "--console"])
        .arg(format!("--basedir={}", d.display()))
        .arg(format!("--datadir={}", data.display()))
        .output()?;
    if !out.status.success() { bail!("mysqld --initialize failed: {}", String::from_utf8_lossy(&out.stderr)); }
    Ok(())
}

// ------------------------------------------------------------------ PostgreSQL extensions

fn copy_tree(src: &Path, dest: &Path) -> R<()> {
    std::fs::create_dir_all(dest)?;
    for e in std::fs::read_dir(src)?.flatten() {
        let p = e.path();
        let to = dest.join(e.file_name());
        if p.is_dir() { copy_tree(&p, &to)?; } else { std::fs::copy(&p, &to)?; }
    }
    Ok(())
}

fn timescale_versions(pg: &Path) -> Vec<String> {
    std::fs::read_to_string(pg.join(".tez-timescale")).unwrap_or_default().lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Add an extension to one PostgreSQL major. Returns the version installed.
pub fn install_pg_extension(major: &str, ext: &str, version: Option<&str>) -> R<String> {
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
            let listing = fetch::get_text(&format!("https://download.osgeo.org/postgis/windows/pg{major}/"))?;
            let mut zips: Vec<String> = fetch::hrefs(&listing).into_iter().filter(|h| h.starts_with(&format!("postgis-bundle-pg{major}-")) && h.ends_with("x64.zip")).collect();
            zips.sort_by(|a, b| util::cmp_version(&a.replace(|c: char| !c.is_ascii_digit() && c != '.', "."), &b.replace(|c: char| !c.is_ascii_digit() && c != '.', ".")));
            let file = zips.pop().ok_or_else(|| format!("no PostGIS bundle for PostgreSQL {major}"))?;
            util::step(&format!("Downloading {file}"));
            let zip = fetch::download(&format!("https://download.osgeo.org/postgis/windows/pg{major}/{file}"), &file)?;
            fetch::extract(&zip, &tmp, &[])?;
            let entries: Vec<_> = std::fs::read_dir(&tmp)?.flatten().collect();
            let root = if entries.len() == 1 && entries[0].path().is_dir() { entries[0].path() } else { tmp.clone() };
            copy_tree(&root, &pg)?;
            file.trim_start_matches(&format!("postgis-bundle-pg{major}-")).trim_end_matches("x64.zip").to_string()
        }
        "pgvector" | "vector" => {
            let tag = match version { Some(v) => v.to_string(), None => fetch::github_latest_tag("andreiramani/pgvector_pgsql_windows")? };
            let file = format!("vector.v{tag}-pg{major}.zip");
            let zip = fetch::download(&format!("https://github.com/andreiramani/pgvector_pgsql_windows/releases/download/{tag}/{file}"), &file)?;
            fetch::extract(&zip, &tmp, &[])?;
            place_by_type(&tmp, &pg)?;
            tag
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
                .stop_with(base.join("bin").join("pg_ctl.exe"), vec!["stop".into(), "-D".into(), paths::fwd(&data), "-m".into(), "fast".into(), "-w".into(), "-t".into(), "20".into()])
        }
        "mysql" | "mariadb" => {
            let mut a: Vec<String> = Vec::new();
            let ini = data.join("my.ini");
            if k == "mariadb" && ini.exists() { a.push(format!("--defaults-file={}", ini.display())); } else { a.push("--no-defaults".into()); }
            a.push(format!("--basedir={}", base.display()));
            a.push(format!("--datadir={}", data.display()));
            a.push(format!("--port={port}"));
            a.push("--bind-address=127.0.0.1".into());
            a.push("--console".into());
            if k == "mysql" { a.push("--mysqlx=OFF".into()); }
            for (ok, _, _) in options_for(k) {
                let v = opt(sc, k, ok);
                if !v.is_empty() { a.push(format!("--{ok}={v}")); }
            }
            let admin = first_existing(&base.join("bin"), &["mariadb-admin.exe", "mysqladmin.exe"]);
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
                .stop_with(base.join("redis-cli.exe"), vec!["-p".into(), port.to_string(), "shutdown".into()])
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
    let out = std::process::Command::new(dir("postgres", &version).join("bin").join("psql.exe"))
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
    let client = first_existing(&bin, &["mariadb.exe", "mysql.exe"]);
    let out = std::process::Command::new(client)
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
