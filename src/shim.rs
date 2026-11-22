//! tez is also `php` and `composer` (and `exec` on Windows): links to the same binary.
//!
//! * `php` runs the PHP version the current folder's site uses (`tez isolate`, `.php-version`,
//!   composer.json), so `php artisan` always matches what the browser gets.
//! * `composer` runs composer.phar on that PHP. On Windows, platform checks for pcntl/posix are
//!   skipped, because tez provides Horizon support for them there.
//! * `exec` (Windows) lets the Unix `exec <command>` that Horizon writes in its process command
//!   lines work inside cmd.exe: it runs the rest of the line and returns its exit code.
//!
//! On macOS and Linux the shims replace themselves with PHP (exec), so signals, Ctrl+C and exit
//! codes behave exactly as if PHP had been started directly.

use crate::config::Config;
use crate::paths;
use crate::php;
use crate::sites;
use std::process::Command;

fn run(mut cmd: Command) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = cmd.exec();
        eprintln!("tez: {e}");
        127
    }
    #[cfg(not(unix))]
    {
        crate::sys::ignore_ctrl_c();
        match cmd.status() {
            Ok(s) => s.code().unwrap_or(1),
            Err(e) => { eprintln!("tez: {e}"); 127 }
        }
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

/// PHP reads the php.ini beside its binary. On macOS and Linux PHPRC also points there, so a
/// PHPRC inherited from another version's process never picks the wrong php.ini.
fn php_command(exe: &std::path::Path) -> Command {
    let mut c = Command::new(exe);
    if cfg!(unix) { if let Some(dir) = exe.parent() { c.env("PHPRC", dir); } }
    c
}

pub fn php(args: &[String]) -> i32 {
    match resolve_php() {
        Ok(exe) => { let mut c = php_command(&exe); c.args(args); run(c) }
        Err(e) => { eprintln!("tez: {e}"); 1 }
    }
}

pub fn composer(args: &[String]) -> i32 {
    let exe = match resolve_php() { Ok(e) => e, Err(e) => { eprintln!("tez: {e}"); return 1; } };
    let phar = paths::bin().join("composer.phar");
    if !phar.exists() { eprintln!("tez: composer.phar is missing; run `tez install`"); return 1; }
    let mut c = php_command(&exe);
    c.arg("-d").arg("memory_limit=-1").arg(phar).args(args);
    if cfg!(windows) && std::env::var_os("COMPOSER_IGNORE_PLATFORM_REQ").is_none() && std::env::var_os("COMPOSER_IGNORE_PLATFORM_REQS").is_none() {
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
