//! Node.js: tez installs official builds from nodejs.org (each with npm and npx, and corepack for
//! pnpm and yarn where that release ships it), so no site depends on a Node.js installed by hand.
//! A site runs the version tez pins for it, else what its .nvmrc, .node-version or package.json
//! `engines.node` asks for, else the default.
//!
//! As `node`, `npm`, `npx`, `pnpm`, `yarn` and `corepack` (links to tez, like `php`) it runs the
//! version for the current folder, or the Node.js on PATH when tez has none installed.

use crate::bail;
use crate::config::{Config, SiteSettings};
use crate::fetch;
use crate::paths;
use crate::sites;
use crate::sys;
use crate::util::{self, bold, dim, R};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DIST: &str = "https://nodejs.org/dist";

/// The names tez answers to as Node.js shims.
pub const SHIMS: &[&str] = &["node", "npm", "npx", "pnpm", "yarn", "corepack"];

/// The oldest line offered for install; older ones are long out of support.
const OLDEST_MAJOR: u64 = 18;

/// LTS code names by major, for `lts/iron` in an .nvmrc when the release list is not at hand.
const LTS_NAMES: &[(&str, u64)] = &[
    ("argon", 4), ("boron", 6), ("carbon", 8), ("dubnium", 10), ("erbium", 12), ("fermium", 14),
    ("gallium", 16), ("hydrogen", 18), ("iron", 20), ("jod", 22), ("krypton", 24), ("lithium", 26),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    /// The LTS code name in lower case ("jod"), or None for a Current release.
    pub lts: Option<String>,
}

/// (file name part, archive type, the key nodejs.org lists in `files`) for this computer.
fn platform() -> Option<(&'static str, &'static str, &'static str)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some(("win-x64", "zip", "win-x64-zip")),
        ("windows", "aarch64") => Some(("win-arm64", "zip", "win-arm64-zip")),
        ("macos", "aarch64") => Some(("darwin-arm64", "tar.gz", "osx-arm64-tar")),
        ("macos", "x86_64") => Some(("darwin-x64", "tar.gz", "osx-x64-tar")),
        ("linux", "x86_64") => Some(("linux-x64", "tar.gz", "linux-x64")),
        ("linux", "aarch64") => Some(("linux-arm64", "tar.gz", "linux-arm64")),
        _ => None,
    }
}

// ---------------------------------------------------------------- releases

/// Releases in nodejs.org's index.json that have a build for `key`, newest first. Alpha, beta and
/// release-candidate builds are left out.
fn parse_index(text: &str, key: &str) -> Vec<Release> {
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    list.iter()
        .filter(|r| r["files"].as_array().map(|f| f.iter().any(|x| x == key)).unwrap_or(false))
        .filter_map(|r| {
            let version = r["version"].as_str()?.trim_start_matches('v').to_string();
            if version.contains('-') { return None; }
            Some(Release { version, lts: r["lts"].as_str().map(|s| s.to_ascii_lowercase()) })
        })
        .collect()
}

static LAST_FAILED: Mutex<Option<Instant>> = Mutex::new(None);

fn index_cache() -> PathBuf { paths::cache_dir().join("node-index.json") }

/// Every release with a build for this computer. The list is cached for six hours; offline, the
/// last copy is used, and a failed fetch is not retried for ten minutes.
pub fn releases() -> R<Vec<Release>> {
    let (_, _, key) = platform().ok_or("tez has no Node.js builds for this system")?;
    let cache = index_cache();
    let age = cache.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    let cached = || std::fs::read_to_string(&cache).ok();
    let recently_failed = LAST_FAILED.lock().unwrap().map(|t| t.elapsed() < Duration::from_secs(600)).unwrap_or(false);
    let text = match age {
        Some(a) if a < Duration::from_secs(6 * 3600) => cached(),
        _ if recently_failed => cached(),
        _ => match fetch::get_text(&format!("{DIST}/index.json")) {
            Ok(t) => {
                let _ = std::fs::create_dir_all(paths::cache_dir());
                let _ = std::fs::write(&cache, &t);
                Some(t)
            }
            Err(e) => {
                *LAST_FAILED.lock().unwrap() = Some(Instant::now());
                match cached() { Some(t) => Some(t), None => return Err(e) }
            }
        },
    };
    let text = text.ok_or("the Node.js release list is not available")?;
    Ok(parse_index(&text, key))
}

/// The LTS code name of a version: from the cached release list, else by its major.
fn lts_name(version: &str) -> Option<String> {
    if let Some((_, _, key)) = platform() {
        if let Ok(text) = std::fs::read_to_string(index_cache()) {
            if let Some(r) = parse_index(&text, key).into_iter().find(|r| r.version == version) { return r.lts; }
        }
    }
    let major = ver(version)?.0;
    LTS_NAMES.iter().find(|(_, m)| *m == major).map(|(n, _)| n.to_string())
}

