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
/// Compiled caches hold the old machine's absolute paths; Laravel rebuilds them. These are paths
/// from the project root, never bare names: `config/services.php` must still be copied.
const SKIP_FILES: &[&str] = &[
    "bootstrap/cache/config.php", "bootstrap/cache/routes-v7.php", "bootstrap/cache/packages.php",
    "bootstrap/cache/services.php", "bootstrap/cache/events.php", "public/hot",
];
/// Kept inside the skipped cache and log folders, so git sees no deleted files.
const KEEP_IN_SKIPPED: &str = ".gitignore";
const BIG: u64 = 1 << 30;

fn source_path(src: &str, distro: &str) -> PathBuf {
    // On Windows a Linux path (/home/me/code/app) means a folder inside the WSL distro.
    if cfg!(windows) && src.starts_with('/') {
        PathBuf::from(format!(r"\\wsl.localhost\{distro}{}", src.replace('/', "\\")))
    } else if cfg!(windows) {
        PathBuf::from(src)
    } else {
        paths::absolute(Path::new(src))
    }
}

/// Total bytes under a directory: robocopy's list mode on Windows (fast over \\wsl.localhost),
/// `du -sk` elsewhere.
fn measure(dir: &Path) -> u64 {
    if !cfg!(windows) {
        let out = Command::new("du").arg("-sk").arg(dir).output();
        return out.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().next().and_then(|n| n.parse::<u64>().ok())).map(|kb| kb * 1024).unwrap_or(0);
    }
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
        let _ = std::fs::create_dir_all(paths::join_rel(project, d));
    }
}

/// The skipped files as absolute paths under `src`.
fn skip_files(src: &Path) -> Vec<PathBuf> {
    SKIP_FILES.iter().map(|f| paths::join_rel(src, f)).collect()
}

/// Copy only the `.gitignore` files under `src` into the same places under `dest`. The skipped
/// cache and log folders hold one each, and Laravel's repo tracks them.
fn copy_kept(src: &Path, dest: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(src) else { return 0 };
    let mut copied = 0;
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            copied += copy_kept(&p, &dest.join(e.file_name()));
        } else if ft.is_file() && e.file_name() == KEEP_IN_SKIPPED {
            let _ = std::fs::create_dir_all(dest);
            copied += std::fs::copy(&p, dest.join(e.file_name())).unwrap_or(0);
        }
    }
    copied
}

/// Copy a tree, leaving out `skip_dirs` and `skip_files` (both absolute). Symlinks are copied as
/// links, never followed. (macOS and Linux; Windows uses robocopy.)
#[cfg(unix)]
fn copy_filtered(src: &Path, dest: &Path, skip_dirs: &[PathBuf], skip_files: &[PathBuf]) -> R<u64> {
    std::fs::create_dir_all(dest)?;
    let mut copied = 0;
    for e in std::fs::read_dir(src)?.flatten() {
        let p = e.path();
        let to = dest.join(e.file_name());
        let ft = e.file_type()?;
        if ft.is_symlink() {
            let target = std::fs::read_link(&p)?;
            let _ = std::fs::remove_file(&to);
            std::os::unix::fs::symlink(target, &to)?;
        } else if ft.is_dir() {
            if skip_dirs.iter().any(|d| d == &p) { continue; }
            copied += copy_filtered(&p, &to, skip_dirs, skip_files)?;
        } else {
            if skip_files.iter().any(|f| f == &p) { continue; }
            copied += std::fs::copy(&p, &to)?;
        }
    }
    Ok(copied)
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
    let parked = cfg.parked.first().cloned().unwrap_or_else(|| paths::user_home().join("Sites").display().to_string());
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
    for sub in SKIP_SUBDIRS { excluded_dirs.push(paths::join_rel(&src, sub)); }
    for (n, size, why) in &skipped {
        println!("  {} {:<16} {}", dim("skip"), n, dim(&format!("{}{why}", if *size > 0 { format!("{}  ", util::human_bytes(*size)) } else { String::new() })));
    }
    println!("  {} {}", bold("copy"), util::human_bytes(total));
    fetch::ensure_space(Path::new(&parked), total)?;

    util::step(&format!("Copying to {}", dest.display()));
    #[cfg(unix)]
    {
        let bytes = copy_filtered(&src, &dest, &excluded_dirs, &skip_files(&src))?;
        util::ok(&format!("copied {}", util::human_bytes(bytes)));
    }
    #[cfg(windows)]
    {
        let mut cmd = Command::new("robocopy");
        cmd.arg(&src).arg(&dest).args(["/E", "/XJ", "/MT:32", "/R:1", "/W:1", "/NFL", "/NDL", "/NP", "/NJH"]);
        cmd.arg("/XD");
        for d in &excluded_dirs { cmd.arg(d); }
        // Full paths: a bare name would also leave out every other file with that name.
        cmd.arg("/XF");
        for f in skip_files(&src) { cmd.arg(f); }
        let status = cmd.status()?;
        // robocopy: 0-7 are success codes (files copied, extras, mismatches), 8+ means failures.
        if status.code().unwrap_or(16) >= 8 { bail!("the copy did not finish cleanly (robocopy code {})", status.code().unwrap_or(-1)); }
    }
    // The skipped cache and log folders must exist, or Laravel refuses to boot; their .gitignore
    // files come across so git shows nothing deleted.
    ensure_laravel_dirs(&dest);
    for sub in SKIP_SUBDIRS { copy_kept(&paths::join_rel(&src, sub), &paths::join_rel(&dest, sub)); }
    util::ok(&format!("{folder} is at {} and served at https://{}.{}", dest.display(), util::host_label(&folder), cfg.tld));
    println!("  next, in that folder:");
    println!("    {}   point .env at tez's services and create the database", bold("tez adopt"));
    println!("    {}   PHP packages for this OS", bold("composer install"));
    println!("    {}   JavaScript packages for this OS", bold("npm install"));
    println!("    {}   tables", bold("php artisan migrate"));
    println!("    {}   Horizon, Reverb, Vite", bold("tez up"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_compiled_caches_but_not_config_files() {
        let src = Path::new("project");
        let skip = skip_files(src);
        assert!(skip.contains(&paths::join_rel(src, "bootstrap/cache/services.php")));
        assert!(skip.contains(&paths::join_rel(src, "public/hot")));
        assert!(!skip.contains(&paths::join_rel(src, "config/services.php")));
        assert!(!skip.contains(&paths::join_rel(src, "config/events.php")));
    }

    #[test]
    fn keeps_gitignore_files_from_skipped_folders() {
        let root = std::env::temp_dir().join(format!("tez-import-test-{}", std::process::id()));
        let (src, dest) = (root.join("src"), root.join("dest"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(src.join("storage/framework/cache/data/ab")).unwrap();
        std::fs::write(src.join("storage/framework/cache/.gitignore"), "*\n").unwrap();
        std::fs::write(src.join("storage/framework/cache/data/.gitignore"), "*\n").unwrap();
        std::fs::write(src.join("storage/framework/cache/data/ab/blob"), "cached").unwrap();
        copy_kept(&src.join("storage/framework/cache"), &dest.join("storage/framework/cache"));
        assert!(dest.join("storage/framework/cache/.gitignore").is_file());
        assert!(dest.join("storage/framework/cache/data/.gitignore").is_file());
        assert!(!dest.join("storage/framework/cache/data/ab/blob").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
