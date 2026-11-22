//! Removing a site, at two levels:
//!
//! * **Remove from tez.** A linked site is unlinked; a folder inside a Sites folder goes on the
//!   `hidden_sites` list, so tez stops serving it while it stays on disk (`unhide` brings it back).
//! * **Delete the folder too.** Only after the site's name is typed back: the folder moves to the
//!   Recycle Bin (Windows) or the Trash (macOS, Linux). Nothing is ever deleted outright. If the
//!   move fails, the site is put back exactly as it was and nothing is deleted.
//!
//! Either way the site's processes stop, its settings go and the web server stops serving it.
//! Its database is dropped only when asked (the name typed back too, since a dropped database
//! cannot be recovered) and only when it lives on a server tez runs here.

use crate::bail;
use crate::config::Config;
use crate::daemon::{self, Shared};
use crate::paths;
use crate::services;
use crate::sites::{self, Site};
use crate::sys::{self, Hidden};
use crate::util::{self, R};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One removal at a time: a second request for the same site then finds it gone.
static BUSY: Mutex<()> = Mutex::new(());

/// A site's database, from its .env.
#[derive(Debug, Clone)]
struct Db {
    connection: String,
    name: String,
    host: String,
    port: u16,
    /// The tez service instance holding it ("postgres", "mysql@8.0"); None when it is elsewhere.
    key: Option<String>,
}

/// Databases tez never drops, whatever a .env says.
const SYSTEM_DBS: &[&str] = &["postgres", "template0", "template1", "mysql", "information_schema", "performance_schema", "sys"];

fn site_db(cfg: &Config, site: &Site) -> Option<Db> {
    let env = util::read_env(&site.path.join(".env"));
    if env.is_empty() { return None; }
    let connection = env.get("DB_CONNECTION").map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "sqlite".into());
    let name = env.get("DB_DATABASE").cloned().unwrap_or_default();
    let host = env.get("DB_HOST").cloned().unwrap_or_else(|| "127.0.0.1".into());
    let (kinds, default_port): (&[&str], u16) = match connection.as_str() {
        "pgsql" => (&["postgres"], 5432),
        "mysql" | "mariadb" => (&["mysql", "mariadb"], 3306),
        _ => (&[], 0),
    };
    let port = env.get("DB_PORT").and_then(|p| p.trim().parse().ok()).unwrap_or(default_port);
    let key = if !kinds.is_empty() && !name.is_empty() && is_local_host(&host) { tez_instance(cfg, kinds, port) } else { None };
    Some(Db { connection, name, host, port, key })
}

/// 127.0.0.1, localhost, ::1, or tez's PostgreSQL socket folder (macOS).
fn is_local_host(host: &str) -> bool {
    let h = host.trim().trim_matches(['[', ']']).to_ascii_lowercase();
    matches!(h.as_str(), "" | "127.0.0.1" | "localhost" | "::1") || sites::same_path(Path::new(host.trim()), &paths::run_dir())
}

/// The tez instance of one of `kinds` listening on `port`.
fn tez_instance(cfg: &Config, kinds: &[&str], port: u16) -> Option<String> {
    for (key, sc) in &cfg.services {
        if kinds.contains(&services::kind(key)) && services::port_of(key, sc) == port { return Some(key.clone()); }
    }
    // The default instance can be installed without an entry in tez.json.
    for k in kinds {
        if cfg.services.contains_key(*k) { continue; }
        if let Ok(info) = services::info(k) {
            if info.port == port && services::is_installed(k, info.version) { return Some(k.to_string()); }
        }
    }
    None
}

fn quote_pg(name: &str) -> String { format!("\"{}\"", name.replace('"', "\"\"")) }
fn quote_my(name: &str) -> String { format!("`{}`", name.replace('`', "``")) }