// ---------------------------------------------------------------- where versions live

pub fn root() -> PathBuf { paths::home().join("node") }
pub fn dir(version: &str) -> PathBuf { root().join(version) }
/// The folder with `node` in it: a Windows build's root, `bin` elsewhere.
pub fn bin_dir(dir: &Path) -> PathBuf { if cfg!(windows) { dir.to_path_buf() } else { dir.join("bin") } }
pub fn exe(dir: &Path) -> PathBuf { bin_dir(dir).join(sys::exe("node")) }
/// npm and corepack: node_modules beside node.exe on Windows, lib/node_modules elsewhere.
fn modules(dir: &Path) -> PathBuf { if cfg!(windows) { dir.join("node_modules") } else { dir.join("lib").join("node_modules") } }

/// A version string tez can use as a folder name: digits and dots only.
fn valid(v: &str) -> bool { !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.') && !v.contains("..") }

/// Installed versions, newest first.
pub fn installed() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(root()).map(|rd| rd.flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            (valid(&n) && exe(&e.path()).exists()).then_some(n)
        }).collect()).unwrap_or_default();
    v.sort_by(|a, b| util::cmp_version(b, a));
    v
}

fn installed_tagged() -> Vec<(String, Option<String>)> { installed().into_iter().map(|v| { let l = lts_name(&v); (v, l) }).collect() }

fn npm_version(d: &Path) -> Option<String> {
    let text = std::fs::read_to_string(modules(d).join("npm").join("package.json")).ok()?;
    serde_json::from_str::<Value>(&text).ok()?["version"].as_str().map(|s| s.to_string())
}

fn has_corepack(d: &Path) -> bool { modules(d).join("corepack").join("dist").join("corepack.js").exists() }

fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten().map(|e| match e.file_type() {
        Ok(t) if t.is_dir() => dir_size(&e.path()),
        Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
        _ => 0,
    }).sum()
}

// ---------------------------------------------------------------- versions and ranges

type Ver = (u64, u64, u64);

/// "v22.11.0", "22.11", "22" -> (22, 11, 0); missing parts are 0.
fn ver(s: &str) -> Option<Ver> {
    let s = s.trim().trim_start_matches(['v', 'V', '=']);
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let a = it.next()?.trim().parse().ok()?;
    let b = match it.next() { Some(x) => x.parse().ok()?, None => 0 };
    let c = match it.next() { Some(x) => x.parse().ok()?, None => 0 };
    Some((a, b, c))
}

fn major(v: &str) -> u64 { ver(v).map(|x| x.0).unwrap_or(0) }

/// A version with parts left open ("22", "22.x", "22.11", "*"): None marks an open part.
type Partial = (Option<u64>, Option<u64>, Option<u64>);

