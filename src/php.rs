//! PHP versions: install, update, extensions and php.ini.
//!
//! On Windows, builds come from windows.php.net (the official Visual Studio builds,
//! non-thread-safe, x64), with SHA-256 checked. PECL extensions (redis, mongodb, xdebug, and any
//! other one asked for) come from downloads.php.net/~windows/pecl, matched to each PHP version.
//!
//! On macOS and Linux, builds are static-php-cli's prebuilt "bulk" set (dl.static-php.dev): one
//! self-contained `php` and `php-fpm` per version with ~60 extensions compiled in (opcache,
//! intl, redis, pcntl, posix, pdo_pgsql, pdo_mysql, pdo_sqlite, gd, imagick, swoole...). Static
//! builds cannot load extensions, so there is nothing to switch on or add per version.

use crate::config::Config;
use crate::fetch;
use crate::paths;
use crate::sys::{self, Hidden};
use crate::util::{self, R};
use crate::bail;
use std::path::{Path, PathBuf};

const RELEASES: &str = "https://windows.php.net/downloads/releases/releases.json";
const BASE: &str = "https://windows.php.net/downloads/releases/";
const ARCHIVES: &str = "https://windows.php.net/downloads/releases/archives/";
const PECL: &str = "https://downloads.php.net/~windows/pecl/releases/";
const STATIC: &str = "https://dl.static-php.dev/static-php-cli/bulk/";

/// Versions tez offers, oldest first (Herd's 7.4 to 8.5, plus 8.6).
pub const MINORS: &[&str] = &["7.4", "8.0", "8.1", "8.2", "8.3", "8.4", "8.5", "8.6"];

/// Versions static-php-cli publishes for macOS and Linux (8.6 is served by FrankenPHP's own PHP).
const STATIC_MINORS: &[&str] = &["8.0", "8.1", "8.2", "8.3", "8.4", "8.5"];

/// Versions this OS has builds for.
pub fn available_minors() -> Vec<&'static str> {
    if cfg!(windows) { MINORS.to_vec() } else { STATIC_MINORS.to_vec() }
}

/// Static builds carry their extensions inside; only Windows builds load DLLs.
pub const LOADABLE_EXTENSIONS: bool = cfg!(windows);

/// Extensions bundled with PHP that tez enables, in load order (exif after mbstring).
const BUNDLED: &[&str] = &[
    "bz2", "curl", "fileinfo", "ftp", "gd", "gettext", "gmp", "intl", "mbstring", "exif", "mysqli",
    "openssl", "pdo_mysql", "pdo_pgsql", "pdo_sqlite", "pgsql", "shmop", "soap", "sockets", "sodium",
    "sqlite3", "xsl", "zip", "ffi",
];

/// PECL extensions installed with every version, like Herd (xdebug is installed but off by default).
pub const PECL_DEFAULT: &[&str] = &["redis", "mongodb", "xdebug"];

pub struct Release { pub minor: String, pub version: String, pub zip: String, pub sha256: Option<String> }

pub fn releases() -> R<Vec<Release>> {
    if cfg!(windows) { windows_releases() } else { static_releases() }
}

/// static-php-cli's names for this machine: ("macos" | "linux", "x86_64" | "aarch64").
fn static_target() -> (&'static str, &'static str) {
    (if cfg!(target_os = "macos") { "macos" } else { "linux" }, if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" })
}

/// The newest patch of each minor that has both a CLI and an FPM build for this machine.
fn static_releases() -> R<Vec<Release>> {
    let body = fetch::get_text(&format!("{STATIC}?format=json"))?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    let (os, arch) = static_target();
    let names: Vec<String> = json.as_array().into_iter().flatten().filter_map(|e| e["name"].as_str().map(|s| s.to_string())).collect();
    Ok(static_pick(&names, os, arch))
}

fn static_pick(names: &[String], os: &str, arch: &str) -> Vec<Release> {
    let suffix = format!("-{os}-{arch}.tar.gz");
    let mut out: Vec<Release> = Vec::new();
    for n in names {
        let Some(version) = n.strip_prefix("php-").and_then(|r| r.strip_suffix(&suffix)).and_then(|r| r.strip_suffix("-cli")) else { continue };
        if !names.contains(&format!("php-{version}-fpm{suffix}")) { continue; }
        let minor = version.split('.').take(2).collect::<Vec<_>>().join(".");
        if !STATIC_MINORS.contains(&minor.as_str()) { continue; }
        match out.iter_mut().find(|r| r.minor == minor) {
            Some(r) if util::cmp_version(version, &r.version) == std::cmp::Ordering::Greater => { r.version = version.to_string(); r.zip = n.clone(); }
            Some(_) => {}
            None => out.push(Release { minor, version: version.to_string(), zip: n.clone(), sha256: None }),
        }
    }
    out.sort_by(|a, b| util::cmp_version(&a.minor, &b.minor));
    out
}

fn windows_releases() -> R<Vec<Release>> {
    let body = fetch::get_text(RELEASES)?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    let mut out = Vec::new();
    if let Some(map) = json.as_object() {
        for (minor, v) in map {
            let version = v["version"].as_str().unwrap_or_default().to_string();
            let Some(obj) = v.as_object() else { continue };
            let build = obj.iter().find(|(k, _)| k.starts_with("nts-") && k.ends_with("-x64"));
            if let Some((_, b)) = build {
                out.push(Release {
                    minor: minor.clone(),
                    version,
                    zip: b["zip"]["path"].as_str().unwrap_or_default().to_string(),
                    sha256: b["zip"]["sha256"].as_str().map(|s| s.to_string()),
                });
            }
        }
    }
    out.sort_by(|a, b| util::cmp_version(&a.minor, &b.minor));
    Ok(out)
}

pub fn installed() -> Vec<(String, String)> {
    let mut v = Vec::new();
    for minor in MINORS {
        let dir = paths::php_dir(minor);
        if php_exe(minor).exists() {
            let ver = std::fs::read_to_string(dir.join(".tez-version")).unwrap_or_default().trim().to_string();
            v.push((minor.to_string(), ver));
        }
    }
    v
}

pub fn is_installed(minor: &str) -> bool { php_exe(minor).exists() }
pub fn php_exe(minor: &str) -> PathBuf { paths::php_dir(minor).join(sys::exe("php")) }
/// The FastCGI server for web requests: php-cgi on Windows, php-fpm on macOS and Linux.
pub fn php_cgi_exe(minor: &str) -> PathBuf { paths::php_dir(minor).join(if cfg!(windows) { "php-cgi.exe" } else { "php-fpm" }) }

/// Accept "8.4", "84", "php84", "8.4.12" and return "8.4".
pub fn normalize(v: &str) -> R<String> {
    let s = v.trim().trim_start_matches("php").trim_start_matches('@');
    let s = if !s.contains('.') && s.len() == 2 { format!("{}.{}", &s[..1], &s[1..]) } else { s.to_string() };
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() < 2 { bail!("'{v}' is not a PHP version (try 8.4)"); }
    let minor = format!("{}.{}", parts[0], parts[1]);
    if !MINORS.contains(&minor.as_str()) { bail!("PHP {minor} is not offered; choose one of {}", MINORS.join(", ")); }
    Ok(minor)
}

/// The Windows build zips windows.php.net lists (current releases and archives): file names.
fn windows_listings() -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    for base in [BASE, ARCHIVES] {
        if let Ok(html) = fetch::get_text(base) {
            for h in fetch::hrefs(&html) {
                let name = h.rsplit('/').next().unwrap_or(&h).to_string();
                if name.starts_with("php-") && name.ends_with(".zip") { out.push((name, base)); }
            }
        }
    }
    out
}

