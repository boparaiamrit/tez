//! The version catalog: every version of every service, PostgreSQL extension and PHP that tez
//! can install on this machine, straight from each publisher's own listing (MongoDB's
//! full.json, MariaDB's REST API, MySQL's archives, EnterpriseDB, GitHub releases,
//! windows.php.net, static-php.dev, PECL).
//!
//! Building it makes a few hundred small requests (HEAD checks run in parallel), so it is kept
//! on disk for six hours and refreshed in the background; whether a version is installed is
//! read from disk on every request.

use crate::fetch;
use crate::paths;
use crate::php;
use crate::services;
use crate::sys;
use crate::util::{self, R};
use crate::bail;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const TTL: u64 = 6 * 3600;
static REFRESHING: AtomicBool = AtomicBool::new(false);

fn now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

fn cache_file() -> PathBuf { paths::cache_dir().join("catalog.json") }

fn age(p: &Path) -> u64 {
    p.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs()).unwrap_or(u64::MAX)
}

fn read_json(p: &Path) -> Option<Value> { std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()) }

fn write_json(p: &Path, v: &Value) {
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() { let _ = std::fs::rename(&tmp, p); }
}

/// GET /api/catalog. `refresh` rebuilds it now; otherwise a stale copy is served while a fresh
/// one is built in the background.
pub fn get(refresh: bool) -> Value {
    let file = cache_file();
    let cached = if refresh { None } else { read_json(&file) };
    let v = match cached {
        Some(c) => {
            if age(&file) > TTL && !REFRESHING.swap(true, Ordering::SeqCst) {
                std::thread::spawn(move || {
                    let v = build();
                    write_json(&cache_file(), &v);
                    REFRESHING.store(false, Ordering::SeqCst);
                });
            }
            c
        }
        None => {
            let v = build();
            write_json(&file, &v);
            v
        }
    };
    mark_installed(v)
}

fn pg_majors() -> Vec<String> {
    let mut majors: Vec<String> = services::info("postgres").map(|i| i.versions.iter().map(|s| s.to_string()).collect()).unwrap_or_default();
    if let Ok(rd) = std::fs::read_dir(paths::service_dir("postgres", "0").parent().unwrap_or(Path::new("."))) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.chars().all(|c| c.is_ascii_digit()) && !majors.contains(&n) { majors.push(n); }
        }
    }
    majors.sort_by(|a, b| util::cmp_version(b, a));
    majors
}

fn build() -> Value {
    let kinds: Vec<&'static str> = services::CATALOG.iter().map(|i| i.name).collect();
    let majors = pg_majors();
    let (svc, pgx, php_list) = std::thread::scope(|s| {
        let a = s.spawn(|| {
            let lists = fetch::par_map(&kinds, 4, |k| services::offers(k));
            let mut out = serde_json::Map::new();
            for (k, list) in kinds.iter().zip(lists) {
                let title = services::info(k).map(|i| i.title).unwrap_or(k);
                let entry = match list {
                    Ok(list) => json!({ "title": title, "versions": list.iter().map(|o| json!({
                        "version": o.version, "series": services::series(k, &o.version), "available": o.available, "reason": o.reason,
                    })).collect::<Vec<_>>() }),
                    Err(e) => json!({ "title": title, "versions": [], "error": e.to_string() }),
                };
                out.insert(k.to_string(), entry);
            }
            Value::Object(out)
        });
        let b = s.spawn(|| {
            let exts = ["timescaledb", "postgis", "pgvector"];
            let lists = fetch::par_map(&exts, 3, |e| services::pg_extension_offers(e, &majors));
            let mut out = serde_json::Map::new();
            for (e, list) in exts.iter().zip(lists) {
                let mut per = serde_json::Map::new();
                match list {
                    Ok(list) => for (m, vs) in list {
                        per.insert(m, json!(vs.into_iter().map(|(v, bin)| json!({ "version": v, "binary": bin })).collect::<Vec<_>>()));
                    },
                    Err(err) => { per.insert("error".into(), json!(err.to_string())); }
                }
                out.insert(e.to_string(), Value::Object(per));
            }
            Value::Object(out)
        });
        let c = s.spawn(|| match php::all_versions() {
            Ok(list) => json!(list.into_iter().map(|(m, vs)| json!({ "minor": m, "available": php::available_minors().contains(&m.as_str()), "versions": vs })).collect::<Vec<_>>()),
            Err(e) => json!({ "error": e.to_string() }),
        });
        (a.join().unwrap_or(Value::Null), b.join().unwrap_or(Value::Null), c.join().unwrap_or(Value::Null))
    });
    let mut v = json!({ "generated": now(), "os": sys::os(), "arch": sys::arch(), "services": svc, "pg_extensions": pgx, "php": php_list });
    if !services::pg_extensions_available() {
        v["pg_extensions_note"] = json!(format!("TimescaleDB, PostGIS and pgvector builds are for Windows; on {} every PostgreSQL has the contrib extensions (pgcrypto, hstore, citext, pg_trgm, uuid-ossp...)", sys::os()));
    }
    v
}