fn partial(s: &str) -> Option<Partial> {
    let s = s.trim().trim_start_matches(['v', 'V', '=']).trim();
    if s.is_empty() || s == "*" || s.eq_ignore_ascii_case("x") { return Some((None, None, None)); }
    let core = s.split(['-', '+']).next().unwrap_or(s);
    let mut parts = core.split('.');
    let part = |x: Option<&str>| -> Option<Option<u64>> {
        match x { None => Some(None), Some(t) if t == "x" || t == "X" || t == "*" => Some(None), Some(t) => t.parse().ok().map(Some) }
    };
    let (a, b, c) = (part(parts.next())?, part(parts.next())?, part(parts.next())?);
    if parts.next().is_some() || (a.is_none() && (b.is_some() || c.is_some())) || (b.is_none() && c.is_some()) { return None; }
    Some((a, b, c))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op { Ge, Gt, Le, Lt, Eq }

fn low(p: Partial) -> Vec<(Op, Ver)> {
    match p.0 { Some(a) => vec![(Op::Ge, (a, p.1.unwrap_or(0), p.2.unwrap_or(0)))], None => Vec::new() }
}

/// Below the next version the open parts allow: 22 -> <23.0.0, 22.11 -> <22.12.0.
fn below_next(p: Partial) -> Vec<(Op, Ver)> {
    match p {
        (Some(a), Some(b), Some(c)) => vec![(Op::Le, (a, b, c))],
        (Some(a), Some(b), None) => vec![(Op::Lt, (a, b + 1, 0))],
        (Some(a), None, _) => vec![(Op::Lt, (a + 1, 0, 0))],
        _ => Vec::new(),
    }
}

/// One `||` alternative as plain comparisons; None when it is not a range tez understands.
fn comparators(alt: &str) -> Option<Vec<(Op, Ver)>> {
    let alt = alt.trim();
    if let Some((lo, hi)) = alt.split_once(" - ") {
        let mut out = low(partial(lo)?);
        out.extend(below_next(partial(hi)?));
        return Some(out);
    }
    // Join an operator written apart from its version (">= 18").
    let mut tokens: Vec<String> = Vec::new();
    let mut pending = String::new();
    for t in alt.split_whitespace() {
        if matches!(t, ">=" | ">" | "<=" | "<" | "=" | "^" | "~") { pending = t.to_string(); continue; }
        tokens.push(format!("{pending}{t}"));
        pending.clear();
    }
    let mut out = Vec::new();
    for tok in tokens {
        let (op, rest) = ["~>", ">=", "<=", ">", "<", "=", "^", "~"].iter()
            .find_map(|o| tok.strip_prefix(o).map(|r| (*o, r))).unwrap_or(("", tok.as_str()));
        let p = partial(rest)?;
        match op {
            "^" => {
                let Some(a) = p.0 else { continue };
                let (b, c) = (p.1.unwrap_or(0), p.2.unwrap_or(0));
                out.push((Op::Ge, (a, b, c)));
                out.push((Op::Lt, if a > 0 || p.1.is_none() { (a + 1, 0, 0) } else if b > 0 || p.2.is_none() { (0, b + 1, 0) } else { (0, 0, c + 1) }));
            }
            "~" | "~>" => {
                let Some(a) = p.0 else { continue };
                out.push((Op::Ge, (a, p.1.unwrap_or(0), p.2.unwrap_or(0))));
                out.push((Op::Lt, match p.1 { Some(b) => (a, b + 1, 0), None => (a + 1, 0, 0) }));
            }
            ">=" => out.extend(low(p)),
            ">" => out.push(match p {
                (Some(a), Some(b), Some(c)) => (Op::Gt, (a, b, c)),
                (Some(a), Some(b), None) => (Op::Ge, (a, b + 1, 0)),
                (Some(a), None, _) => (Op::Ge, (a + 1, 0, 0)),
                _ => (Op::Lt, (0, 0, 0)),
            }),
            "<=" => out.extend(below_next(p)),
            "<" => out.push((Op::Lt, (p.0.unwrap_or(0), p.1.unwrap_or(0), p.2.unwrap_or(0)))),
            _ => match p {
                (Some(a), Some(b), Some(c)) => out.push((Op::Eq, (a, b, c))),
                _ => { out.extend(low(p)); out.extend(below_next(p)); }
            },
        }
    }
    Some(out)
}

/// Whether `version` falls in an npm-style range ("22", "^20.9", ">=18 <23", "20 || 22", "*").
pub fn satisfies(version: &str, range: &str) -> bool {
    let Some(v) = ver(version) else { return false };
    range.split("||").any(|alt| comparators(alt).map(|cs| cs.iter().all(|(op, x)| match op {
        Op::Ge => v >= *x, Op::Gt => v > *x, Op::Le => v <= *x, Op::Lt => v < *x, Op::Eq => v == *x,
    })).unwrap_or(false))
}

/// The newest of `versions` that `spec` asks for: a range, `lts/*`, `lts/jod`, `node` or `latest`.
fn pick<'a>(spec: &str, versions: &'a [(String, Option<String>)]) -> Option<&'a String> {
    let s = spec.trim().to_ascii_lowercase();
    let newest = |keep: &dyn Fn(&(String, Option<String>)) -> bool| versions.iter().filter(|x| keep(x))
        .max_by(|a, b| util::cmp_version(&a.0, &b.0)).map(|x| &x.0);
    match s.as_str() {
        "" | "node" | "stable" | "latest" | "current" | "*" => newest(&|_| true),
        "lts" | "lts/*" | "lts/-0" => newest(&|x| x.1.is_some()),
        x if x.starts_with("lts/") => { let name = &x[4..]; newest(&|r| r.1.as_deref() == Some(name)) }
        _ => newest(&|r| satisfies(&r.0, &s)),
    }
}

// ---------------------------------------------------------------- which version a folder uses

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Choice {
    /// The installed version that runs, when tez has any.
    pub version: Option<String>,
    /// What was asked for ("22", ">=20", "lts/*"); empty when nothing asks.
    pub wanted: String,
    /// Where that came from: "set in tez", ".nvmrc", ".node-version", "package.json engines", "default".
    pub source: String,
    /// What was asked for is not installed, so `version` is the default instead.
    pub missing: bool,
}

/// The first line of an .nvmrc that is not empty or a comment.
fn first_line(text: &str) -> String {
    text.lines().map(|l| l.split('#').next().unwrap_or_default().trim()).find(|l| !l.is_empty()).unwrap_or_default().to_string()
}