fn db_exists(cfg: &Config, key: &str, name: &str) -> R<bool> {
    let lit = name.replace('\\', "\\\\").replace('\'', "''");
    let out = if services::kind(key) == "postgres" {
        services::psql_on(cfg, key, "postgres", &format!("SELECT 1 FROM pg_database WHERE datname = '{}'", name.replace('\'', "''")))?
    } else {
        services::mysql_exec(cfg, key, &format!("SELECT 1 FROM information_schema.schemata WHERE schema_name = '{lit}'"))?
    };
    Ok(!out.trim().is_empty())
}

/// Drop one database if it exists. True when it existed.
fn drop_db(cfg: &Config, key: &str, name: &str) -> R<bool> {
    if SYSTEM_DBS.contains(&name.to_ascii_lowercase().as_str()) { bail!("tez never drops the {name} database"); }
    if !db_exists(cfg, key, name)? { return Ok(false); }
    if services::kind(key) == "postgres" {
        // FORCE (PostgreSQL 13+) ends the connections still open to it.
        services::psql_on(cfg, key, "postgres", &format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", quote_pg(name)))?;
    } else {
        services::mysql_exec(cfg, key, &format!("DROP DATABASE IF EXISTS {}", quote_my(name)))?;
    }
    Ok(true)
}

fn db_json(cfg: &Config, db: &Option<Db>) -> Value {
    let Some(d) = db else { return Value::Null };
    let mut v = json!({ "connection": d.connection, "name": d.name, "host": d.host, "port": d.port, "local": d.key.is_some(), "exists": false });
    match &d.key {
        Some(key) => match db_exists(cfg, key, &d.name) {
            Ok(e) => v["exists"] = json!(e),
            Err(_) => v["note"] = json!(format!("{} is not running, so tez cannot tell whether {} exists", key, d.name)),
        },
        None if d.connection == "sqlite" => v["note"] = json!("an SQLite file; it goes with the folder"),
        None if is_local_host(&d.host) => v["note"] = json!(format!("no server tez runs listens on port {}", d.port)),
        None => v["note"] = json!(format!("on {}, not on this computer; tez never touches it", d.host)),
    }
    v
}

/// Bytes in a folder, without following links. With a deadline it stops early and says so.
pub fn dir_size(root: &Path, deadline: Option<Instant>) -> (u64, bool) {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if deadline.map(|t| Instant::now() > t).unwrap_or(false) { return (total, true); }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            if m.file_type().is_symlink() { continue; }
            if m.is_dir() { stack.push(e.path()); } else { total += m.len(); }
        }
    }
    (total, false)
}

/// Uncommitted changes, commits on no remote, and stashes: work that only exists in this folder.
fn git_state(dir: &Path) -> (bool, String) {
    if !dir.join(".git").exists() { return (false, String::new()); }
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git").arg("-C").arg(dir).args(args).hidden().stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
    };
    let changes = git(&["status", "--porcelain"]).map(|s| s.lines().count()).unwrap_or(0);
    let unpushed = git(&["log", "--branches", "--not", "--remotes", "--oneline", "-n", "100"]).map(|s| s.lines().count()).unwrap_or(0);
    let stashes = git(&["stash", "list"]).map(|s| s.lines().count()).unwrap_or(0);
    let mut parts = Vec::new();
    if changes > 0 { parts.push(format!("{changes} uncommitted change{}", if changes == 1 { "" } else { "s" })); }
    if unpushed > 0 {
        let n = if unpushed >= 100 { "100+".to_string() } else { unpushed.to_string() };
        parts.push(format!("{n} commit{} not pushed anywhere", if unpushed == 1 { "" } else { "s" }));
    }
    if stashes > 0 { parts.push(format!("{stashes} stash{}", if stashes == 1 { "" } else { "es" })); }
    (!parts.is_empty(), parts.join(", "))
}

/// The site's processes the daemon is running now ("acmeshop/horizon").
fn running_processes(shared: Option<&Shared>, name: &str) -> Vec<String> {
    let Some(s) = shared else { return Vec::new() };
    let d = s.lock().unwrap_or_else(|e| e.into_inner());
    let group = format!("site:{name}");
    d.procs.iter().filter(|p| p.spec.group == group && p.pid().is_some()).map(|p| p.spec.name.clone()).collect()
}