/// "php-8.3.27-nts-Win32-vs16-x64.zip" -> "8.3.27" (only plain x.y.z releases).
fn windows_zip_version(name: &str) -> Option<String> {
    let rest = name.strip_prefix("php-")?.strip_suffix("-x64.zip")?;
    let (ver, tail) = rest.split_once("-nts-Win32-")?;
    if tail.contains('-') || ver.split('.').count() != 3 || !ver.chars().all(|c| c.is_ascii_digit() || c == '.') { return None; }
    Some(ver.to_string())
}

/// Every patch of each PHP version this OS has builds for, newest first.
pub fn all_versions() -> R<Vec<(String, Vec<String>)>> {
    let mut map: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    if cfg!(windows) {
        for (name, _) in windows_listings() {
            if let Some(v) = windows_zip_version(&name) {
                let minor = v.split('.').take(2).collect::<Vec<_>>().join(".");
                map.entry(minor).or_default().push(v);
            }
        }
        // releases.json can list a release before the directory listing shows it.
        for r in windows_releases().unwrap_or_default() { map.entry(r.minor.clone()).or_default().push(r.version); }
    } else {
        let body = fetch::get_text(&format!("{STATIC}?format=json"))?;
        let json: serde_json::Value = serde_json::from_str(&body)?;
        let (os, arch) = static_target();
        let suffix = format!("-{os}-{arch}.tar.gz");
        let names: Vec<String> = json.as_array().into_iter().flatten().filter_map(|e| e["name"].as_str().map(|s| s.to_string())).collect();
        for n in &names {
            let Some(v) = n.strip_prefix("php-").and_then(|r| r.strip_suffix(&suffix)).and_then(|r| r.strip_suffix("-cli")) else { continue };
            if !names.contains(&format!("php-{v}-fpm{suffix}")) { continue; }
            map.entry(v.split('.').take(2).collect::<Vec<_>>().join(".")).or_default().push(v.to_string());
        }
    }
    let mut out: Vec<(String, Vec<String>)> = map.into_iter().filter(|(m, _)| MINORS.contains(&m.as_str())).map(|(m, mut v)| {
        v.sort_by(|a, b| util::cmp_version(b, a));
        v.dedup();
        (m, v)
    }).collect();
    out.sort_by(|a, b| util::cmp_version(&b.0, &a.0));
    Ok(out)
}

/// The build of one exact PHP version ("8.3.27") for this machine.
fn release_for(minor: &str, version: &str) -> R<Release> {
    if cfg!(windows) {
        let current = windows_releases().unwrap_or_default().into_iter().find(|r| r.version == version);
        if let Some(r) = current { return Ok(r); }
        let found = windows_listings().into_iter().find(|(n, _)| windows_zip_version(n).as_deref() == Some(version));
        let Some((zip, _)) = found else { bail!("windows.php.net has no x64 NTS build of PHP {version}") };
        return Ok(Release { minor: minor.to_string(), version: version.to_string(), zip, sha256: None });
    }
    let body = fetch::get_text(&format!("{STATIC}?format=json"))?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    let (os, arch) = static_target();
    let names: Vec<String> = json.as_array().into_iter().flatten().filter_map(|e| e["name"].as_str().map(|s| s.to_string())).collect();
    let cli = format!("php-{version}-cli-{os}-{arch}.tar.gz");
    if !names.contains(&cli) || !names.contains(&cli.replace("-cli-", "-fpm-")) { bail!("static-php.dev has no {os} {arch} build of PHP {version}"); }
    Ok(Release { minor: minor.to_string(), version: version.to_string(), zip: cli, sha256: None })
}

/// Install (or update to the newest patch of) one PHP version.
pub fn install(minor: &str, cfg: &Config) -> R<String> { install_version(minor, None, cfg) }