/// What a project folder asks for, and where.
fn wanted_in(dir: &Path) -> Option<(String, &'static str)> {
    for (file, label) in [(".nvmrc", ".nvmrc"), (".node-version", ".node-version")] {
        if let Ok(t) = std::fs::read_to_string(dir.join(file)) {
            let v = first_line(&t);
            if !v.is_empty() { return Some((v, label)); }
        }
    }
    let pkg: Value = std::fs::read_to_string(dir.join("package.json")).ok().and_then(|t| serde_json::from_str(&t).ok())?;
    pkg["engines"]["node"].as_str().map(|e| e.trim().to_string()).filter(|e| !e.is_empty()).map(|e| (e, "package.json engines"))
}

/// The default: the one set with `tez node default`, else the newest LTS installed, else the newest.
pub fn default_version(cfg: &Config) -> Option<String> {
    let have = installed_tagged();
    if !cfg.default_node.is_empty() {
        if let Some(v) = pick(&cfg.default_node, &have) { return Some(v.clone()); }
    }
    pick("lts/*", &have).or_else(|| pick("", &have)).cloned()
}

fn choose_from(cfg: &Config, have: &[(String, Option<String>)], pin: Option<&str>, project: Option<&Path>) -> Choice {
    let (wanted, source) = match pin {
        Some(p) if !p.trim().is_empty() => (p.trim().to_string(), "set in tez"),
        _ => project.and_then(wanted_in).unwrap_or((String::new(), "default")),
    };
    let found = if wanted.is_empty() { None } else { pick(&wanted, have).cloned() };
    let missing = !wanted.is_empty() && found.is_none();
    let version = found.or_else(|| default_version(cfg));
    Choice { version, wanted, source: source.into(), missing }
}

pub fn choose(cfg: &Config, pin: Option<&str>, project: Option<&Path>) -> Choice { choose_from(cfg, &installed_tagged(), pin, project) }

pub fn for_site(cfg: &Config, site: &sites::Site) -> Choice { choose(cfg, site.settings.node.as_deref(), Some(&site.path)) }

/// A folder's choice: its site's, else the nearest .nvmrc, .node-version or package.json above it.
pub fn for_dir(cfg: &Config, dir: &Path) -> Choice {
    if let Some(site) = sites::for_dir(cfg, dir) { return for_site(cfg, &site); }
    let mut cur = Some(paths::absolute(dir));
    while let Some(d) = cur {
        if wanted_in(&d).is_some() { return choose(cfg, None, Some(&d)); }
        cur = d.parent().map(|p| p.to_path_buf());
    }
    choose(cfg, None, None)
}

/// The program and leading arguments that run `tool` on one installed version, or None when that
/// version cannot (no corepack for pnpm or yarn).
fn command_on(version: &str, tool: &str) -> Option<(PathBuf, Vec<String>)> {
    let d = dir(version);
    let node = exe(&d);
    if !node.exists() { return None; }
    let m = modules(&d);
    let js = |p: PathBuf| p.exists().then(|| (node.clone(), vec![p.display().to_string()]));
    match tool {
        "node" => Some((node.clone(), Vec::new())),
        "npm" => js(m.join("npm").join("bin").join("npm-cli.js")),
        "npx" => js(m.join("npm").join("bin").join("npx-cli.js")),
        "corepack" => js(m.join("corepack").join("dist").join("corepack.js")),
        // A global install (npm install -g pnpm) first, then corepack's.
        "pnpm" => js(m.join("pnpm").join("bin").join("pnpm.cjs")).or_else(|| js(m.join("corepack").join("dist").join("pnpm.js"))),
        "yarn" => js(m.join("yarn").join("bin").join("yarn.js")).or_else(|| js(m.join("corepack").join("dist").join("yarn.js"))),
        _ => None,
    }
}

/// How a site's process runs `tool` (npm for Vite) on tez's Node.js; None leaves it to PATH.
pub fn command_for(cfg: &Config, site: &sites::Site, tool: &str) -> Option<(PathBuf, Vec<String>)> {
    command_on(&for_site(cfg, site).version?, tool)
}

/// The same for a command run in a folder (dashboard jobs, `tez setup`).
pub fn command_in(dir: &Path, tool: &str) -> Option<(PathBuf, Vec<String>)> {
    command_on(&for_dir(&Config::load(), dir).version?, tool)
}

/// The folder to put first on a child process's PATH, so scripts that call `node` get the same one.
pub fn path_dir_in(dir: &Path) -> Option<PathBuf> { for_dir(&Config::load(), dir).version.map(|v| bin_dir(&self::dir(&v))) }
pub fn default_path_dir() -> Option<PathBuf> { default_version(&Config::load()).map(|v| bin_dir(&dir(&v))) }

fn path_with(first: &Path) -> String {
    format!("{}{}{}", first.display(), sys::PATH_SEP, std::env::var("PATH").unwrap_or_default())
}

