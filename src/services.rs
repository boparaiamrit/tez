//! Services (Herd Pro's "Services"): PostgreSQL + TimescaleDB, Redis, Mailpit, Meilisearch and
//! RustFS (S3, in place of MinIO, whose binaries were withdrawn in 2025). All native Windows builds,
//! bound to 127.0.0.1 only, with data under `~/.tez/data`.

use crate::config::{Config, ServiceConfig};
use crate::fetch;
use crate::paths;
use crate::spec::Spec;
use crate::util::{self, R};
use crate::bail;
use std::path::PathBuf;

pub struct Info {
    pub name: &'static str,
    pub title: &'static str,
    pub version: &'static str,
    pub port: u16,
    pub about: &'static str,
}

pub const CATALOG: &[Info] = &[
    Info { name: "postgres", title: "PostgreSQL", version: "17", port: 5432, about: "PostgreSQL with TimescaleDB, pg_dump and psql" },
    Info { name: "redis", title: "Redis", version: "8.10.3", port: 6379, about: "Redis for cache, sessions, queues and Horizon" },
    Info { name: "mailpit", title: "Mailpit", version: "1.34.0", port: 1025, about: "Catches every email; web inbox on port 8025" },
    Info { name: "meilisearch", title: "Meilisearch", version: "1.57.0", port: 7700, about: "Search engine for Laravel Scout" },
    Info { name: "rustfs", title: "S3 (RustFS)", version: "1.0.6", port: 9000, about: "S3-compatible storage; console on port 9001" },
];

pub const TIMESCALE_VERSION: &str = "2.32.1";
pub const MAILPIT_UI_PORT: u16 = 8025;
pub const S3_CONSOLE_PORT: u16 = 9001;
pub const S3_KEY: &str = "tez";
pub const S3_SECRET: &str = "tez-secret";

pub fn info(name: &str) -> R<&'static Info> {
    match CATALOG.iter().find(|i| i.name == name) {
        Some(i) => Ok(i),
        None => bail!("unknown service '{name}'; choose one of {}", CATALOG.iter().map(|i| i.name).collect::<Vec<_>>().join(", ")),
    }
}

/// PostgreSQL majors map to the newest EDB build that exists.
fn postgres_full_version(major_or_full: &str) -> R<String> {
    if major_or_full.contains('.') { return Ok(major_or_full.to_string()); }
    let major: u32 = major_or_full.parse().map_err(|_| format!("'{major_or_full}' is not a PostgreSQL version"))?;
    for minor in (0..=20).rev() {
        let v = format!("{major}.{minor}");
        if fetch::exists(&edb_url(&v)) { return Ok(v); }
    }
    bail!("no PostgreSQL {major} build found at EnterpriseDB")
}

fn edb_url(v: &str) -> String { format!("https://get.enterprisedb.com/postgresql/postgresql-{v}-1-windows-x64-binaries.zip") }

fn pg_major(version: &str) -> String { version.split('.').next().unwrap_or("17").to_string() }

pub fn dir(name: &str, version: &str) -> PathBuf {
    if name == "postgres" { paths::service_dir(name, &pg_major(version)) } else { paths::service_dir(name, version) }
}

pub fn exe(name: &str, version: &str) -> PathBuf {
    let d = dir(name, version);
    match name {
        "postgres" => d.join("bin").join("postgres.exe"),
        "redis" => d.join("redis-server.exe"),
        "mailpit" => d.join("mailpit.exe"),
        "meilisearch" => d.join("meilisearch.exe"),
        "rustfs" => d.join("rustfs.exe"),
        _ => d.join(format!("{name}.exe")),
    }
}

pub fn is_installed(name: &str, version: &str) -> bool { exe(name, version).exists() }

pub fn data(name: &str, version: &str) -> PathBuf {
    if name == "postgres" { paths::data_dir(&format!("postgres-{}", pg_major(version))) } else { paths::data_dir(name) }
}

