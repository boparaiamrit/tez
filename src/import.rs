//! `tez import`: bring a project from WSL (or any folder) into a parked folder, safely.
//!
//! It measures before copying, leaves out what Windows rebuilds (vendor, node_modules, caches,
//! logs), leaves out any folder over 1 GB unless asked for it (build outputs, mobile apps), and
//! refuses when the copy would leave the disk with less than 2 GB free. The source is only read.

use crate::config::Config;
use crate::fetch;
use crate::paths;
use crate::util::{self, bold, dim, R};
use crate::bail;
use std::path::{Path, PathBuf};
use std::process::Command;

const SKIP_DIRS: &[&str] = &["vendor", "node_modules", ".playwright", ".turbo", ".next", ".nuxt"];
const SKIP_SUBDIRS: &[&str] = &["storage/logs", "storage/framework/cache", "storage/framework/sessions", "storage/framework/views", "storage/debugbar"];
/// Compiled caches hold the old machine's absolute paths; Laravel rebuilds them.
const SKIP_FILES: &[&str] = &["hot", "config.php", "routes-v7.php", "packages.php", "services.php", "events.php"];
const BIG: u64 = 1 << 30;

fn source_path(src: &str, distro: &str) -> PathBuf {
    // A Linux path (/home/me/code/app) means a folder inside the WSL distro.
    if src.starts_with('/') {
        PathBuf::from(format!(r"\\wsl.localhost\{distro}{}", src.replace('/', "\\")))
    } else {
        PathBuf::from(src)
    }
}

/// Total bytes under a directory, via robocopy's list mode (fast over \\wsl.localhost).
fn measure(dir: &Path) -> u64 {
    let out = Command::new("robocopy")
        .arg(dir).arg(r"C:\__tez_nowhere__")
        .args(["/L", "/E", "/XJ", "/NJH", "/NFL", "/NDL", "/NP", "/BYTES", "/R:0", "/W:0"])
        .output();
    let Ok(out) = out else { return 0 };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("Bytes :") {
            return rest.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
        }
    }
    0
}

fn top_files_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map(|rd| rd.flatten().filter(|e| e.path().is_file()).filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0)
}

pub fn ensure_laravel_dirs(project: &Path) {
    if !project.join("artisan").exists() { return; }
    for d in ["storage/logs", "storage/framework/cache/data", "storage/framework/sessions", "storage/framework/views", "storage/framework/testing", "bootstrap/cache"] {
        let _ = std::fs::create_dir_all(project.join(d.replace('/', "\\")));
    }
}

pub fn run(args: &[&str]) -> R<()> {
    let Some(src_arg) = args.first() else { bail!("usage: tez import <folder or /wsl/path> [name] [--include dir] [--exclude dir] [--distro Ubuntu]") };
    let mut name: Option<String> = None;
    let mut include: Vec<String> = Vec::new();
    let mut exclude: Vec<String> = Vec::new();
    let mut distro = "Ubuntu".to_string();
    let mut i = 1;
    while i < args.len() {
        match args[i] {
            "--include" => { if let Some(v) = args.get(i + 1) { include.push(v.to_string()); } i += 2; }
            "--exclude" => { if let Some(v) = args.get(i + 1) { exclude.push(v.to_string()); } i += 2; }
            "--distro" => { if let Some(v) = args.get(i + 1) { distro = v.to_string(); } i += 2; }
            other => { name = Some(other.to_string()); i += 1; }
        }
    }
    let src = source_path(src_arg, &distro);
    if !src.is_dir() { bail!("{} is not a folder I can read", src.display()); }
    let cfg = Config::load();
    let parked = cfg.parked.first().cloned().unwrap_or_else(|| format!("{}\\Sites", std::env::var("USERPROFILE").unwrap_or_default()));
    let folder = name.unwrap_or_else(|| src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "site".into()));
    let dest = PathBuf::from(&parked).join(&folder);
    if dest.join("artisan").exists() || dest.join("composer.json").exists() {
        bail!("{} already exists; pass another name: tez import {src_arg} <name>", dest.display());
    }

    util::step(&format!("Measuring {}", src.display()));
    let mut skipped: Vec<(String, u64, &str)> = Vec::new();
    let mut total = top_files_size(&src);
    let mut excluded_dirs: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&src)?.flatten() {
        let p = entry.path();
        if !p.is_dir() { continue; }
        let n = entry.file_name().to_string_lossy().to_string();
        let wanted = include.iter().any(|x| x == &n);
        if !wanted && (SKIP_DIRS.contains(&n.as_str()) || exclude.iter().any(|x| x == &n)) {
            excluded_dirs.push(p);
            skipped.push((n, 0, "rebuilt on Windows or excluded"));
            continue;
        }
        let size = measure(&p);
        if size > BIG && !wanted {
            excluded_dirs.push(p);
            skipped.push((n, size, "over 1 GB, left out (add --include to copy it)"));
            continue;
        }
        total += size;
    }
    for sub in SKIP_SUBDIRS { excluded_dirs.push(src.join(sub.replace('/', "\\"))); }
    for (n, size, why) in &skipped {
        println!("  {} {:<16} {}", dim("skip"), n, dim(&format!("{}{why}", if *size > 0 { format!("{}  ", util::human_bytes(*size)) } else { String::new() })));
    }
    println!("  {} {}", bold("copy"), util::human_bytes(total));
    fetch::ensure_space(Path::new(&parked), total)?;

    util::step(&format!("Copying to {}", dest.display()));
    let mut cmd = Command::new("robocopy");
    cmd.arg(&src).arg(&dest).args(["/E", "/XJ", "/MT:32", "/R:1", "/W:1", "/NFL", "/NDL", "/NP", "/NJH"]);
    cmd.arg("/XD");
    for d in &excluded_dirs { cmd.arg(d); }
    cmd.arg("/XF");
    for f in SKIP_FILES { cmd.arg(f); }
    let status = cmd.status()?;
    // robocopy: 0-7 are success codes (files copied, extras, mismatches), 8+ means failures.
    if status.code().unwrap_or(16) >= 8 { bail!("the copy did not finish cleanly (robocopy code {})", status.code().unwrap_or(-1)); }
    // The skipped cache and log folders must exist (empty), or Laravel refuses to boot.
    ensure_laravel_dirs(&dest);
    util::ok(&format!("{folder} is at {} and served at https://{}.{}", dest.display(), util::host_label(&folder), cfg.tld));
    println!("  next, in that folder:");
    println!("    {}   point .env at tez's services and create the database", bold("tez adopt"));
    println!("    {}   PHP packages for Windows", bold("composer install"));
    println!("    {}   JavaScript packages for Windows", bold("npm install"));
    println!("    {}   tables", bold("php artisan migrate"));
    println!("    {}   Horizon, Reverb, Vite", bold("tez up"));
    Ok(())
}