/// Install one PHP version: the newest patch, or exactly `version` ("8.3.27").
pub fn install_version(minor: &str, version: Option<&str>, cfg: &Config) -> R<String> {
    if !available_minors().contains(&minor) {
        bail!("PHP {minor} has no {} build; choose one of {}{}", sys::os(), available_minors().join(", "),
            if minor == "8.6" { " (sites on 8.6 run inside FrankenPHP's own PHP 8.6)" } else { "" });
    }
    let rel = match version {
        Some(v) => Some(release_for(minor, v)?),
        None => releases()?.into_iter().find(|r| r.minor == minor),
    };
    let Some(rel) = rel else { bail!("{} lists no build for PHP {minor}", if cfg!(windows) { "windows.php.net" } else { "static-php.dev" }) };
    let current = std::fs::read_to_string(paths::php_dir(minor).join(".tez-version")).unwrap_or_default();
    if current.trim() == rel.version && is_installed(minor) {
        util::ok(&format!("PHP {} is up to date", rel.version));
        write_ini(minor, cfg)?;
        return Ok(rel.version);
    }
    util::step(&format!("Downloading PHP {}", rel.version));
    let dir = paths::php_dir(minor);
    let fresh = dir.with_extension("new");
    let _ = std::fs::remove_dir_all(&fresh);
    if cfg!(windows) {
        let url = if fetch::exists(&format!("{BASE}{}", rel.zip)) { format!("{BASE}{}", rel.zip) } else { format!("{ARCHIVES}{}", rel.zip) };
        let zip = fetch::download(&url, &rel.zip)?;
        if let Some(sum) = &rel.sha256 { fetch::verify_sha256(&zip, sum)?; }
        fetch::extract(&zip, &fresh, &[])?;
    } else {
        // Two single-file archives: `php` (the CLI) and `php-fpm` (the web pool).
        for file in [rel.zip.clone(), rel.zip.replace("-cli-", "-fpm-")] {
            let tgz = fetch::download(&format!("{STATIC}{file}"), &file)?;
            fetch::extract(&tgz, &fresh, &[])?;
        }
        for exe in ["php", "php-fpm"] {
            let p = fresh.join(exe);
            if !p.exists() { bail!("the static PHP {} archive has no {exe}", rel.version); }
            fetch::make_executable(&p)?;
        }
    }
    // Extensions added to the old folder (PECL) are reinstalled below; keep nothing else.
    if dir.exists() {
        let old = dir.with_extension("old");
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(&dir, &old).map_err(|e| format!("PHP {minor} is in use; run `tez stop` first ({e})"))?;
        let _ = std::fs::remove_dir_all(&old);
    }
    std::fs::rename(&fresh, &dir)?;
    std::fs::write(dir.join(".tez-version"), &rel.version)?;

    if !LOADABLE_EXTENSIONS {
        let _ = std::fs::remove_file(dir.join(".tez-modules"));
        let mods = modules(&dir);
        write_ini(minor, cfg)?;
        util::ok(&format!("PHP {} installed ({} extensions built in)", rel.version, mods.len()));
        return Ok(rel.version);
    }
    for ext in PECL_DEFAULT {
        if let Err(e) = pecl_install(minor, ext, false) {
            util::warn(&format!("{ext} for PHP {minor}: {e}"));
        }
    }
    for ext in &cfg.extensions {
        if !dir.join("ext").join(format!("php_{ext}.dll")).exists() {
            if let Err(e) = pecl_install(minor, ext, false) { util::warn(&format!("{ext} for PHP {minor}: {e}")); }
        }
    }
    write_ini(minor, cfg)?;
    util::ok(&format!("PHP {} installed", rel.version));
    Ok(rel.version)
}

fn vc_tags() -> &'static [&'static str] { &["vs18", "vs17", "vs16", "vc15"] }

fn valid_ext_name(ext: &str) -> R<()> {
    if ext.is_empty() || !ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') { bail!("'{ext}' is not an extension name"); }
    Ok(())
}

/// Versions with a Windows build folder on downloads.php.net, newest first.
pub fn pecl_windows_versions(ext: &str) -> Vec<String> {
    let Ok(listing) = fetch::get_text_retry(&format!("{PECL}{ext}/"), 3) else { return Vec::new() };
    let mut v: Vec<String> = fetch::hrefs(&listing).into_iter()
        .filter_map(|h| h.strip_suffix('/').map(|s| s.to_string()))
        .filter(|s| s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))
        .collect();
    v.sort_by(|a, b| util::cmp_version(b, a));
    v.dedup();
    v
}

/// Every release pecl.php.net has published for an extension, newest first: (version, stability).
pub fn pecl_source_releases(ext: &str) -> Vec<(String, String)> {
    let Ok(xml) = fetch::get_text_retry(&format!("https://pecl.php.net/rest/r/{}/allreleases.xml", ext.to_ascii_lowercase()), 3) else { return Vec::new() };
    let mut out = Vec::new();
    let mut rest = xml.as_str();
    while let Some(i) = rest.find("<r>") {
        rest = &rest[i + 3..];
        let Some(end) = rest.find("</r>") else { break };
        let block = &rest[..end];
        let tag = |t: &str| block.find(&format!("<{t}>")).and_then(|a| block[a + t.len() + 2..].find(&format!("</{t}>")).map(|b| block[a + t.len() + 2..a + t.len() + 2 + b].to_string()));
        if let Some(v) = tag("v") { out.push((v, tag("s").unwrap_or_else(|| "stable".into()))); }
        rest = &rest[end..];
    }
    out
}

/// The Windows DLL zip of one extension version for one PHP version: Ok(None) when PECL has no
/// build of it, Err when the listing could not be read (never mistaken for "no build").
fn pecl_dll_file(ext: &str, ver: &str, minor: &str, flavor: &str) -> R<Option<String>> {
    let files = fetch::hrefs(&fetch::get_text_retry(&format!("{PECL}{ext}/{ver}/"), 3).map_err(|e| format!("could not read PECL's {ext} {ver} builds: {e}"))?);
    let want: Vec<String> = vc_tags().iter().map(|vc| format!("php_{ext}-{ver}-{minor}-{flavor}-{vc}-x64.zip")).collect();
    Ok(files.iter().map(|f| f.rsplit('/').next().unwrap_or(f).to_string()).find(|f| want.contains(f)))
}

/// Whether one extension version has a Windows DLL for a PHP version (NTS, x64); Err when PECL
/// could not be reached.
pub fn pecl_has_dll(ext: &str, ver: &str, minor: &str) -> R<bool> { Ok(pecl_dll_file(ext, ver, minor, "nts")?.is_some()) }