/// The folder as given and as resolved (links followed), so a check cannot be dodged either way.
fn forms(p: &Path) -> Vec<PathBuf> {
    let mut v = vec![p.to_path_buf()];
    let abs = paths::absolute(p);
    if !sites::same_path(&abs, p) { v.push(abs); }
    v
}

/// Whether tez may move this site's folder to the trash; the reason when not.
pub fn deletable(cfg: &Config, site: &Site) -> Result<(), String> {
    let p = &site.path;
    if p.as_os_str().is_empty() || !p.is_absolute() { return Err(format!("its folder ({}) is not a full path", p.display())); }
    let meta = std::fs::symlink_metadata(p).map_err(|_| "its folder is already gone".to_string())?;
    if meta.file_type().is_symlink() {
        return Err("its folder is a link (symlink or junction); remove the link yourself so what it points to is left alone".into());
    }
    if !meta.is_dir() { return Err("it is not a folder".into()); }
    let home = paths::user_home();
    let tez_home = paths::home();
    for f in forms(p) {
        if f.parent().is_none() { return Err("it is a whole drive".into()); }
        if sites::is_within(&home, &f) { return Err("it is your home folder, or holds it".into()); }
        if sites::is_within(&tez_home, &f) || sites::is_within(&f, &tez_home) { return Err("it holds tez's own files".into()); }
        for root in &cfg.parked {
            if sites::is_within(Path::new(root), &f) { return Err(format!("it is a Sites folder ({root}), or holds one")); }
        }
        for other in sites::all(cfg) {
            if other.name != site.name && sites::is_within(&other.path, &f) { return Err(format!("it holds another site, {}", other.name)); }
        }
    }
    // A parked site is a folder directly inside one of the Sites folders.
    if !site.linked && !cfg.parked.iter().any(|r| p.parent().map(|par| sites::same_path(par, Path::new(r))).unwrap_or(false)) {
        return Err("it is not a folder inside a Sites folder".into());
    }
    Ok(())
}

/// Moving or deleting the folder needs the site's exact name typed back; so does dropping its
/// database, which no Recycle Bin can bring back.
pub fn check_confirm(name: &str, files: bool, database: bool, confirm: &str) -> R<()> {
    if (files || database) && confirm != name {
        let what = match (files, database) {
            (true, true) => "move its folder to the trash and drop its database",
            (true, false) => "move its folder to the trash",
            _ => "drop its database",
        };
        bail!("to {what}, type the site's name exactly: {name}");
    }
    Ok(())
}

fn trash_word() -> &'static str { if cfg!(windows) { "Recycle Bin" } else { "Trash" } }

/// GET /api/site/<name>/delete-preview: what removing the site would touch.
pub fn preview(cfg: &Config, shared: Option<&Shared>, name: &str) -> R<Value> {
    let Some(site) = sites::find(cfg, name) else { bail!("no site named {name}") };
    let exists = site.path.is_dir();
    let (size, partial) = if exists { dir_size(&site.path, Some(Instant::now() + Duration::from_secs(2))) } else { (0, false) };
    let (git_dirty, git_detail) = if exists { git_state(&site.path) } else { (false, String::new()) };
    let (trash, blocked) = match deletable(cfg, &site).and_then(|_| sys::trash_check(&site.path, size)) {
        Ok(label) => (json!(label), Value::Null),
        Err(why) => (Value::Null, json!(why)),
    };
    let db = site_db(cfg, &site);
    Ok(json!({
        "name": site.name, "path": site.path, "exists": exists,
        "kind": if site.linked { "linked" } else { "parked" },
        "processes": running_processes(shared, name),
        "database": db_json(cfg, &db),
        "size_bytes": size, "size_partial": partial,
        "git_dirty": git_dirty, "git_detail": git_detail,
        "trash": trash, "trash_blocked": blocked,
        "hosts": hosts_listed(cfg, &site),
    }))
}