// ---------------------------------------------------------------- the shims

fn missing_tool(tool: &str, version: &str) -> String {
    match tool {
        "pnpm" | "yarn" => format!("tez: Node.js {version} has no corepack (Node.js 25 and later leave it out), so {tool} is not available. Install it with `npm install -g {tool}`, or use Node.js 24: tez node install 24"),
        "corepack" => format!("tez: Node.js {version} has no corepack (Node.js 25 and later leave it out)"),
        _ => format!("tez: {tool} is missing from Node.js {version}; reinstall it: tez node remove {version} && tez node install {version}"),
    }
}

/// tez has no Node.js: run the one on PATH, skipping tez's own shims.
fn fallback(tool: &str, args: &[String]) -> i32 {
    let bin = paths::bin();
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat"] } else { &[""] };
    let found = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p)
        .filter(|d| !sites::same_path(d, &bin))
        .find_map(|d| exts.iter().map(|e| d.join(format!("{tool}{e}"))).find(|f| f.is_file())));
    match found {
        Some(p) => { let mut c = Command::new(p); c.args(args); crate::shim::run(c) }
        None => { eprintln!("tez: Node.js is not installed. Run: tez node install lts"); 127 }
    }
}

/// `node`, `npm`, `npx`, `pnpm`, `yarn`, `corepack`: the version for the current folder
/// (TEZ_NODE overrides it for one command).
pub fn shim(tool: &str, args: &[String]) -> i32 {
    use std::io::IsTerminal;
    let cfg = Config::load();
    let cwd = std::env::current_dir().unwrap_or_default();
    let choice = match std::env::var("TEZ_NODE").ok().filter(|v| !v.trim().is_empty()) {
        Some(v) => choose(&cfg, Some(&v), None),
        None => for_dir(&cfg, &cwd),
    };
    let Some(version) = choice.version.clone() else { return fallback(tool, args) };
    if choice.missing && std::io::stderr().is_terminal() {
        eprintln!("tez: {} asks for Node.js {}, which is not installed; using {version} (tez node install {})", choice.source, choice.wanted, choice.wanted);
    }
    let Some((exe, pre)) = command_on(&version, tool) else { eprintln!("{}", missing_tool(tool, &version)); return 1 };
    let mut c = Command::new(exe);
    c.args(pre).args(args).env("PATH", path_with(&bin_dir(&dir(&version))));
    if matches!(tool, "pnpm" | "yarn" | "corepack") { c.env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"); }
    crate::shim::run(c)
}

// ---------------------------------------------------------------- install, default, remove

fn sha_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let (h, f) = (it.next()?, it.next()?);
        (f == file && h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).then(|| h.to_ascii_lowercase())
    })
}

/// Install the newest release matching `spec` ("22", "22.11.0", "lts", "latest", ">=20"),
/// checked against that release's SHASUMS256.txt.
pub fn install(spec: &str) -> R<String> {
    let (suffix, ext, _) = platform().ok_or("tez has no Node.js builds for this system")?;
    let list: Vec<(String, Option<String>)> = releases()?.into_iter().map(|r| (r.version, r.lts)).collect();
    let spec = if spec.trim().is_empty() { "lts" } else { spec.trim() };
    let version = pick(spec, &list).cloned().ok_or_else(|| format!("no Node.js release matches '{spec}' on this system (tez node available)"))?;
    let dest = dir(&version);
    if exe(&dest).exists() { util::ok(&format!("Node.js {version} is already installed")); return Ok(version); }
    // The archive is about 30 MB and unpacks to about 100 MB.
    fetch::ensure_space(&paths::home(), 300 << 20)?;
    let file = format!("node-v{version}-{suffix}.{ext}");
    util::step(&format!("Downloading Node.js {version}"));
    let archive = fetch::download(&format!("{DIST}/v{version}/{file}"), &file)?;
    let sums = fetch::get_text(&format!("{DIST}/v{version}/SHASUMS256.txt"))?;
    let expected = sha_for(&sums, &file).ok_or_else(|| format!("SHASUMS256.txt for Node.js {version} does not list {file}"))?;
    fetch::verify_sha256(&archive, &expected)?;
    util::step(&format!("Unpacking into {}", dest.display()));
    fetch::extract_flat(&archive, &dest, &[])?;
    let _ = std::fs::remove_file(&archive);
    if !exe(&dest).exists() { bail!("the Node.js {version} archive has no {}", exe(&dest).display()); }
    fetch::make_executable(&exe(&dest))?;
    let extra = if has_corepack(&dest) { ", pnpm and yarn through corepack" } else { "" };
    util::ok(&format!("Node.js {version} with npm {}{extra}", npm_version(&dest).unwrap_or_else(|| "?".into())));
    Ok(version)
}

