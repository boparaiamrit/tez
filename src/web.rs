//! The web server: one Caddy process in front of every site. It serves HTTPS with its own local
//! certificate authority, HTTP/2, compression and websockets, and passes PHP requests to a pool
//! for the site's version: php-cgi processes on Windows, a php-fpm master on macOS and Linux.
//! Caddy runs no PHP itself: every site, whatever its version, runs on the official PHP builds.

use crate::config::Config;
use crate::fetch;
use crate::paths;
use crate::php;
use crate::sites::{self, Site};
use crate::spec::Spec;
use crate::sys;
use crate::util::{self, R};
use std::path::PathBuf;

pub const CADDY_VERSION: &str = "2.11.9";
pub const ADMIN: &str = "127.0.0.1:2019";

/// Caddy's release archive for this machine.
fn caddy_asset() -> String {
    let (os, ext) = match sys::os() { "windows" => ("windows", "zip"), "macos" => ("mac", "tar.gz"), _ => ("linux", "tar.gz") };
    let arch = if sys::arch() == "aarch64" { "arm64" } else { "amd64" };
    format!("caddy_{CADDY_VERSION}_{os}_{arch}.{ext}")
}

pub fn server_dir() -> PathBuf { paths::service_dir("caddy", CADDY_VERSION) }
pub fn server_exe() -> PathBuf { server_dir().join(sys::exe("caddy")) }

/// Download Caddy once; later calls return straight away.
pub fn install_server(_cfg: &Config) -> R<()> {
    if server_exe().exists() { return Ok(()); }
    util::step(&format!("Downloading Caddy {CADDY_VERSION} (the web server)"));
    let file = caddy_asset();
    let archive = fetch::download(&format!("https://github.com/caddyserver/caddy/releases/download/v{CADDY_VERSION}/{file}"), &file)?;
    fetch::extract(&archive, &server_dir(), &[])?;
    fetch::make_executable(&server_exe())?;
    util::ok(&format!("Caddy {CADDY_VERSION}"));
    Ok(())
}
/// php-cgi listens on 9000 + major*100 + minor*10 + n, so 8.4 uses 9840, 9841, ...
pub fn cgi_port(minor: &str, n: u16) -> u16 {
    let mut it = minor.split('.').map(|x| x.parse::<u16>().unwrap_or(0));
    let (a, b) = (it.next().unwrap_or(8), it.next().unwrap_or(0));
    9000 + a * 100 + b * 10 + n
}

fn workers(cfg: &Config) -> u16 { cfg.php_workers.clamp(1, 10) }

/// FastCGI addresses the web server sends a version's requests to: one php-cgi per worker on
/// Windows, one php-fpm master (with `workers` children) elsewhere.
pub fn cgi_upstreams(minor: &str, cfg: &Config) -> Vec<String> {
    let n = if cfg!(windows) { workers(cfg) } else { 1 };
    (0..n).map(|i| format!("127.0.0.1:{}", cgi_port(minor, i))).collect()
}

/// php-fpm's pool for one version: a fixed set of children, environment passed through.
fn write_fpm_conf(minor: &str, cfg: &Config) -> PathBuf {
    let dir = paths::php_dir(minor);
    let conf = dir.join("php-fpm.conf");
    let text = format!(
        "; Written by tez on every start.\n[global]\nerror_log = /dev/stderr\ndaemonize = no\nlog_level = warning\n\n[www]\nlisten = 127.0.0.1:{port}\npm = static\npm.max_children = {n}\npm.max_requests = 0\nclear_env = no\ncatch_workers_output = yes\ndecorate_workers_output = no\nrequest_terminate_timeout = 300\n",
        port = cgi_port(minor, 0),
        n = workers(cfg)
    );
    if std::fs::read_to_string(&conf).ok().as_deref() != Some(text.as_str()) { let _ = std::fs::write(&conf, text); }
    conf
}