/// Download and set up a service. Returns the installed version.
pub fn install(name: &str, version: Option<&str>) -> R<String> {
    let info = info(name)?;
    let version = version.unwrap_or(info.version).trim_start_matches('v').to_string();
    let d = dir(name, &version);
    match name {
        "postgres" => {
            let full = postgres_full_version(&version)?;
            util::step(&format!("Downloading PostgreSQL {full} (EnterpriseDB build)"));
            let zip = fetch::download(&edb_url(&full), &format!("postgresql-{full}-1-windows-x64-binaries.zip"))?;
            util::step("Unpacking (server, client tools and extensions; pgAdmin and docs left out)");
            fetch::extract_flat(&zip, &d, &["pgsql/pgAdmin 4", "pgsql/StackBuilder", "pgsql/doc", "pgsql/symbols", "pgsql/include"])?;
            std::fs::write(d.join(".tez-version"), &full)?;
            install_timescale(&pg_major(&full))?;
            init_postgres(&version)?;
            util::ok(&format!("PostgreSQL {full} with TimescaleDB {TIMESCALE_VERSION}"));
            return Ok(pg_major(&full));
        }
        "redis" => {
            let file = format!("Redis-{version}-Windows-x64-msys2.zip");
            let zip = fetch::download(&format!("https://github.com/redis-windows/redis-windows/releases/download/{version}/{file}"), &file)?;
            fetch::extract_flat(&zip, &d, &[])?;
        }
        "mailpit" => {
            let zip = fetch::download(
                &format!("https://github.com/axllent/mailpit/releases/download/v{version}/mailpit-windows-amd64.zip"),
                &format!("mailpit-{version}-windows-amd64.zip"),
            )?;
            fetch::extract(&zip, &d, &[])?;
        }
        "meilisearch" => {
            let file = fetch::download(
                &format!("https://github.com/meilisearch/meilisearch/releases/download/v{version}/meilisearch-windows-amd64.exe"),
                &format!("meilisearch-{version}-windows-amd64.exe"),
            )?;
            std::fs::create_dir_all(&d)?;
            std::fs::copy(file, d.join("meilisearch.exe"))?;
        }
        "rustfs" => {
            let file = format!("rustfs-windows-x86_64-v{version}.zip");
            let zip = fetch::download(&format!("https://github.com/rustfs/rustfs/releases/download/{version}/{file}"), &file)?;
            fetch::extract(&zip, &d, &[])?;
        }
        _ => bail!("cannot install {name}"),
    }
    std::fs::write(d.join(".tez-version"), &version)?;
    std::fs::create_dir_all(data(name, &version))?;
    util::ok(&format!("{} {version} installed", info.title));
    Ok(version)
}

