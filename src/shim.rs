//! tez.exe is also php.exe, composer.exe and exec.exe (hard links to the same file).
//!
//! * `php` runs the PHP version the current folder's site uses (`tez isolate`, `.php-version`,
//!   composer.json), so `php artisan` always matches what the browser gets.
//! * `composer` runs composer.phar on that PHP. Platform checks for pcntl/posix are skipped,
//!   because tez provides Horizon support for them on Windows.
//! * `exec` lets the Unix `exec <command>` that Horizon writes in its process command lines work
//!   inside cmd.exe: it runs the rest of the line and returns its exit code.

use crate::config::Config;
use crate::paths;
use crate::php;
use crate::sites;
use crate::win;
use std::process::Command;

fn run(mut cmd: Command) -> i32 {
    win::ignore_ctrl_c();
    match cmd.status() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => { eprintln!("tez: {e}"); 127 }
    }
}

fn resolve_php() -> Result<std::path::PathBuf, String> {
    let cfg = Config::load();
    let cwd = std::env::current_dir().unwrap_or_default();
    let minor = std::env::var("TEZ_PHP").ok().and_then(|v| php::normalize(&v).ok()).unwrap_or_else(|| sites::php_for_dir(&cfg, &cwd));
    let exe = php::php_exe(&minor);
    if exe.exists() { return Ok(exe); }
    let fallback = php::php_exe(&cfg.default_php);
    if fallback.exists() { return Ok(fallback); }
    Err(format!("PHP {minor} is not installed. Run: tez php install {minor}"))
}

pub fn php(args: &[String]) -> i32 {
    match resolve_php() {
        Ok(exe) => { let mut c = Command::new(exe); c.args(args); run(c) }
        Err(e) => { eprintln!("tez: {e}"); 1 }
    }
}

pub fn composer(args: &[String]) -> i32 {
    let exe = match resolve_php() { Ok(e) => e, Err(e) => { eprintln!("tez: {e}"); return 1; } };
    let phar = paths::bin().join("composer.phar");
    if !phar.exists() { eprintln!("tez: composer.phar is missing; run `tez install`"); return 1; }
    let mut c = Command::new(exe);
    c.arg("-d").arg("memory_limit=-1").arg(phar).args(args);
    if std::env::var_os("COMPOSER_IGNORE_PLATFORM_REQ").is_none() && std::env::var_os("COMPOSER_IGNORE_PLATFORM_REQS").is_none() {
        c.env("COMPOSER_IGNORE_PLATFORM_REQ", "ext-pcntl,ext-posix");
    }
    run(c)
}

pub fn exec(args: &[String]) -> i32 {
    let Some((first, rest)) = args.split_first() else { return 0 };
    // `exec ps aux | grep ...` (Horizon's process counter) has nothing to count here.
    if first == "ps" { return 0; }
    let mut c = Command::new(first.trim_matches('"'));
    c.args(rest);
    run(c)
}