/// `tez install`: the newest LTS when tez has no Node.js yet.
pub fn ensure_installed() -> R<()> {
    if installed().is_empty() { install("lts")?; }
    Ok(())
}

/// Set the default to `spec` (kept as given, so "22" follows the newest 22 installed); empty
/// goes back to the newest LTS installed. Returns the version it picks now.
pub fn set_default(spec: &str) -> R<String> {
    let spec = spec.trim();
    let have = installed_tagged();
    let now = if spec.is_empty() { None } else { Some(pick(spec, &have).cloned().ok_or_else(|| format!("Node.js {spec} is not installed (tez node install {spec})"))?) };
    let mut cfg = Config::load();
    cfg.default_node = spec.to_string();
    cfg.save()?;
    Ok(now.or_else(|| default_version(&cfg)).unwrap_or_default())
}

pub fn remove(version: &str) -> R<()> {
    if !valid(version) { bail!("'{version}' is not a Node.js version"); }
    let d = dir(version);
    if !exe(&d).exists() { bail!("Node.js {version} is not installed"); }
    std::fs::remove_dir_all(&d).map_err(|e| format!("could not remove {} ({e}); stop what is using it first", d.display()))?;
    let mut cfg = Config::load();
    if !cfg.default_node.is_empty() && pick(&cfg.default_node, &installed_tagged()).is_none() {
        cfg.default_node.clear();
        cfg.save()?;
    }
    Ok(())
}

// ---------------------------------------------------------------- API

fn newest_per_major(list: &[Release]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut seen: Vec<u64> = Vec::new();
    let mut sorted = list.to_vec();
    sorted.sort_by(|a, b| util::cmp_version(&b.version, &a.version));
    for r in sorted {
        let m = major(&r.version);
        if m < OLDEST_MAJOR || seen.contains(&m) { continue; }
        seen.push(m);
        out.push(json!({ "major": m, "version": r.version, "lts": r.lts }));
    }
    out
}

/// GET /api/node: what is installed (with the default, size and the sites on each) and the newest
/// release of each line.
pub fn api_get() -> Value {
    let cfg = Config::load();
    let def = default_version(&cfg);
    let uses: Vec<(String, Choice)> = sites::all(&cfg).iter().map(|s| (s.name.clone(), for_site(&cfg, s))).collect();
    let installed: Vec<Value> = installed_tagged().into_iter().map(|(v, lts)| {
        let d = dir(&v);
        let users: Vec<&String> = uses.iter().filter(|(_, c)| c.version.as_deref() == Some(v.as_str())).map(|(n, _)| n).collect();
        json!({ "version": v, "major": major(&v), "lts": lts, "default": def.as_deref() == Some(v.as_str()), "size": dir_size(&d),
                "npm": npm_version(&d), "corepack": has_corepack(&d), "sites": users })
    }).collect();
    let (available, note) = match releases() { Ok(list) => (newest_per_major(&list), Value::Null), Err(e) => (Vec::new(), json!(e.to_string())) };
    json!({ "installed": installed, "default": def, "default_setting": cfg.default_node, "available": available, "note": note })
}

/// A spec someone typed: a version, a range or an alias, nothing that could be a path.
fn valid_spec(s: &str) -> bool { !s.is_empty() && s.len() < 40 && s.chars().all(|c| c.is_ascii_alphanumeric() || ".*/<>=^~ |-".contains(c)) && !s.contains("..") }

/// POST /api/node { install } | { default } | { remove }.
pub fn api_set(shared: &crate::daemon::Shared, body: &Value) -> R<Value> {
    if let Some(spec) = body["install"].as_str() {
        let spec = spec.trim();
        if !valid_spec(spec) { bail!("'{spec}' is not a Node.js version"); }
        let id = crate::manage::start_job(&["node".into(), "install".into(), spec.into()], None, &format!("Install Node.js {spec}"))?;
        return Ok(json!({ "ok": true, "job": id }));
    }
    if body.get("default").is_some() {
        let spec = body["default"].as_str().unwrap_or_default();
        if !spec.is_empty() && !valid_spec(spec) { bail!("'{spec}' is not a Node.js version"); }
        let now = set_default(spec)?;
        let notes = crate::daemon::reload(shared)?;
        return Ok(json!({ "ok": true, "default": now, "notes": notes }));
    }
    if let Some(v) = body["remove"].as_str() {
        remove(v)?;
        let notes = crate::daemon::reload(shared)?;
        return Ok(json!({ "ok": true, "notes": notes }));
    }
    bail!("send install, default or remove")
}

/// The site detail's "node": the version that runs, what asks for it and where from.
pub fn site_json(cfg: &Config, site: &sites::Site) -> Value {
    let c = for_site(cfg, site);
    json!({ "version": c.version, "wanted": c.wanted, "source": c.source, "missing": c.missing, "pinned": site.settings.node })
}