/// Download a PECL DLL zip and put php_<ext>.dll in ext/ and any helper DLLs beside php.exe.
fn place_dll_zip(php_dir: &Path, ext: &str, ver: &str, minor: &str, flavor: &str, file: &str) -> R<bool> {
    let zip = fetch::download(&format!("{PECL}{ext}/{ver}/{file}"), file)?;
    let tmp = paths::cache_dir().join(format!("pecl-{ext}-{minor}-{flavor}"));
    let _ = std::fs::remove_dir_all(&tmp);
    fetch::extract(&zip, &tmp, &[])?;
    let mut placed = false;
    for entry in walk(&tmp) {
        let name = entry.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if !name.to_ascii_lowercase().ends_with(".dll") { continue; }
        let dest = if name.starts_with("php_") { php_dir.join("ext").join(&name) } else { php_dir.join(&name) };
        std::fs::copy(&entry, &dest)?;
        placed |= name.starts_with("php_");
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(placed)
}

/// Which version of a PECL extension to install.
#[derive(Clone, Debug, PartialEq)]
pub enum PeclWant {
    /// The newest stable release that has a Windows DLL for this PHP (used when PHP is installed).
    Auto,
    /// The newest stable release; built from source when PECL has no DLL for it.
    Latest,
    /// Exactly this release; built from source when PECL has no DLL for it.
    Exact(String),
}

/// Install a PECL extension's DLL into a PHP folder (`ts` for FrankenPHP's thread-safe PHP).
pub fn pecl_install_into(php_dir: &Path, minor: &str, ext: &str, ts: bool) -> R<String> {
    pecl_install_want(php_dir, minor, ext, ts, &PeclWant::Auto, false)
}

/// Install a PECL extension into a PHP folder: the version `want` asks for, from PECL's Windows
/// DLLs, else compiled from the pecl.php.net source release (Windows; `force_source` always
/// compiles).
pub fn pecl_install_want(php_dir: &Path, minor: &str, ext: &str, ts: bool, want: &PeclWant, force_source: bool) -> R<String> {
    valid_ext_name(ext)?;
    if !LOADABLE_EXTENSIONS {
        bail!("{}", static_note(minor));
    }
    let flavor = if ts { "ts" } else { "nts" };
    match want {
        PeclWant::Exact(ver) => {
            if !force_source {
                if let Some(file) = pecl_dll_file(ext, ver, minor, flavor)? {
                    if place_dll_zip(php_dir, ext, ver, minor, flavor, &file)? { return Ok(ver.clone()); }
                }
                util::step(&format!("PECL has no Windows {flavor} build of {ext} {ver} for PHP {minor}; compiling it from source"));
            }
            build_from_source(php_dir, minor, ext, ver, ts)
        }
        PeclWant::Latest => {
            let source = pecl_source_releases(ext);
            let newest = source.iter().find(|(v, s)| s == "stable" && !util::is_prerelease(v)).or(source.first()).map(|(v, _)| v.clone())
                .or_else(|| pecl_windows_versions(ext).into_iter().find(|v| !util::is_prerelease(v)));
            let Some(ver) = newest else { bail!("PECL lists no release of '{ext}'") };
            if !force_source {
                if let Some(file) = pecl_dll_file(ext, &ver, minor, flavor)? {
                    if place_dll_zip(php_dir, ext, &ver, minor, flavor, &file)? { return Ok(ver); }
                }
                util::step(&format!("The newest {ext} ({ver}) has no Windows {flavor} build for PHP {minor}; compiling it from source"));
            }
            match build_from_source(php_dir, minor, ext, &ver, ts) {
                Ok(v) => Ok(v),
                Err(e) if !force_source => {
                    util::warn(&format!("compiling {ext} {ver} failed ({e}); installing the newest release PECL built for PHP {minor} instead"));
                    pecl_install_want(php_dir, minor, ext, ts, &PeclWant::Auto, false)
                }
                Err(e) => Err(e),
            }
        }
        PeclWant::Auto => {
            let versions = pecl_windows_versions(ext);
            if versions.is_empty() { bail!("PECL has no Windows builds of '{ext}'"); }
            let stable: Vec<String> = versions.iter().filter(|v| !util::is_prerelease(v)).cloned().collect();
            let candidates = if stable.is_empty() { versions } else { stable };
            // Old PHP versions (7.4, 8.0) need older extension releases, so look back far enough.
            for ver in candidates.iter().take(60) {
                if let Ok(Some(file)) = pecl_dll_file(ext, ver, minor, flavor) {
                    if place_dll_zip(php_dir, ext, ver, minor, flavor, &file)? { return Ok(ver.clone()); }
                }
            }
            bail!("no {flavor} build of '{ext}' for PHP {minor} on PECL")
        }
    }
}

pub fn pecl_install(minor: &str, ext: &str, ts: bool) -> R<String> {
    let v = pecl_install_into(&paths::php_dir(minor), minor, ext, ts)?;
    util::ok(&format!("{ext} {v} for PHP {minor}"));
    Ok(v)
}

/// What macOS and Linux can do instead of adding a PECL extension: the static builds already
/// carry their extensions.
pub fn static_note(minor: &str) -> String {
    let mods = modules(&paths::php_dir(minor));
    format!("PHP on {} is a static build: its extensions are compiled in, so one cannot be added or swapped per version. PHP {minor} already has: {}",
        sys::os(), if mods.is_empty() { "(install PHP first)".to_string() } else { mods.join(", ") })
}

/// The installed version of an extension in one PHP folder (from its DLL), if any.
pub fn pecl_installed_version(minor: &str, ext: &str) -> Option<String> {
    let dir = paths::php_dir(minor);
    let php = php_exe(minor);
    if !php.exists() { return None; }
    let mut cmd = std::process::Command::new(&php);
    cmd.hidden();
    if LOADABLE_EXTENSIONS {
        if !has_ext(&dir, ext) { return None; }
        let directive = if matches!(ext, "xdebug" | "opcache") { "zend_extension" } else { "extension" };
        cmd.args(["-n", "-d", &format!("extension_dir={}", dir.join("ext").display()), "-d", &format!("{directive}={ext}"), "-d", "display_startup_errors=0"]);
    }
    let out = cmd.args(["-r", &format!("echo PHP_EOL, phpversion('{ext}') ?: '';")]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let v = text.lines().last().unwrap_or("").trim().to_string();
    (!v.is_empty() && !v.contains(' ')).then_some(v)
}

/// The installation folder of Visual Studio (Build Tools) with the x64 C++ compiler, via vswhere.
fn visual_studio() -> Option<PathBuf> {
    let vswhere = PathBuf::from(std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into())).join(r"Microsoft Visual Studio\Installer\vswhere.exe");
    if !vswhere.exists() { return None; }
    let out = std::process::Command::new(vswhere).hidden()
        .args(["-products", "*", "-latest", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"])
        .output().ok()?;
    let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    p.join(r"VC\Auxiliary\Build\vcvarsall.bat").exists().then_some(p)
}

/// php-sdk-binary-tools (bison, re2c and the rest), unpacked once into the cache.
fn php_sdk() -> R<PathBuf> {
    let tag = fetch::github_latest_tag("php/php-sdk-binary-tools").unwrap_or_else(|_| "php-sdk-2.9.0".into());
    let root = paths::cache_dir().join("php-sdk").join(&tag);
    let marker = root.join(".tez-ready");
    if marker.exists() {
        if let Some(d) = std::fs::read_dir(&root).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.join("msys2").exists())) { return Ok(d); }
    }
    util::step(&format!("Downloading the PHP SDK tools ({tag})"));
    let zip = fetch::download(&format!("https://github.com/php/php-sdk-binary-tools/archive/refs/tags/{tag}.zip"), &format!("php-sdk-binary-tools-{tag}.zip"))?;
    let _ = std::fs::remove_dir_all(&root);
    fetch::extract(&zip, &root, &[])?;
    std::fs::write(&marker, &tag)?;
    std::fs::read_dir(&root)?.flatten().map(|e| e.path()).find(|p| p.join("msys2").exists()).ok_or_else(|| "the PHP SDK archive has an unexpected layout".into())
}

/// The PHP development pack matching an installed PHP (headers, php8.lib, phpize).
fn devel_pack(full: &str, ts: bool) -> R<PathBuf> {
    let flavor = if ts { "" } else { "nts-" };
    let found = windows_listings().into_iter().find(|(n, _)| n.starts_with(&format!("php-devel-pack-{full}-{flavor}Win32-")) && n.ends_with("-x64.zip"));
    let Some((zip, base)) = found else { bail!("windows.php.net has no development pack for PHP {full}") };
    util::step(&format!("Downloading {zip}"));
    let file = fetch::download(&format!("{base}{zip}"), &zip)?;
    let dest = paths::cache_dir().join("php-devel").join(zip.trim_end_matches(".zip"));
    if !dest.exists() { fetch::extract(&file, &dest, &[])?; }
    std::fs::read_dir(&dest)?.flatten().map(|e| e.path()).find(|p| p.join("phpize.bat").exists()).ok_or_else(|| "the development pack has no phpize.bat".into())
}

/// The configure switch an extension's config.w32 declares: "--enable-igbinary", "--with-redis".
fn configure_switch(src: &Path, ext: &str) -> String {
    let text = std::fs::read_to_string(src.join("config.w32")).unwrap_or_default();
    let mut first: Option<String> = None;
    for kind in ["ARG_ENABLE", "ARG_WITH"] {
        let mut rest = text.as_str();
        while let Some(i) = rest.find(&format!("{kind}(")) {
            rest = &rest[i + kind.len() + 1..];
            let name: String = rest.trim_start().trim_start_matches(['"', '\'']).chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
            let flag = format!("--{}-{name}", if kind == "ARG_ENABLE" { "enable" } else { "with" });
            if name.eq_ignore_ascii_case(ext) || name.replace('-', "_").eq_ignore_ascii_case(&ext.replace('-', "_")) { return flag; }
            if first.is_none() && !name.is_empty() { first = Some(flag); }
        }
    }
    first.unwrap_or_else(|| format!("--enable-{ext}"))
}

/// Compile a PECL extension from its pecl.php.net source release with Visual Studio, the PHP SDK
/// tools and the matching development pack, and put the DLL in ext/. The build output goes to
/// stdout, so a dashboard job shows all of it.
pub fn build_from_source(php_dir: &Path, minor: &str, ext: &str, ver: &str, ts: bool) -> R<String> {
    valid_ext_name(ext)?;
    if !cfg!(windows) { bail!("{}", static_note(minor)); }
    if ts { bail!("source builds are for the NTS PHP that serves sites; FrankenPHP's thread-safe PHP takes PECL's DLLs only"); }
    let full = std::fs::read_to_string(php_dir.join(".tez-version")).map(|s| s.trim().to_string()).map_err(|_| format!("PHP {minor} is not installed"))?;
    let vs = visual_studio().ok_or("compiling an extension needs Visual Studio 2022 Build Tools with the C++ workload (winget install Microsoft.VisualStudio.2022.BuildTools --override \"--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive\")")?;
    fetch::ensure_space(&paths::cache_dir(), 400 << 20)?;
    let sdk = php_sdk()?;
    let devel = devel_pack(&full, ts)?;
    util::step(&format!("Downloading the {ext} {ver} source from pecl.php.net"));
    let tgz = fetch::download(&format!("https://pecl.php.net/get/{ext}-{ver}.tgz"), &format!("{ext}-{ver}.tgz"))
        .map_err(|_| format!("pecl.php.net has no source release {ext} {ver}"))?;
    let build = paths::cache_dir().join("pecl-build").join(format!("{ext}-{ver}-{minor}"));
    let _ = std::fs::remove_dir_all(&build);
    fetch::extract(&tgz, &build, &[])?;
    let src = walk(&build).into_iter().find(|p| p.file_name().map(|n| n == "config.w32").unwrap_or(false)).and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .ok_or_else(|| format!("{ext} {ver} has no config.w32, so it does not build on Windows"))?;
    let switch = configure_switch(&src, ext);
    let bat = format!(
        "@echo off\r\nset \"VSCMD_START_DIR={src}\"\r\ncall \"{vc}\" x64 >nul 2>nul || (echo tez: could not set up Visual Studio & exit /b 1)\r\nset \"PATH={sdk}\\bin;{sdk}\\msys2\\usr\\bin;%PATH%\"\r\ncd /d \"{src}\"\r\necho tez: phpize\r\ncall \"{devel}\\phpize.bat\" || exit /b 2\r\necho tez: configure {switch}\r\ncall \"{src}\\configure.bat\" {switch} || exit /b 3\r\necho tez: nmake\r\nnmake || exit /b 4\r\n",
        src = src.display(), vc = vs.join(r"VC\Auxiliary\Build\vcvarsall.bat").display(), sdk = sdk.display(), devel = devel.display(),
    );
    let script = build.join("tez-build.bat");
    std::fs::write(&script, bat)?;
    util::step(&format!("Compiling {ext} {ver} for PHP {full} ({switch}) with {}", vs.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
    // No console window: the output is piped through to tez's own output (the job log).
    let mut child = std::process::Command::new(paths::system32("cmd.exe")).arg("/c").arg(&script).current_dir(&src)
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).hidden().spawn()?;
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let forward = |r: Option<Box<dyn std::io::Read + Send>>| std::thread::spawn(move || {
        use std::io::BufRead;
        if let Some(r) = r { for line in std::io::BufReader::new(r).lines().map_while(Result::ok) { println!("{line}"); } }
    });
    let t1 = forward(out.map(|o| Box::new(o) as Box<dyn std::io::Read + Send>));
    let t2 = forward(err.map(|e| Box::new(e) as Box<dyn std::io::Read + Send>));
    let st = child.wait()?;
    let _ = (t1.join(), t2.join());
    if !st.success() { bail!("compiling {ext} {ver} failed at step {} (1 Visual Studio, 2 phpize, 3 configure, 4 nmake); the output above has the reason", st.code().unwrap_or(-1)); }
    let dll = walk(&src.join("x64")).into_iter().find(|p| {
        let n = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        n.starts_with("php_") && n.ends_with(".dll")
    }).ok_or("the build finished but made no php_*.dll")?;
    let name = dll.file_name().unwrap().to_string_lossy().to_string();
    std::fs::copy(&dll, php_dir.join("ext").join(&name))?;
    let _ = std::fs::remove_dir_all(&build);
    util::ok(&format!("compiled {name} ({ext} {ver}) for PHP {full}"));
    Ok(ver.to_string())
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { out.extend(walk(&p)); } else { out.push(p); }
        }
    }
    out
}