/// Add `installed` flags from what is on disk now.
fn mark_installed(mut v: Value) -> Value {
    if let Some(map) = v["services"].as_object_mut() {
        for (k, entry) in map.iter_mut() {
            for o in entry["versions"].as_array_mut().into_iter().flatten() {
                let ver = o["version"].as_str().unwrap_or_default().to_string();
                let installed = match k.as_str() {
                    "postgres" | "mysql" | "mariadb" | "mongodb" => services::installed_full(k, &ver)
                        .map(|f| f.trim_end_matches(".0") == ver.trim_end_matches(".0")).unwrap_or(false) && services::is_installed(k, &ver),
                    _ => services::is_installed(k, &ver),
                };
                o["installed"] = json!(installed);
            }
        }
    }
    if let Some(map) = v["pg_extensions"].as_object_mut() {
        for (ext, per) in map.iter_mut() {
            let Some(per) = per.as_object_mut() else { continue };
            for (major, list) in per.iter_mut() {
                let pg = paths::service_dir("postgres", major);
                let ts = services::timescale_versions(&pg);
                let have = services::pg_extensions_installed(major);
                for o in list.as_array_mut().into_iter().flatten() {
                    let ver = o["version"].as_str().unwrap_or_default().to_string();
                    let installed = if ext == "timescaledb" { ts.contains(&ver) || (ts.is_empty() && have.iter().any(|(e, v)| e == ext && v == &ver)) }
                        else { have.iter().any(|(e, v)| e == ext && v == &ver) };
                    o["installed"] = json!(installed);
                }
            }
        }
    }
    let installed: Vec<(String, String)> = php::installed();
    if let Some(list) = v["php"].as_array_mut() {
        for o in list {
            let m = o["minor"].as_str().unwrap_or_default().to_string();
            o["installed"] = installed.iter().find(|(im, _)| *im == m).map(|(_, v)| json!(v)).unwrap_or(Value::Null);
        }
    }
    v
}

/// GET /api/php/<minor>/pecl/<ext>: every release of an extension, newest first, and whether
/// PECL has a Windows DLL of it for this PHP (else tez compiles it).
pub fn pecl_versions(minor: &str, ext: &str) -> R<Value> {
    if ext.is_empty() || !ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') { bail!("'{ext}' is not an extension name"); }
    let minor = php::normalize(minor)?;
    let file = paths::cache_dir().join("pecl-versions").join(format!("{ext}-{minor}.json"));
    let mut v = match read_json(&file) {
        Some(c) if age(&file) < TTL => c,
        _ => {
            let source = php::pecl_source_releases(ext);
            let win = if cfg!(windows) { php::pecl_windows_versions(ext) } else { Vec::new() };
            let mut list: Vec<(String, String)> = source.clone();
            for w in &win {
                if !list.iter().any(|(v, _)| v.eq_ignore_ascii_case(w)) {
                    list.push((w.clone(), if util::is_prerelease(w) { "beta".into() } else { "stable".into() }));
                }
            }
            if list.is_empty() { bail!("PECL lists no release of '{ext}'"); }
            list.sort_by(|a, b| util::cmp_version(&b.0, &a.0));
            list.truncate(40);
            let bins: Vec<bool> = if cfg!(windows) {
                fetch::par_map(&list, 12, |(v, _)| win.iter().any(|w| w == v) && php::pecl_has_dll(ext, v, &minor))
            } else { vec![false; list.len()] };
            let v = json!({
                "ext": ext, "minor": minor,
                "versions": list.iter().zip(bins).map(|((v, s), b)| json!({ "version": v, "stability": s, "binary": b, "source": source.iter().any(|(sv, _)| sv == v) })).collect::<Vec<_>>(),
                "builds_from_source": cfg!(windows),
            });
            write_json(&file, &v);
            v
        }
    };
    v["installed"] = php::pecl_installed_version(&minor, ext).map(Value::String).unwrap_or(Value::Null);
    if !php::LOADABLE_EXTENSIONS { v["note"] = json!(php::static_note(&minor)); }
    Ok(v)
}
