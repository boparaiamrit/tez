//! tez: a fast, native Laravel development environment for Windows.
//!
//! One binary. Run as `tez` it is the command line and the background daemon; hard-linked as
//! `php.exe`, `composer.exe` and `exec.exe` it is the version-aware PHP, Composer and Horizon shims.

mod api;
mod cli;
mod config;
mod daemon;
mod fetch;
mod import;
mod manage;
mod paths;
mod php;
mod project;
mod services;
mod shim;
mod sites;
mod spec;
mod util;
mod web;
mod win;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let stem = std::path::Path::new(args.first().map(|s| s.as_str()).unwrap_or("tez"))
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let rest = if args.is_empty() { &[][..] } else { &args[1..] };
    let code = match stem.as_str() {
        "php" => shim::php(rest),
        "composer" => shim::composer(rest),
        "exec" => shim::exec(rest),
        _ => cli::run(rest),
    };
    std::process::exit(code);
}
