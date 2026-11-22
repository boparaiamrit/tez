//! The web server: one FrankenPHP process (Caddy inside, plus an embedded PHP 8.6) in front of
//! every site. It serves HTTPS with its own local certificate authority, HTTP/2, compression and
//! websockets, and passes PHP requests to a pool of php-cgi processes for the site's version.
//! Sites on PHP 8.6 run inside FrankenPHP itself.

use crate::config::Config;
use crate::fetch;
use crate::paths;
use crate::php;
use crate::sites::{self, Site};
use crate::spec::Spec;
use crate::util::{self, R};
use std::path::PathBuf;

pub const FRANKENPHP_VERSION: &str = "1.14.0";
pub const EMBEDDED_PHP: &str = "8.6";
pub const ADMIN: &str = "127.0.0.1:2019";

pub fn frankenphp_dir() -> PathBuf { paths::service_dir("frankenphp", FRANKENPHP_VERSION) }
pub fn frankenphp_exe() -> PathBuf { frankenphp_dir().join("frankenphp.exe") }

pub fn install_frankenphp(cfg: &Config) -> R<()> {
    let dir = frankenphp_dir();
    if !frankenphp_exe().exists() {
        util::step(&format!("Downloading FrankenPHP {FRANKENPHP_VERSION} (web server with embedded PHP {EMBEDDED_PHP})"));
        let zip = fetch::download(
            &format!("https://github.com/php/frankenphp/releases/download/v{FRANKENPHP_VERSION}/frankenphp-windows-x86_64.zip"),
            &format!("frankenphp-windows-x86_64-v{FRANKENPHP_VERSION}.zip"),
        )?;
        fetch::extract(&zip, &dir, &[])?;
        for ext in ["redis", "mongodb"] {
            if let Err(e) = php::pecl_install_into(&dir, EMBEDDED_PHP, ext, true) {
                util::warn(&format!("{ext} for the embedded PHP {EMBEDDED_PHP}: {e}"));
            }
        }
        util::ok(&format!("FrankenPHP {FRANKENPHP_VERSION}"));
    }
    std::fs::write(dir.join("php.ini"), php::ini_text(&dir, EMBEDDED_PHP, cfg))?;
    Ok(())
}

/// php-cgi listens on 9000 + major*100 + minor*10 + n, so 8.4 uses 9840, 9841, ...
pub fn cgi_port(minor: &str, n: u16) -> u16 {
    let mut it = minor.split('.').map(|x| x.parse::<u16>().unwrap_or(0));
    let (a, b) = (it.next().unwrap_or(8), it.next().unwrap_or(0));
    9000 + a * 100 + b * 10 + n
}

fn workers(cfg: &Config) -> u16 { cfg.php_workers.clamp(1, 10) }

/// The php-cgi pools for every PHP version a site needs.
pub fn cgi_specs(cfg: &Config) -> Vec<Spec> {
    let mut out = Vec::new();
    for minor in sites::needed_php(cfg) {
        if minor == EMBEDDED_PHP { continue; }
        for n in 0..workers(cfg) {
            let port = cgi_port(&minor, n);
            out.push(
                Spec::new(format!("php-{minor}-{n}"), "php", php::php_cgi_exe(&minor))
                    .args(["-b".to_string(), format!("127.0.0.1:{port}")])
                    .cwd(paths::php_dir(&minor))
                    // php-cgi exits after 500 requests by default; tez restarts it, but 0 means never.
                    .env("PHP_FCGI_MAX_REQUESTS", "0")
                    .env("PHPRC", paths::php_dir(&minor).display().to_string())
                    .port(port),
            );
        }
    }
    out
}

/// Ports the web server should use: the configured ones, or the next free ones when taken.
pub fn effective_ports(cfg: &Config) -> (u16, u16, Vec<String>) {
    let saved = paths::run_dir().join("web-ports");
    // While our server runs it holds the ports itself, so reuse what it was started with.
    if util::port_open(2019) {
        if let Ok(t) = std::fs::read_to_string(&saved) {
            let v: Vec<u16> = t.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if v.len() == 2 { return (v[0], v[1], Vec::new()); }
        }
    }
    let mut notes = Vec::new();
    let mut pick = |want: u16, alt: u16, what: &str| -> u16 {
        if util::port_free(want) { return want; }
        notes.push(format!("port {want} is in use by another program, so {what} uses {alt}"));
        alt
    };
    let http = pick(cfg.http_port, 8088, "HTTP");
    let https = pick(cfg.https_port, 8443, "HTTPS");
    let _ = std::fs::write(&saved, format!("{http} {https}"));
    (http, https, notes)
}