fn has_ext(php_dir: &Path, name: &str) -> bool { php_dir.join("ext").join(format!("php_{name}.dll")).exists() }

/// The php.ini settings tez manages and shows in the dashboard: key, default, what it does.
pub const INI: &[(&str, &str, &str)] = &[
    ("memory_limit", "2048M", "Most memory one request or command may use"),
    ("max_execution_time", "30", "Seconds a web request may run (CLI has no limit)"),
    ("max_input_time", "60", "Seconds to read request data"),
    ("max_input_vars", "5000", "Form fields accepted per request"),
    ("post_max_size", "256M", "Largest request body"),
    ("upload_max_filesize", "256M", "Largest uploaded file"),
    ("max_file_uploads", "50", "Files per request"),
    ("display_errors", "On", "Show errors in the page"),
    ("display_startup_errors", "On", "Show errors raised while PHP starts"),
    ("error_reporting", "E_ALL", "Which errors are reported"),
    ("log_errors", "On", "Write errors to the log"),
    ("date.timezone", "UTC", "Default time zone"),
    ("default_socket_timeout", "60", "Seconds before socket reads give up"),
    ("output_buffering", "4096", "Output buffer size"),
    ("short_open_tag", "Off", "Allow <? tags"),
    ("allow_url_fopen", "On", "Open URLs like files"),
    ("realpath_cache_size", "16M", "Path cache size; large avoids slow Windows file lookups"),
    ("realpath_cache_ttl", "600", "Seconds paths stay cached"),
    ("session.gc_maxlifetime", "1440", "Seconds a PHP session lives"),
    ("opcache.enable", "1", "Cache compiled code for web requests"),
    ("opcache.enable_cli", "1", "Cache compiled code for artisan, Horizon and queues (~4x faster, ~85% less memory)"),
    ("opcache.memory_consumption", "512", "MB of compiled-code cache"),
    ("opcache.interned_strings_buffer", "64", "MB for shared strings"),
    ("opcache.max_accelerated_files", "100000", "How many files can be cached"),
    ("opcache.validate_timestamps", "1", "Notice edited files (keep 1 when developing)"),
    ("opcache.revalidate_freq", "1", "Seconds between checks for edited files"),
    ("opcache.jit", "disable", "JIT compiler (tracing, function, disable)"),
    ("opcache.jit_buffer_size", "0", "Memory for JIT code, e.g. 64M"),
    ("xdebug.mode", "debug,develop", "Xdebug features when Xdebug is on"),
    ("xdebug.start_with_request", "trigger", "trigger: only with XDEBUG_TRIGGER; yes: always"),
    ("xdebug.client_port", "9003", "Port your IDE listens on"),
];