/// Host names of this site that the hosts file still maps.
fn hosts_listed(cfg: &Config, site: &Site) -> Vec<String> {
    let mut mine = vec![site.host(&cfg.tld)];
    mine.extend(site.settings.domains.iter().map(|d| d.to_ascii_lowercase()));
    let present = crate::cli::hosts_file_names();
    mine.into_iter().filter(|h| present.contains(h)).collect()
}

fn reload(shared: Option<&Shared>, notes: &mut Vec<String>) {
    if let Some(s) = shared {
        match daemon::reload(s) {
            Ok(n) => notes.extend(n),
            Err(e) => notes.push(format!("reload: {e}")),
        }
    }
}

/// tez renames the folder aside and back: on Windows that fails while anything inside is open
/// (a terminal or editor in it, a running command), which would leave a half-recycled folder.
fn in_use_check(p: &Path) -> Result<(), String> {
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let probe = p.with_file_name(format!("{name}.tez-check-{}", std::process::id()));
    if probe.exists() { return Err(format!("{} is in the way; remove it first", probe.display())); }
    match std::fs::rename(p, &probe) {
        Ok(()) => {
            for _ in 0..50 {
                if std::fs::rename(&probe, p).is_ok() { return Ok(()); }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(format!("tez renamed it to {} to check it and could not rename it back; rename it to {name} yourself", probe.display()))
        }
        Err(e) if matches!(e.raw_os_error(), Some(5) | Some(32) | Some(33)) || e.kind() == std::io::ErrorKind::PermissionDenied => {
            Err("a program is using it (a terminal, editor or Explorer window open in it, or a command still running there); close it and try again".into())
        }
        Err(e) => Err(format!("it cannot be moved: {e}")),
    }
}

/// POST /api/site/<name>/delete. `shared` is the daemon (None when the CLI runs without one).
pub fn delete(shared: Option<&Shared>, name: &str, body: &Value) -> R<Value> {
    let _one_at_a_time = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    let files = body["files"].as_bool().unwrap_or(false);
    let database = body["database"].as_bool().unwrap_or(false);
    let confirm = body["confirm"].as_str().unwrap_or("");
    let before = Config::load();
    let Some(site) = sites::find(&before, name) else { bail!("no site named {name}") };
    check_confirm(name, files, database, confirm)?;
    let trash = trash_word();
    let mut notes: Vec<String> = Vec::new();

    // Everything is checked before anything changes.
    let folder = std::fs::symlink_metadata(&site.path).is_ok();
    if files && folder {
        deletable(&before, &site).map_err(|why| format!("tez will not move {} to the {trash}: {why}. Nothing was changed.", site.path.display()))?;
        let (size, _) = dir_size(&site.path, None);
        sys::trash_check(&site.path, size).map_err(|why| format!("{} cannot go to the {trash}: {why}. Nothing was changed.", site.path.display()))?;
    }
    if files && !folder { notes.push(format!("its folder ({}) was already gone", site.path.display())); }
    let db = if database { site_db(&before, &site) } else { None };
    if database {
        match &db {
            None => notes.push("it has no .env, so there was no database to drop".into()),
            Some(d) if d.key.is_none() && d.connection == "sqlite" => notes.push("its database is an SQLite file; it goes with the folder".into()),
            Some(d) if d.key.is_none() => notes.push(format!("its database {} on {}:{} is not on a server tez runs here, so it was left alone", d.name, d.host, d.port)),
            Some(d) if SYSTEM_DBS.contains(&d.name.to_ascii_lowercase().as_str()) => bail!("its .env names the {} database, which tez never drops. Nothing was changed.", d.name),
            Some(_) => {}
        }
    }

    // 1. Stop serving it: its settings go, a linked site is unlinked, a parked folder hidden.
    //    The reload stops its processes and takes it out of the web server.
    let mut cfg = before.clone();
    cfg.sites.remove(name);
    if site.linked {
        cfg.links.remove(name);
    } else if !cfg.hidden_sites.iter().any(|h| sites::same_path(Path::new(h), &site.path)) {
        cfg.hidden_sites.push(site.path.display().to_string());
    }
    cfg.save()?;
    reload(shared, &mut notes);

    // 2. The folder, now that nothing of tez's runs in it. Any failure puts the site back.
    let mut trashed = false;
    if files && folder {
        let undo = |why: String| -> R<Value> {
            let _ = before.save();
            reload(shared, &mut Vec::new());
            bail!("{} was not moved to the {trash}: {why}. Nothing was deleted, and {name} is back as it was.", site.path.display())
        };
        if let Err(why) = in_use_check(&site.path) { return undo(why); }
        if let Err(e) = sys::move_to_trash(&site.path) { return undo(e.to_string()); }
        trashed = true;
        // The folder is gone, so it needs no hiding.
        cfg.hidden_sites.retain(|h| !sites::same_path(Path::new(h), &site.path));
        cfg.save()?;
    }

    // 3. The database, only on tez's own servers.
    let mut dropped: Vec<String> = Vec::new();
    if let Some(Db { key: Some(key), name: db_name, .. }) = &db {
        for n in [db_name.clone(), format!("{db_name}_testing")] {
            match drop_db(&cfg, key, &n) {
                Ok(true) => dropped.push(n),
                Ok(false) => {}
                Err(e) => notes.push(format!("could not drop {n}: {e}")),
            }
        }
        if dropped.is_empty() && notes.iter().all(|n| !n.starts_with("could not drop")) { notes.push(format!("its database {db_name} did not exist")); }
    }

    let left = hosts_listed(&cfg, &site);
    if !left.is_empty() {
        notes.push(format!("the hosts file still maps {}; that is harmless, and `tez hosts` tidies it ({})", left.join(", "), if cfg!(windows) { "one admin prompt" } else { "sudo" }));
    }

    let path = site.path.display();
    let mut removed = if trashed {
        format!("Moved {path} to the {trash}; tez no longer serves {name}.")
    } else if site.linked {
        format!("Unlinked {name}; its folder {path} is untouched.")
    } else {
        format!("tez no longer serves {name}; its folder {path} stays where it is.")
    };
    if !dropped.is_empty() { removed.push_str(&format!(" Dropped the database{} {}.", if dropped.len() == 1 { "" } else { "s" }, dropped.join(" and "))); }
    Ok(json!({ "ok": true, "removed": removed, "notes": notes, "trash": if trashed { json!(trash) } else { Value::Null }, "dropped": dropped }))
}

/// GET /api/sites/hidden.
pub fn hidden_list(cfg: &Config) -> Value {
    let list: Vec<Value> = cfg.hidden_sites.iter().map(|h| {
        let p = Path::new(h);
        let name = util::host_label(&p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        json!({ "name": name, "path": h, "exists": p.is_dir() })
    }).collect();
    json!({ "hidden": list })
}

/// POST /api/sites/unhide: serve a hidden folder again. `which` is its path or its site name.
pub fn unhide(shared: Option<&Shared>, which: &str) -> R<Value> {
    let _one_at_a_time = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    let mut cfg = Config::load();
    let which = which.trim();
    let pos = cfg.hidden_sites.iter().position(|h| {
        let p = Path::new(h);
        sites::same_path(p, Path::new(which)) || util::host_label(&p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()) == which
    });
    let Some(i) = pos else { bail!("{which} is not a hidden site (see `tez site hidden`)") };
    let path = cfg.hidden_sites.remove(i);
    cfg.save()?;
    let mut notes = Vec::new();
    reload(shared, &mut notes);
    let name = sites::all(&cfg).into_iter().find(|s| sites::same_path(&s.path, Path::new(&path))).map(|s| s.name);
    Ok(json!({ "ok": true, "path": path, "name": name, "notes": notes }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SiteSettings;

    /// A scratch folder under target/, removed at the end of each test.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let p = std::env::current_dir().unwrap().join("target").join("remove-tests").join(format!("{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Scratch(paths::absolute(&p))
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn site(name: &str, path: &Path, linked: bool) -> Site {
        Site { name: name.into(), path: path.to_path_buf(), root: path.to_path_buf(), php: "8.4".into(), linked, laravel: false, settings: SiteSettings::default() }
    }

    #[test]
    fn confirm_rule() {
        assert!(check_confirm("shop", false, false, "").is_ok(), "removing from tez needs no typing");
        assert!(check_confirm("shop", true, false, "").is_err());
        assert!(check_confirm("shop", true, false, "Shop").is_err(), "the name must match exactly");
        assert!(check_confirm("shop", true, false, " shop").is_err());
        assert!(check_confirm("shop", true, false, "shop").is_ok());
        assert!(check_confirm("shop", false, true, "").is_err(), "dropping a database needs the name too");
        assert!(check_confirm("my-app.v2", true, true, "my-app.v2").is_ok());
    }

    #[test]
    fn safety_checks() {
        let s = Scratch::new("safety");
        let sites_dir = s.0.join("Sites");
        let shop = sites_dir.join("shop");
        let elsewhere = s.0.join("elsewhere").join("api");
        std::fs::create_dir_all(&shop).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let mut cfg = Config::default();
        cfg.parked = vec![sites_dir.display().to_string()];
        cfg.links.insert("api".into(), elsewhere.display().to_string());

        assert!(deletable(&cfg, &site("shop", &shop, false)).is_ok(), "a folder in Sites");
        assert!(deletable(&cfg, &site("api", &elsewhere, true)).is_ok(), "a linked folder elsewhere");

        let e = deletable(&cfg, &site("all", &sites_dir, true)).unwrap_err();
        assert!(e.contains("Sites folder"), "{e}");
        let e = deletable(&cfg, &site("me", &paths::user_home(), true)).unwrap_err();
        assert!(e.contains("home"), "{e}");
        let root = PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" });
        assert!(deletable(&cfg, &site("disk", &root, true)).is_err(), "a drive root");
        assert!(deletable(&cfg, &site("tez", &paths::home(), true)).is_err(), "tez's own folder");
        // A linked folder that holds another site.
        let e = deletable(&cfg, &site("outer", &s.0.join("elsewhere"), true)).unwrap_err();
        assert!(e.contains("another site"), "{e}");
        // Missing, relative, and not a folder.
        assert!(deletable(&cfg, &site("gone", &s.0.join("nope"), true)).unwrap_err().contains("gone"));
        assert!(deletable(&cfg, &site("rel", Path::new("relative/dir"), true)).is_err());
        let file = s.0.join("file.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(deletable(&cfg, &site("file", &file, true)).unwrap_err().contains("not a folder"));
        // A "parked" site that is not directly inside a Sites folder.
        let deep = shop.join("sub");
        std::fs::create_dir_all(&deep).unwrap();
        assert!(deletable(&cfg, &site("sub", &deep, false)).is_err());
    }

    #[test]
    fn symlinked_folder_is_refused() {
        let s = Scratch::new("link");
        let real = s.0.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = s.0.join("link");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&real, &link).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&real, &link).is_ok()
            || Command::new(paths::system32("cmd.exe")).args(["/c", "mklink", "/J"]).arg(&link).arg(&real).hidden().stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        if !made { return; } // no permission to make links here
        let cfg = Config::default();
        let e = deletable(&cfg, &site("link", &link, true)).unwrap_err();
        assert!(e.contains("link"), "{e}");
        assert!(real.is_dir(), "the target is untouched");
    }

    #[test]
    fn hidden_sites_are_not_served() {
        let s = Scratch::new("hidden");
        let sites_dir = s.0.join("Sites");
        for d in ["alpha", "beta", "My App.v2"] { std::fs::create_dir_all(sites_dir.join(d)).unwrap(); }
        let mut cfg = Config::default();
        cfg.parked = vec![sites_dir.display().to_string()];
        let names = |c: &Config| sites::all(c).into_iter().map(|s| s.name).collect::<Vec<_>>();
        assert_eq!(names(&cfg), vec!["alpha", "beta", "my-app.v2"]);
        cfg.hidden_sites.push(sites_dir.join("beta").display().to_string());
        assert_eq!(names(&cfg), vec!["alpha", "my-app.v2"]);
        // Paths compare case-insensitively and without a trailing separator on Windows.
        if cfg!(windows) {
            cfg.hidden_sites = vec![format!("{}\\", sites_dir.join("ALPHA").display())];
            assert_eq!(names(&cfg), vec!["beta", "my-app.v2"]);
        }
        let list = hidden_list(&cfg);
        assert_eq!(list["hidden"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn in_use_check_renames_back() {
        let s = Scratch::new("inuse");
        let d = s.0.join("site");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.txt"), b"x").unwrap();
        assert!(in_use_check(&d).is_ok());
        assert!(d.join("a.txt").exists(), "renamed back");
    }

    #[test]
    fn database_locality() {
        assert!(is_local_host("127.0.0.1") && is_local_host("localhost") && is_local_host("[::1]") && is_local_host(""));
        assert!(!is_local_host("db.example.com") && !is_local_host("10.0.0.5") && !is_local_host("pgsql"));
        let s = Scratch::new("db");
        let d = s.0.join("shop");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(".env"), "DB_CONNECTION=pgsql\nDB_HOST=db.example.com\nDB_DATABASE=shop\n").unwrap();
        let cfg = Config::default();
        let db = site_db(&cfg, &site("shop", &d, false)).unwrap();
        assert!(db.key.is_none(), "a remote database is never tez's");
        std::fs::write(d.join(".env"), "DB_CONNECTION=sqlite\n").unwrap();
        assert!(site_db(&cfg, &site("shop", &d, false)).unwrap().key.is_none());
        std::fs::remove_file(d.join(".env")).unwrap();
        assert!(site_db(&cfg, &site("shop", &d, false)).is_none());
        assert!(drop_db(&cfg, "postgres", "template1").is_err());
        assert_eq!(quote_pg("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(quote_my("we`ird"), "`we``ird`");
    }

    #[test]
    fn folder_size_is_counted() {
        let s = Scratch::new("size");
        std::fs::create_dir_all(s.0.join("a").join("b")).unwrap();
        std::fs::write(s.0.join("a").join("one"), vec![0u8; 1000]).unwrap();
        std::fs::write(s.0.join("a").join("b").join("two"), vec![0u8; 24]).unwrap();
        assert_eq!(dir_size(&s.0, None), (1024, false));
    }

    #[cfg(windows)]
    #[test]
    fn reads_recycle_bin_settings() {
        let out = "\r\nHKEY_CURRENT_USER\\...\\Volume\\{abc}\r\n    MaxCapacity    REG_DWORD    0x3e8\r\n\r\n";
        assert_eq!(sys::parse_reg_dword(out, "MaxCapacity"), Some(1000));
        assert_eq!(sys::parse_reg_dword(out, "NukeOnDelete"), None);
    }

    #[cfg(unix)]
    #[test]
    fn trashinfo_paths_are_escaped() {
        assert_eq!(sys::trash_url_path("/home/me/Sites/My App"), "/home/me/Sites/My%20App");
        assert_eq!(sys::trash_url_path("/a/ü"), "/a/%C3%BC");
    }

    /// Moves a scratch folder to the real Recycle Bin / Trash. Not run by default (it leaves an
    /// item called tez-trash-test-<pid> there): cargo test -p tez trash_for_real -- --ignored
    #[test]
    #[ignore]
    fn trash_for_real() {
        let s = Scratch::new("trash");
        let d = s.0.join(format!("tez-trash-test-{}", std::process::id()));
        std::fs::create_dir_all(d.join("inner")).unwrap();
        std::fs::write(d.join("inner").join("f.txt"), b"tez trash test").unwrap();
        assert_eq!(sys::trash_check(&d, 14), Ok(trash_word()));
        assert_eq!(sys::move_to_trash(&d).unwrap(), trash_word());
        assert!(!d.exists());
    }
}