/// POST /api/site/<n> { node: "22" | null }: pin a version (null or "" for automatic). Returns a
/// version to install when the pin names one tez does not have.
pub fn apply_site(entry: &mut SiteSettings, body: &Value) -> R<Option<String>> {
    if body.get("node").is_none() { return Ok(None); }
    match body["node"].as_str().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(v) => {
            if !valid_spec(v) { bail!("'{v}' is not a Node.js version"); }
            entry.node = Some(v.to_string());
            Ok(pick(v, &installed_tagged()).is_none().then(|| v.to_string()))
        }
        None => { entry.node = None; Ok(None) }
    }
}

// ---------------------------------------------------------------- `tez node`

fn reload_daemon() { if crate::api::running() { let _ = crate::api::call("POST", "/api/reload"); } }

pub fn cli(args: &[&str]) -> R<()> {
    match args {
        [] | ["list"] => list(),
        ["install"] => { install("lts")?; reload_daemon(); Ok(()) }
        ["install", specs @ ..] => { for s in specs { install(s)?; } reload_daemon(); Ok(()) }
        ["default", v] | ["use", v] => {
            let now = set_default(v)?;
            util::ok(&format!("default Node.js: {now}"));
            reload_daemon();
            Ok(())
        }
        ["remove", v] | ["uninstall", v] => { remove(v)?; util::ok(&format!("removed Node.js {v}")); reload_daemon(); Ok(()) }
        ["available"] => available(),
        ["which"] => which(),
        _ => bail!("usage: tez node [list] | install <lts|22|22.11.0> | default <version> | remove <version> | available | which"),
    }
}

fn list() -> R<()> {
    let cfg = Config::load();
    let have = installed();
    if have.is_empty() {
        println!("No Node.js installed by tez yet. {}", dim("Install one: tez node install lts"));
        return Ok(());
    }
    let def = default_version(&cfg);
    let uses: Vec<(String, Choice)> = sites::all(&cfg).iter().map(|s| (s.name.clone(), for_site(&cfg, s))).collect();
    println!("{}", bold("Node.js"));
    for v in &have {
        let d = dir(v);
        let n = uses.iter().filter(|(_, c)| c.version.as_deref() == Some(v.as_str())).count();
        println!("  {:<10} {:<14} {:<9} {:<12} {}", v, lts_name(v).map(|l| format!("LTS {l}")).unwrap_or_default(),
            if def.as_deref() == Some(v.as_str()) { "default" } else { "" },
            npm_version(&d).map(|x| format!("npm {x}")).unwrap_or_default(),
            dim(&match n { 0 => String::new(), 1 => "1 site".into(), n => format!("{n} sites") }));
    }
    which()
}

fn available() -> R<()> {
    let have = installed();
    for r in newest_per_major(&releases()?) {
        let v = r["version"].as_str().unwrap_or_default();
        let tag = r["lts"].as_str().map(|l| format!("LTS {l}")).unwrap_or_else(|| "Current".into());
        println!("  {:<4} {:<10} {:<14} {}", r["major"], v, tag, if have.iter().any(|h| h == v) { "installed" } else { "" });
    }
    Ok(())
}