/// The PHP pools for every PHP version a site needs.
pub fn cgi_specs(cfg: &Config) -> Vec<Spec> {
    let mut out = Vec::new();
    for minor in sites::needed_php(cfg) {
        if !cfg!(windows) {
            let conf = write_fpm_conf(&minor, cfg);
            let dir = paths::php_dir(&minor);
            out.push(
                Spec::new(format!("php-{minor}"), "php", php::php_cgi_exe(&minor))
                    .args(["-F".to_string(), "-y".into(), conf.display().to_string(), "-c".into(), dir.join("php.ini").display().to_string()])
                    .cwd(dir.clone())
                    .env("PHPRC", dir.display().to_string())
                    // Local PostgreSQL has no Kerberos; asking for GSSAPI encryption breaks static libpq on macOS.
                    .env("PGGSSENCMODE", "disable")
                    .port(cgi_port(&minor, 0)),
            );
            continue;
        }
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
        // The running server's own Caddyfile is the truth; the saved file can be stale.
        if let Ok(c) = std::fs::read_to_string(paths::config_dir().join("Caddyfile")) {
            let port = |key: &str| c.lines().find_map(|l| l.trim().strip_prefix(key).and_then(|v| v.trim().parse::<u16>().ok()));
            if let (Some(h), Some(s)) = (port("http_port"), port("https_port")) { return (h, s, Vec::new()); }
        }
        if let Ok(t) = std::fs::read_to_string(&saved) {
            let v: Vec<u16> = t.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if v.len() == 2 { return (v[0], v[1], Vec::new()); }
        }
    }
    let mut notes = Vec::new();
    let mut pick = |want: u16, alt: u16, what: &str| -> u16 {
        if util::port_free(want) { return want; }
        let why = if cfg!(target_os = "linux") && want < 1024 && !sys::is_elevated() { "needs root on Linux" } else { "is in use by another program" };
        notes.push(format!("port {want} {why}, so {what} uses {alt}"));
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
    s.push_str("\tlog {\n\t\tlevel WARN\n\t}\n}\n\n");

    // The dashboard.
    s.push_str(&format!(
        "http://tez.localhost, https://tez.localhost, http://tez.{tld}, https://tez.{tld} {{\n\ttls internal\n\treverse_proxy 127.0.0.1:{api}\n}}\n\n",
        tld = cfg.tld,
        api = cfg.api_port
    ));

    for site in sites::all(cfg) {
        let mut hosts = vec![
            format!("http://{}.{}", site.name, cfg.tld),
            format!("https://{}.{}", site.name, cfg.tld),
            format!("http://{}.localhost", site.name),
            format!("https://{}.localhost", site.name),
        ];
        for d in &site.settings.domains {
            let d = d.trim().to_ascii_lowercase();
            if d.is_empty() || d == "localhost" || d.contains(['/', ' ', ',', '{', '}']) { continue; }
            hosts.push(format!("http://{d}"));
            hosts.push(format!("https://{d}"));
        }
        s.push_str(&format!("# {}  ({})\n", site.name, site.path.display()));
        s.push_str(&hosts.join(", "));
        s.push_str(" {\n\ttls internal\n");
        // A JSON access log per site: the daemon reads it for the dashboard's request numbers.
        s.push_str(&format!(
            "\tlog {{\n\t\toutput file \"{}\" {{\n\t\t\troll_size 10MiB\n\t\t\troll_keep 2\n\t\t}}\n\t\tformat json\n\t}}\n",
            paths::fwd(&crate::stats::access_log(&site.name))
        ));
        s.push_str(&format!("\troot * \"{}\"\n", paths::fwd(&site.root)));
        s.push_str("\tencode zstd gzip\n");
        if php::is_installed(&site.php) {
            let ups = cgi_upstreams(&site.php, cfg);
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
    let exe = server_exe();
    Spec::new("web", "web", exe.clone())
        .args(["run".to_string(), "--config".into(), paths::fwd(&paths::caddyfile()), "--adapter".into(), "caddyfile".into()])
        .cwd(server_dir())
        .env("XDG_DATA_HOME", paths::caddy_storage().display().to_string())
        .port(https)
        .url(format!("http://127.0.0.1:{http}"))
        .stop_with(exe, vec!["stop".into(), "--address".into(), ADMIN.into()])
}

/// Apply a changed Caddyfile to the running server without dropping connections.
pub fn reload() -> R<()> {
    let out = std::process::Command::new(server_exe())
        .args(["reload", "--config", &paths::fwd(&paths::caddyfile()), "--adapter", "caddyfile", "--address", ADMIN])
        .output()?;
    if !out.status.success() {
        crate::bail!("web reload failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}