/// Settings for one version: tez defaults, then all-version settings, then this version's.
pub fn ini_values(minor: &str, cfg: &Config) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = INI.iter().map(|(k, d, _)| (k.to_string(), d.to_string())).collect();
    let per = cfg.php_ini_for.get(minor);
    for (k, val) in cfg.php_ini.iter().chain(per.into_iter().flatten()) {
        match v.iter_mut().find(|(kk, _)| kk == k) {
            Some(e) => e.1 = val.clone(),
            None => v.push((k.clone(), val.clone())),
        }
    }
    v
}

/// The extensions compiled into a static PHP (`php -n -m`), cached beside it in `.tez-modules`.
pub fn modules(php_dir: &Path) -> Vec<String> {
    let cache = php_dir.join(".tez-modules");
    if let Ok(t) = std::fs::read_to_string(&cache) {
        let v: Vec<String> = t.lines().map(|s| s.to_string()).filter(|s| !s.is_empty()).collect();
        if !v.is_empty() { return v; }
    }
    let exe = php_dir.join(sys::exe("php"));
    let Ok(out) = std::process::Command::new(&exe).args(["-n", "-m"]).hidden().output() else { return Vec::new() };
    let mut v: Vec<String> = String::from_utf8_lossy(&out.stdout).lines()
        .map(|l| l.trim().to_ascii_lowercase().replace(' ', "_"))
        .filter(|l| !l.is_empty() && !l.starts_with('['))
        .collect();
    v.sort();
    v.dedup();
    if !v.is_empty() { let _ = std::fs::write(&cache, v.join("\n")); }
    v
}