fn which() -> R<()> {
    let cfg = Config::load();
    let c = for_dir(&cfg, &std::env::current_dir()?);
    match &c.version {
        Some(v) => {
            let why = if c.wanted.is_empty() { "the default".to_string() } else if c.missing { format!("{} asks for {}, not installed; the default instead", c.source, c.wanted) } else { format!("{} asks for {}", c.source, c.wanted) };
            println!("this folder: Node.js {v} {}", dim(&format!("({why}) {}", exe(&dir(v)).display())));
        }
        None => println!("this folder: {}", dim("no tez Node.js; the node on PATH runs")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert!(satisfies("22.11.0", "22"));
        assert!(satisfies("22.11.0", "v22"));
        assert!(satisfies("22.11.0", "22.x"));
        assert!(!satisfies("23.0.0", "22"));
        assert!(satisfies("20.18.1", ">=18"));
        assert!(satisfies("20.18.1", ">= 18"));
        assert!(!satisfies("16.20.2", ">=18"));
        assert!(satisfies("20.9.0", "^20.9.0"));
        assert!(!satisfies("21.0.0", "^20.9.0"));
        assert!(!satisfies("20.8.9", "^20.9.0"));
        assert!(satisfies("20.11.1", "~20.11"));
        assert!(!satisfies("20.12.0", "~20.11"));
        assert!(satisfies("22.1.0", "18 || 20 || 22"));
        assert!(!satisfies("21.1.0", "18 || 20 || 22"));
        assert!(satisfies("20.5.0", ">=18 <21"));
        assert!(!satisfies("21.0.0", ">=18 <21"));
        assert!(satisfies("20.0.0", ">18"));
        assert!(!satisfies("18.20.0", ">18"));
        assert!(satisfies("20.3.0", "18.17 - 20"));
        assert!(!satisfies("21.0.0", "18.17 - 20"));
        assert!(satisfies("24.1.0", "*"));
        assert!(satisfies("22.11.0", "22.11.0"));
        assert!(!satisfies("22.11.1", "22.11.0"));
        assert!(!satisfies("22.0.0", "not a range"));
    }

    #[test]
    fn picks_the_newest_match() {
        let have = vec![("24.11.1".to_string(), Some("krypton".to_string())), ("23.11.0".to_string(), None),
                        ("22.21.0".to_string(), Some("jod".to_string())), ("20.19.5".to_string(), Some("iron".to_string()))];
        assert_eq!(pick("22", &have).map(|s| s.as_str()), Some("22.21.0"));
        assert_eq!(pick("lts/*", &have).map(|s| s.as_str()), Some("24.11.1"));
        assert_eq!(pick("lts/iron", &have).map(|s| s.as_str()), Some("20.19.5"));
        assert_eq!(pick("node", &have).map(|s| s.as_str()), Some("24.11.1"));
        assert_eq!(pick(">=20 <24", &have).map(|s| s.as_str()), Some("23.11.0"));
        assert_eq!(pick("18", &have), None);
    }

    #[test]
    fn reads_what_a_project_asks_for() {
        let d = std::env::temp_dir().join(format!("tez-node-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(wanted_in(&d), None);
        std::fs::write(d.join("package.json"), r#"{ "engines": { "node": ">=20.9" } }"#).unwrap();
        assert_eq!(wanted_in(&d), Some((">=20.9".to_string(), "package.json engines")));
        std::fs::write(d.join(".node-version"), "22.11.0\n").unwrap();
        assert_eq!(wanted_in(&d), Some(("22.11.0".to_string(), ".node-version")));
        std::fs::write(d.join(".nvmrc"), "# pinned for Vite 6\nlts/jod\n").unwrap();
        assert_eq!(wanted_in(&d), Some(("lts/jod".to_string(), ".nvmrc")));
        let have = vec![("24.11.1".to_string(), Some("krypton".to_string())), ("22.21.0".to_string(), Some("jod".to_string()))];
        let cfg = Config::default();
        let c = choose_from(&cfg, &have, None, Some(&d));
        assert_eq!((c.wanted.as_str(), c.source.as_str(), c.missing), ("lts/jod", ".nvmrc", false));
        assert_eq!(c.version.as_deref(), Some("22.21.0"));
        // A pin in tez wins over the project's files.
        let c = choose_from(&cfg, &have, Some("24"), Some(&d));
        assert_eq!((c.source.as_str(), c.version.as_deref()), ("set in tez", Some("24.11.1")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn parses_shasums() {
        let sums = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  node-v22.21.0-darwin-arm64.tar.gz\n\
                    ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789  node-v22.21.0-win-x64.zip\n";
        assert_eq!(sha_for(sums, "node-v22.21.0-win-x64.zip").as_deref(), Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"));
        assert_eq!(sha_for(sums, "node-v22.21.0-linux-x64.tar.gz"), None);
    }

    #[test]
    fn parses_the_release_index() {
        let text = r#"[
            {"version":"v27.0.0-alpha.0.1.1","date":"2026-11-01","files":["win-x64-zip"],"lts":false},
            {"version":"v25.2.0","date":"2025-11-11","files":["linux-x64","osx-arm64-tar","win-x64-zip"],"lts":false},
            {"version":"v24.11.1","date":"2025-11-11","files":["linux-x64","win-x64-zip"],"lts":"Krypton"},
            {"version":"v22.21.0","date":"2025-10-20","files":["osx-arm64-tar"],"lts":"Jod"}
        ]"#;
        let win = parse_index(text, "win-x64-zip");
        assert_eq!(win, vec![Release { version: "25.2.0".into(), lts: None }, Release { version: "24.11.1".into(), lts: Some("krypton".into()) }]);
        assert_eq!(parse_index(text, "osx-arm64-tar").len(), 2);
        let majors = newest_per_major(&win);
        assert_eq!(majors.len(), 2);
        assert_eq!(majors[0]["major"], 25);
        assert_eq!(parse_index("not json", "win-x64-zip"), Vec::new());
    }

    #[test]
    fn only_plain_versions_name_folders() {
        assert!(valid("22.21.0"));
        assert!(!valid("../22"));
        assert!(!valid("22/.."));
        assert!(valid_spec(">=20 <23"));
        assert!(valid_spec("lts/jod"));
        assert!(!valid_spec("../../etc"));
        assert!(!valid_spec("22; rm"));
    }
}