/// Forget the saved ports so the next start checks them again.
pub fn reset_ports() { let _ = std::fs::remove_file(paths::run_dir().join("web-ports")); }

pub fn caddyfile(cfg: &Config) -> String {
    let (http, https, _) = effective_ports(cfg);
    let mut s = String::new();
    s.push_str("# Written by tez. Changes here are overwritten by `tez` on every reload.\n{\n");
    s.push_str(&format!("\tadmin {ADMIN}\n"));
    s.push_str("\tdefault_bind 127.0.0.1 [::1]\n");
    s.push_str(&format!("\thttp_port {http}\n\thttps_port {https}\n"));
    s.push_str("\tlocal_certs\n\tskip_install_trust\n\tauto_https disable_redirects\n");
    s.push_str(&format!("\tstorage file_system \"{}\"\n", paths::fwd(&paths::caddy_storage())));
    s.push_str("\tfrankenphp\n");
    s.push_str("\tlog {\n\t\tlevel WARN\n\t}\n}\n\n");

    // The dashboard.
    s.push_str(&format!(
        "http://tez.localhost, https://tez.localhost, http://tez.{tld}, https://tez.{tld} {{\n\ttls internal\n\treverse_proxy 127.0.0.1:{api}\n}}\n\n",
        tld = cfg.tld,
        api = cfg.api_port
    ));

    for site in sites::all(cfg) {
        let hosts = [
            format!("http://{}.{}", site.name, cfg.tld),
            format!("https://{}.{}", site.name, cfg.tld),
            format!("http://{}.localhost", site.name),
            format!("https://{}.localhost", site.name),
        ];
        s.push_str(&format!("# {}  ({})\n", site.name, site.path.display()));
        s.push_str(&hosts.join(", "));
        s.push_str(" {\n\ttls internal\n");
        s.push_str(&format!("\troot * \"{}\"\n", paths::fwd(&site.root)));
        s.push_str("\tencode zstd gzip\n");
        if site.php == EMBEDDED_PHP {
            s.push_str("\tphp_server\n");
        } else if php::is_installed(&site.php) {
            let ups: Vec<String> = (0..workers(cfg)).map(|n| format!("127.0.0.1:{}", cgi_port(&site.php, n))).collect();
            s.push_str(&format!("\tphp_fastcgi {} {{\n", ups.join(" ")));
            s.push_str("\t\tlb_policy least_conn\n\t\tlb_try_duration 30s\n\t\tlb_try_interval 50ms\n\t\tdial_timeout 2s\n\t\tread_timeout 300s\n\t}\n");
            s.push_str("\tfile_server\n");
        } else {
            s.push_str(&format!("\trespond \"tez: PHP {} is not installed. Run: tez php install {}\" 503\n", site.php, site.php));
        }
        s.push_str("}\n\n");
    }
    s
}

pub fn write_caddyfile(cfg: &Config) -> R<bool> {
    let text = caddyfile(cfg);
    let path = paths::caddyfile();
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    if old == text { return Ok(false); }
    std::fs::write(&path, text)?;
    Ok(true)
}

pub fn spec(cfg: &Config) -> Spec {
    let (http, https, _) = effective_ports(cfg);
    let exe = frankenphp_exe();
    Spec::new("web", "web", exe.clone())
        .args(["run".to_string(), "--config".into(), paths::fwd(&paths::caddyfile()), "--adapter".into(), "caddyfile".into()])
        .cwd(frankenphp_dir())
        .env("PHPRC", frankenphp_dir().display().to_string())
        .env("XDG_DATA_HOME", paths::caddy_storage().display().to_string())
        .port(https)
        .url(format!("http://127.0.0.1:{http}"))
        .stop_with(exe, vec!["stop".into(), "--address".into(), ADMIN.into()])
}

/// Apply a changed Caddyfile to the running server without dropping connections.
pub fn reload() -> R<()> {
    let out = std::process::Command::new(frankenphp_exe())
        .args(["reload", "--config", &paths::fwd(&paths::caddyfile()), "--adapter", "caddyfile", "--address", ADMIN])
        .env("PHPRC", frankenphp_dir().display().to_string())
        .output()?;
    if !out.status.success() {
        crate::bail!("web reload failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}