/// Every extension in a PHP folder: (name, loaded, on by default). On Windows these are the DLLs
/// in `ext/`; on macOS and Linux, the ones compiled into the static build (always on).
pub fn extensions_state(php_dir: &Path, minor: &str, cfg: &Config) -> Vec<(String, bool, bool)> {
    if !LOADABLE_EXTENSIONS {
        return modules(php_dir).into_iter().map(|n| (n, true, true)).collect();
    }
    let mut names: Vec<String> = std::fs::read_dir(php_dir.join("ext")).map(|rd| rd.flatten()
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .filter_map(|n| n.strip_prefix("php_").and_then(|x| x.strip_suffix(".dll")).map(|x| x.to_string()))
        .collect()).unwrap_or_default();
    names.sort();
    let on = cfg.php_ext_on.get(minor).cloned().unwrap_or_default();
    let off = cfg.php_ext_off.get(minor).cloned().unwrap_or_default();
    names.into_iter().filter(|n| !["zend_test", "dl_test"].contains(&n.as_str())).map(|n| {
        let default = BUNDLED.contains(&n.as_str()) || n == "gd2" || ["redis", "mongodb", "igbinary", "msgpack", "opcache"].contains(&n.as_str());
        let enabled = if n == "xdebug" { cfg.xdebug && !off.contains(&n) } else if default { !off.contains(&n) } else { on.contains(&n) || cfg.extensions.contains(&n) };
        (n, enabled, default)
    }).collect()
}

/// Write `php.ini` for one PHP folder. User additions in `config/php/<minor>.ini` are appended.
pub fn ini_text(php_dir: &Path, minor: &str, cfg: &Config) -> String {
    let ext_dir = paths::fwd(&php_dir.join("ext"));
    let prepend = paths::fwd(&paths::lib_php().join("prepend.php"));
    let cacert = paths::fwd(&paths::cacert());
    let opcache_dir = paths::cache_dir().join("opcache").join(minor);
    let _ = std::fs::create_dir_all(&opcache_dir);
    let values = ini_values(minor, cfg);
    let mut s = String::new();
    s.push_str("; Written by tez. Change settings in the dashboard (PHP tab), or add your own in\n");
    s.push_str(&format!("; {}\n\n", paths::config_dir().join("php").join(format!("{minor}.ini")).display()));
    let ext_line = if LOADABLE_EXTENSIONS { format!("extension_dir = \"{ext_dir}\"\n") } else { String::new() };
    s.push_str(&format!(
        "[PHP]\nengine = On\nprecision = 14\nserialize_precision = -1\nzend.enable_gc = On\nexpose_php = Off\nvariables_order = \"EGPCS\"\nrequest_order = \"GP\"\nauto_globals_jit = On\ndefault_charset = \"UTF-8\"\nfile_uploads = On\n{ext_line}auto_prepend_file = \"{prepend}\"\ncurl.cainfo = \"{cacert}\"\nopenssl.cafile = \"{cacert}\"\nffi.enable = preload\n"
    ));
    for (k, v) in values.iter().filter(|(k, _)| !k.starts_with("opcache.") && !k.starts_with("xdebug.")) {
        s.push_str(&format!("{k} = {v}\n"));
    }
    s.push('\n');
    if !LOADABLE_EXTENSIONS {
        // Static builds: extensions (opcache included) are compiled in; only their settings go here.
        s.push_str("[opcache]\n");
        for (k, v) in values.iter().filter(|(k, _)| k.starts_with("opcache.")) {
            s.push_str(&format!("{k} = {v}\n"));
        }
        s.push_str(&format!("opcache.save_comments = 1\nopcache.file_cache = \"{}\"\n", paths::fwd(&opcache_dir)));
        append_user_ini(&mut s, minor);
        return s;
    }
    let state = extensions_state(php_dir, minor, cfg);
    let is_on = |n: &str| state.iter().any(|(x, on, _)| x == n && *on);
    // Load order matters: mbstring before exif, igbinary/msgpack before redis.
    let mut order: Vec<String> = BUNDLED.iter().map(|s| s.to_string()).collect();
    order.extend(["gd2", "igbinary", "msgpack", "redis", "mongodb"].iter().map(|s| s.to_string()));
    for (n, _, _) in &state { if !order.contains(n) { order.push(n.clone()); } }
    for n in &order {
        if n == "opcache" || n == "xdebug" { continue; }
        if n == "gd2" && has_ext(php_dir, "gd") { continue; }
        let name = if n == "gd" && !has_ext(php_dir, "gd") { "gd2" } else { n.as_str() };
        if has_ext(php_dir, name) && is_on(name) { s.push_str(&format!("extension={name}\n")); }
    }
    if has_ext(php_dir, "opcache") && is_on("opcache") { s.push_str("zend_extension=opcache\n"); }
    s.push_str("\n[opcache]\n");
    for (k, v) in values.iter().filter(|(k, _)| k.starts_with("opcache.")) {
        s.push_str(&format!("{k} = {v}\n"));
    }
    s.push_str(&format!("opcache.save_comments = 1\nopcache.file_cache = \"{}\"\n", paths::fwd(&opcache_dir)));
    if is_on("xdebug") && has_ext(php_dir, "xdebug") {
        s.push_str("\n[xdebug]\nzend_extension = xdebug\nxdebug.client_host = 127.0.0.1\n");
        for (k, v) in values.iter().filter(|(k, _)| k.starts_with("xdebug.")) {
            s.push_str(&format!("{k} = {v}\n"));
        }
    }
    append_user_ini(&mut s, minor);
    s
}