fn install_timescale(major: &str) -> R<()> {
    let file = format!("timescaledb-{TIMESCALE_VERSION}-postgresql-{major}-windows-amd64.zip");
    let url = format!("https://github.com/timescale/timescaledb/releases/download/{TIMESCALE_VERSION}/timescaledb-postgresql-{major}-windows-amd64.zip");
    let zip = match fetch::download(&url, &file) {
        Ok(z) => z,
        Err(e) => { util::warn(&format!("TimescaleDB for PostgreSQL {major} not available: {e}")); return Ok(()); }
    };
    let tmp = paths::cache_dir().join(format!("timescale-{major}"));
    let _ = std::fs::remove_dir_all(&tmp);
    fetch::extract(&zip, &tmp, &[])?;
    let pg = paths::service_dir("postgres", major);
    for entry in std::fs::read_dir(tmp.join("timescaledb"))?.flatten() {
        let p = entry.path();
        let n = p.file_name().unwrap().to_string_lossy().to_string();
        if n.ends_with(".dll") { std::fs::copy(&p, pg.join("lib").join(&n))?; }
        else if n.ends_with(".control") || n.ends_with(".sql") { std::fs::copy(&p, pg.join("share").join("extension").join(&n))?; }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

fn init_postgres(version: &str) -> R<()> {
    let data = data("postgres", version);
    if data.join("PG_VERSION").exists() { return Ok(()); }
    let bin = dir("postgres", version).join("bin");
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

/// Settings each service exposes in the dashboard: key, default, what it does.
pub fn options_for(name: &str) -> &'static [(&'static str, &'static str, &'static str)] {
    match name {
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

fn write_postgres_conf(version: &str, port: u16, sc: &ServiceConfig) -> R<()> {
    let data = data("postgres", version);
    let has_ts = dir("postgres", version).join("lib").join("timescaledb.dll").exists();
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

/// What the daemon runs for an enabled, installed service.
pub fn spec(name: &str, sc: &ServiceConfig) -> R<Spec> {
    let info = info(name)?;
    let version = if sc.version.is_empty() { info.version.to_string() } else { sc.version.clone() };
    let port = if sc.port == 0 { info.port } else { sc.port };
    let exe = exe(name, &version);
    let data = data(name, &version);
    std::fs::create_dir_all(&data)?;
    let s = Spec::new(name, "service", exe.clone()).port(port).cwd(data.clone());
    Ok(match name {
        "postgres" => {
            write_postgres_conf(&version, port, sc)?;
            let bin = dir(name, &version).join("bin");
            s.args(["-D".to_string(), paths::fwd(&data)])
                .stop_with(bin.join("pg_ctl.exe"), vec!["stop".into(), "-D".into(), paths::fwd(&data), "-m".into(), "fast".into(), "-w".into(), "-t".into(), "20".into()])
        }
        "redis" => {
            let conf = write_redis_conf(port, sc)?;
            let cli = dir(name, &version).join("redis-cli.exe");
            s.args(["redis.conf".to_string()])
                .cwd(conf.parent().unwrap().to_path_buf())
                .stop_with(cli, vec!["-p".into(), port.to_string(), "shutdown".into()])
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
            let key = opt(sc, name, "master-key");
            if !key.is_empty() { a.push("--master-key".into()); a.push(key); }
            let mem = opt(sc, name, "max-indexing-memory");
            if !mem.is_empty() { a.push("--max-indexing-memory".into()); a.push(mem); }
            s.args(a).url(format!("http://127.0.0.1:{port}"))
        }
        "rustfs" => {
            std::fs::create_dir_all(data.join("volume"))?;
            s
            .args([
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
        _ => bail!("no runner for {name}"),
    })
}

/// psql against the tez PostgreSQL.
pub fn psql(cfg: &Config, db: &str, sql: &str) -> R<String> {
    let sc = cfg.services.get("postgres").cloned().unwrap_or_default();
    let version = if sc.version.is_empty() { "17".to_string() } else { sc.version };
    let port = if sc.port == 0 { 5432 } else { sc.port };
    let out = std::process::Command::new(dir("postgres", &version).join("bin").join("psql.exe"))
        .args(["-h", "127.0.0.1", "-p", &port.to_string(), "-U", "postgres", "-d", db, "-v", "ON_ERROR_STOP=1", "-tAc", sql])
        .output()?;
    if !out.status.success() { bail!("{}", String::from_utf8_lossy(&out.stderr).trim()); }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Create a role and database the way a project's .env expects them (idempotent).
pub fn ensure_database(cfg: &Config, database: &str, user: &str, password: &str) -> R<()> {
    let q = |s: &str| s.replace('\'', "''");
    let ident = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    if !user.is_empty() && user != "postgres" {
        let exists = psql(cfg, "postgres", &format!("SELECT 1 FROM pg_roles WHERE rolname = '{}'", q(user)))?;
        if exists.is_empty() {
            psql(cfg, "postgres", &format!("CREATE ROLE {} LOGIN SUPERUSER PASSWORD '{}'", ident(user), q(password)))?;
        }
    }
    let exists = psql(cfg, "postgres", &format!("SELECT 1 FROM pg_database WHERE datname = '{}'", q(database)))?;
    if exists.is_empty() {
        let owner = if user.is_empty() { "postgres".to_string() } else { user.to_string() };
        psql(cfg, "postgres", &format!("CREATE DATABASE {} OWNER {}", ident(database), ident(&owner)))?;
    }
    Ok(())
}
