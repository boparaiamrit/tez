//! Sites: every folder inside a parked path, plus linked folders. Each is served at
//! `<name>.<tld>` and `<name>.localhost` (browsers resolve *.localhost with no hosts entry).

use crate::config::{Config, SiteSettings};
use crate::paths;
use crate::php;
use crate::util;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Site {
    pub name: String,
    pub path: PathBuf,
    /// The web root: `public/` when the project has one (Laravel), else the folder itself.
    pub root: PathBuf,
    pub php: String,
    pub linked: bool,
    pub laravel: bool,
    pub settings: SiteSettings,
}

impl Site {
    pub fn host(&self, tld: &str) -> String { format!("{}.{tld}", self.name) }
    pub fn url(&self, cfg: &Config) -> String {
        let port = if cfg.https_port == 443 { String::new() } else { format!(":{}", cfg.https_port) };
        format!("https://{}{port}", self.host(&cfg.tld))
    }
}

/// PHP version for a project folder: tez setting, then `.php-version`, then composer.json, then default.
pub fn php_for(cfg: &Config, name: &str, path: &Path) -> String {
    if let Some(v) = cfg.sites.get(name).and_then(|s| s.php.clone()) { return v; }
    if let Ok(v) = std::fs::read_to_string(path.join(".php-version")) {
        if let Ok(m) = php::normalize(v.trim()) { return m; }
    }
    if let Some(v) = project_manifest_php(path) { return v; }
    if let Some(v) = php::from_composer(path) {
        // composer.json allows the default? Prefer the default then, for predictability.
        if let Some(c) = composer_php_constraint(path) {
            if php::is_installed(&cfg.default_php) && php::satisfies(&cfg.default_php, &c) { return cfg.default_php.clone(); }
        }
        return v;
    }
    cfg.default_php.clone()
}

fn composer_php_constraint(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path.join("composer.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json["require"]["php"].as_str().map(|s| s.to_string())
}

fn project_manifest_php(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path.join("tez.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json["php"].as_str().and_then(|v| php::normalize(v).ok())
}

fn make(cfg: &Config, name: String, path: PathBuf, linked: bool) -> Site {
    let public = path.join("public");
    let root = if public.is_dir() { public } else { path.clone() };
    let laravel = path.join("artisan").exists();
    let php = php_for(cfg, &name, &path);
    let settings = cfg.sites.get(&name).cloned().unwrap_or_default();
    Site { name, path, root, php, linked, laravel, settings }
}

pub fn all(cfg: &Config) -> Vec<Site> {
    let mut out: Vec<Site> = Vec::new();
    for (name, path) in &cfg.links {
        out.push(make(cfg, name.clone(), PathBuf::from(path), true));
    }
    for parked in &cfg.parked {
        let Ok(rd) = std::fs::read_dir(parked) else { continue };
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for dir in dirs {
            let raw = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if raw.starts_with('.') || raw.starts_with('_') { continue; }
            if cfg.hidden_sites.iter().any(|h| same_path(Path::new(h), &dir)) { continue; }
            let name = util::host_label(&raw);
            if name.is_empty() || out.iter().any(|s| s.name == name) { continue; }
            out.push(make(cfg, name, dir, false));
        }
    }
    // `tez.<tld>` is the dashboard.
    out.retain(|s| !RESERVED.contains(&s.name.as_str()));
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub const RESERVED: &[&str] = &["tez"];

/// A path for comparing: forward slashes, no `\\?\` prefix or trailing separator, and
/// lower case on Windows, where paths are case-insensitive.
fn norm(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").unwrap_or(&s).trim_end_matches('/').to_string();
    if cfg!(windows) { s.to_ascii_lowercase() } else { s }
}

/// Two paths name the same folder.
pub fn same_path(a: &Path, b: &Path) -> bool { norm(a) == norm(b) }

/// `inner` is `outer` or somewhere below it. Everything is within the root `/` (which `norm`
/// turns into an empty string).
pub fn is_within(inner: &Path, outer: &Path) -> bool {
    let (i, o) = (norm(inner), norm(outer));
    i == o || o.is_empty() || i.starts_with(&format!("{o}/"))
}

pub fn find(cfg: &Config, name: &str) -> Option<Site> { all(cfg).into_iter().find(|s| s.name == name) }

/// The site a directory belongs to (the directory or any parent of it).
pub fn for_dir(cfg: &Config, dir: &Path) -> Option<Site> {
    let dir = paths::absolute(dir);
    let d = dir.to_string_lossy().to_ascii_lowercase();
    all(cfg).into_iter().filter(|s| {
        if !cfg!(windows) { return dir.starts_with(paths::absolute(&s.path)); }
        let p = paths::absolute(&s.path).to_string_lossy().to_ascii_lowercase();
        d == p || d.starts_with(&format!("{p}\\"))
    }).max_by_key(|s| s.path.as_os_str().len())
}

/// PHP versions any site needs, plus the default.
pub fn needed_php(cfg: &Config) -> Vec<String> {
    let mut v: Vec<String> = all(cfg).into_iter().map(|s| s.php).collect();
    v.push(cfg.default_php.clone());
    v.sort_by(|a, b| util::cmp_version(a, b));
    v.dedup();
    v.into_iter().filter(|m| php::is_installed(m)).collect()
}

/// Resolve which PHP a command run from `dir` should use (the php.exe shim calls this).
pub fn php_for_dir(cfg: &Config, dir: &Path) -> String {
    if let Some(site) = for_dir(cfg, dir) { return site.php; }
    // Not a site: walk up looking for .php-version, tez.json or composer.json.
    let mut cur = Some(paths::absolute(dir));
    while let Some(d) = cur {
        if d.join(".php-version").exists() || d.join("tez.json").exists() || d.join("composer.json").exists() {
            let v = php_for(cfg, "", &d);
            if php::is_installed(&v) { return v; }
            break;
        }
        cur = d.parent().map(|p| p.to_path_buf());
    }
    cfg.default_php.clone()
}