/// User additions from `config/php/<minor>.ini`.
fn append_user_ini(s: &mut String, minor: &str) {
    let user = paths::config_dir().join("php").join(format!("{minor}.ini"));
    if let Ok(extra) = std::fs::read_to_string(&user) {
        s.push_str("\n; ---- from ");
        s.push_str(&user.display().to_string());
        s.push_str(" ----\n");
        s.push_str(&extra);
        s.push('\n');
    }
}

pub fn write_ini(minor: &str, cfg: &Config) -> R<()> {
    let dir = paths::php_dir(minor);
    if !dir.exists() { return Ok(()); }
    std::fs::write(dir.join("php.ini"), ini_text(&dir, minor, cfg))?;
    Ok(())
}

pub fn write_all_inis(cfg: &Config) -> R<()> {
    for (minor, _) in installed() { write_ini(&minor, cfg)?; }
    Ok(())
}

/// Read a composer.json `require.php` constraint and pick the newest installed version that fits.
pub fn from_composer(project: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project.join("composer.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let constraint = json["require"]["php"].as_str()?.to_string();
    let mut inst: Vec<String> = installed().into_iter().map(|(m, _)| m).collect();
    inst.sort_by(|a, b| util::cmp_version(b, a));
    inst.into_iter().find(|m| satisfies(m, &constraint))
}

/// Minimal Composer constraint check on a minor version: handles ^, ~, >=, <, ||, ranges.
pub fn satisfies(minor: &str, constraint: &str) -> bool {
    constraint.replace("||", "|").split('|').any(|alt| {
        alt.split([',', ' ']).filter(|p| !p.trim().is_empty()).all(|part| {
            let p = part.trim();
            let (op, v) = if let Some(r) = p.strip_prefix(">=") { (">=", r) }
                else if let Some(r) = p.strip_prefix("<=") { ("<=", r) }
                else if let Some(r) = p.strip_prefix('>') { (">", r) }
                else if let Some(r) = p.strip_prefix('<') { ("<", r) }
                else if let Some(r) = p.strip_prefix('^') { ("^", r) }
                else if let Some(r) = p.strip_prefix('~') { ("~", r) }
                else { ("=", p) };
            let v = v.trim_start_matches('v');
            let want: Vec<u64> = v.split('.').map(|x| x.parse().unwrap_or(0)).collect();
            let have: Vec<u64> = minor.split('.').map(|x| x.parse().unwrap_or(0)).collect();
            let (hm, hn) = (have[0], have.get(1).copied().unwrap_or(0));
            let (wm, wn) = (want[0], want.get(1).copied().unwrap_or(0));
            let h = hm * 100 + hn;
            let w = wm * 100 + wn;
            match op {
                ">=" => h >= w,
                ">" => h > w,
                "<=" => h <= w,
                "<" => h < w || (h == w && want.len() > 2 && want[2] > 0),
                "^" => hm == wm && h >= w,
                "~" => if want.len() >= 3 { h == w } else { hm == wm && h >= w },
                _ => if v.contains('*') { hm == wm } else { h == w },
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composer_constraints() {
        assert!(satisfies("8.4", "^8.1"));
        assert!(satisfies("8.1", "^8.1"));
        assert!(!satisfies("8.0", "^8.1"));
        assert!(satisfies("8.4", "^8.4"));
        assert!(satisfies("8.3", "^7.4|^8.0"));
        assert!(satisfies("8.3", "^7.4 || ^8.0"));
        assert!(satisfies("8.2", ">=8.1 <8.4"));
        assert!(!satisfies("8.4", ">=8.1 <8.4"));
        assert!(satisfies("8.3", "~8.3.0"));
        assert!(!satisfies("8.4", "~8.3.0"));
    }
    #[test]
    fn picks_newest_static_build_with_fpm() {
        let names: Vec<String> = [
            "php-8.4.22-cli-linux-x86_64.tar.gz", "php-8.4.22-fpm-linux-x86_64.tar.gz",
            "php-8.4.23-cli-linux-x86_64.tar.gz", "php-8.4.23-fpm-linux-x86_64.tar.gz",
            "php-8.4.24-cli-linux-x86_64.tar.gz", // no fpm yet: skipped
            "php-8.3.32-cli-macos-aarch64.tar.gz", "php-8.3.32-fpm-macos-aarch64.tar.gz",
            "php-8.5.8-micro-linux-x86_64.tar.gz", "README.txt",
        ].iter().map(|s| s.to_string()).collect();
        let linux = static_pick(&names, "linux", "x86_64");
        assert_eq!(linux.len(), 1);
        assert_eq!((linux[0].minor.as_str(), linux[0].version.as_str()), ("8.4", "8.4.23"));
        assert_eq!(linux[0].zip, "php-8.4.23-cli-linux-x86_64.tar.gz");
        let mac = static_pick(&names, "macos", "aarch64");
        assert_eq!(mac[0].version, "8.3.32");
    }
    #[test]
    fn normalizes_versions() {
        assert_eq!(normalize("84").unwrap(), "8.4");
        assert_eq!(normalize("php8.1").unwrap(), "8.1");
        assert_eq!(normalize("8.3.12").unwrap(), "8.3");
        assert!(normalize("9.9").is_err());
    }
}
