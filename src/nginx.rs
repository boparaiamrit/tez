//! Nginx, the alternative to Caddy in front of every site. It serves the same host names and
//! ports, passes PHP to the same pools, and uses one certificate signed by tez's own authority
//! (see tls.rs). Its files: the program in `services/nginx/<version>`, `config/nginx/nginx.conf`
//! with tez's own `mime.types` and `fastcgi_params` beside it, and `data/nginx` as its prefix
//! (`logs/`, `temp/`).

use crate::config::Config;
use crate::fetch;
use crate::paths;
use crate::php;
use crate::sites;
use crate::spec::Spec;
use crate::sys::{self, Hidden};
use crate::tls;
use crate::util::{self, R};
use crate::web;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// nginx.org's own Windows build (latest stable); jirutka/nginx-binaries' static builds elsewhere,
/// whose newest stable release is a little behind nginx.org's.
pub const VERSION: &str = if cfg!(windows) { "1.30.7" } else { "1.30.5" };

/// Whether Nginx can run on this computer, and if not, why.
pub fn available() -> (bool, String) {
    match (sys::os(), sys::arch()) {
        // nginx.org ships a 32-bit x86 build, which Windows on Arm runs too.
        ("windows", _) | ("linux", "x86_64") | ("linux", "aarch64") => (true, String::new()),
        ("macos", arch) => {
            // The macOS builds link Homebrew's PCRE2 instead of bundling it.
            let lib = if arch == "aarch64" { "/opt/homebrew/opt/pcre2/lib/libpcre2-8.0.dylib" } else { "/usr/local/opt/pcre2/lib/libpcre2-8.0.dylib" };
            if std::path::Path::new(lib).exists() { (true, String::new()) }
            else { (false, "The macOS build of Nginx needs Homebrew's PCRE2. Run: brew install pcre2".into()) }
        }
        (os, arch) => (false, format!("No Nginx build is published for {os} on {arch}")),
    }
}

fn download_url() -> (String, String) {
    if cfg!(windows) {
        let file = format!("nginx-{VERSION}.zip");
        return (format!("https://nginx.org/download/{file}"), file);
    }
    let target = match (sys::os(), sys::arch()) {
        ("macos", "aarch64") => "arm64-darwin",
        ("macos", _) => "x86_64-darwin",
        (_, "aarch64") => "aarch64-linux",
        _ => "x86_64-linux",
    };
    let file = format!("nginx-{VERSION}-{target}");
    (format!("https://jirutka.github.io/nginx-binaries/{file}"), file)
}

pub fn dir() -> PathBuf { paths::service_dir("nginx", VERSION) }
pub fn exe() -> PathBuf { dir().join(sys::exe("nginx")) }
pub fn prefix() -> PathBuf { paths::data_dir("nginx") }
pub fn conf_dir() -> PathBuf { paths::config_dir().join("nginx") }
pub fn conf() -> PathBuf { conf_dir().join("nginx.conf") }
fn pid_file() -> PathBuf { prefix().join("logs").join("nginx.pid") }

/// Download Nginx once; later calls return straight away.
pub fn install() -> R<()> {
    if exe().exists() { return Ok(()); }
    let (ok, why) = available();
    if !ok { crate::bail!("{why}"); }
    util::step(&format!("Downloading Nginx {VERSION} (the web server)"));
    let (url, file) = download_url();
    let got = fetch::download(&url, &file)?;
    if cfg!(windows) {
        fetch::extract_flat(&got, &dir(), &[])?;
    } else {
        std::fs::create_dir_all(dir())?;
        std::fs::copy(&got, exe())?;
        fetch::make_executable(&exe())?;
    }
    if !exe().exists() { crate::bail!("the Nginx download did not contain {}", exe().display()); }
    util::ok(&format!("Nginx {VERSION}"));
    Ok(())
}

/// Each request as one JSON line, in the shape Caddy writes, so stats.rs reads either server's log.
pub const LOG_FORMAT: &str = r#"'{"ts":$msec,"request":{"remote_ip":"$remote_addr","proto":"$server_protocol","method":"$request_method","host":"$host","uri":"$request_uri"},"duration":$request_time,"size":$bytes_sent,"status":$status}'"#;

/// One quoted nginx string (paths may hold spaces).
fn q(s: &str) -> String { format!("\"{}\"", s.replace('\\', "/").replace('"', "")) }

/// The `listen` lines for one server block, IPv4 and IPv6 loopback, plain and TLS.
fn listens(http: u16, https: u16, extra: &str) -> String {
    let mut s = String::new();
    for addr in ["127.0.0.1", "[::1]"] {
        s.push_str(&format!("\t\tlisten {addr}:{http}{extra};\n"));
        s.push_str(&format!("\t\tlisten {addr}:{https} ssl{extra};\n"));
    }
    s.push_str("\t\thttp2 on;\n");
    s
}

/// Every host name Nginx answers on, which the one certificate has to cover.
pub fn all_hosts(cfg: &Config) -> Vec<String> {
    let mut hosts = vec!["tez.localhost".to_string(), format!("tez.{}", cfg.tld)];
    for site in sites::all(cfg) { hosts.extend(web::site_hosts(cfg, &site)); }
    hosts
}

/// The upstream block name for a PHP version: php_8_4.
fn upstream(minor: &str) -> String { format!("php_{}", minor.replace('.', "_")) }

pub fn conf_text(cfg: &Config, http: u16, https: u16, cert: &str, key: &str) -> String {
    let mut s = String::new();
    s.push_str("# Written by tez. Changes here are overwritten by `tez` on every reload.\n");
    s.push_str(&format!("# tez ports: http {http} https {https}\n"));
    // Nginx for Windows does its work in a single worker whatever the count.
    s.push_str(&format!("worker_processes {};\n", if cfg!(windows) { 1 } else { 2 }));
    s.push_str("error_log \"logs/error.log\" warn;\npid \"logs/nginx.pid\";\n\nevents {\n\tworker_connections 1024;\n}\n\nhttp {\n");
    s.push_str("\tinclude mime.types;\n\tdefault_type application/octet-stream;\n");
    s.push_str("\tsendfile on;\n\tkeepalive_timeout 65;\n\tserver_tokens off;\n\tabsolute_redirect off;\n");
    s.push_str("\tclient_max_body_size 512m;\n\tlarge_client_header_buffers 4 32k;\n");
    s.push_str("\tserver_names_hash_bucket_size 128;\n\tserver_names_hash_max_size 1024;\n\ttypes_hash_max_size 2048;\n");
    s.push_str("\tgzip on;\n\tgzip_vary on;\n\tgzip_proxied any;\n\tgzip_comp_level 5;\n\tgzip_min_length 256;\n");
    s.push_str("\tgzip_types text/plain text/css text/xml text/javascript application/javascript application/json application/xml application/rss+xml application/manifest+json image/svg+xml font/ttf font/otf application/wasm;\n");
    for (d, t) in [("client_body_temp_path", "client_body"), ("proxy_temp_path", "proxy"), ("fastcgi_temp_path", "fastcgi"), ("uwsgi_temp_path", "uwsgi"), ("scgi_temp_path", "scgi")] {
        s.push_str(&format!("\t{d} \"temp/{t}\";\n"));
    }
    s.push_str(&format!("\tlog_format tez_json escape=json {LOG_FORMAT};\n\taccess_log off;\n"));
    s.push_str(&format!("\tssl_certificate {};\n\tssl_certificate_key {};\n", q(cert), q(key)));
    s.push_str("\tssl_protocols TLSv1.2 TLSv1.3;\n\tssl_session_timeout 1d;\n");
    s.push_str("\tfastcgi_buffer_size 32k;\n\tfastcgi_buffers 16 32k;\n\tfastcgi_connect_timeout 2s;\n\tfastcgi_send_timeout 300s;\n\tfastcgi_read_timeout 300s;\n");
    // Websockets through the proxy: pass Upgrade on, close otherwise.
    s.push_str("\tmap $http_upgrade $connection_upgrade {\n\t\tdefault upgrade;\n\t\t'' close;\n\t}\n\n");

    let site_list = sites::all(cfg);
    let mut versions: Vec<String> = site_list.iter().filter(|x| php::is_installed(&x.php)).map(|x| x.php.clone()).collect();
    versions.sort();
    versions.dedup();
    for v in &versions {
        s.push_str(&format!("\tupstream {} {{\n\t\tleast_conn;\n", upstream(v)));
        // max_fails=0: a worker that is restarting is skipped for that request only.
        for addr in web::cgi_upstreams(v, cfg) { s.push_str(&format!("\t\tserver {addr} max_fails=0;\n")); }
        s.push_str("\t}\n");
    }
    s.push('\n');

    // Unknown host names get a 404 instead of whichever site happens to be first.
    s.push_str("\tserver {\n");
    s.push_str(&listens(http, https, " default_server"));
    s.push_str("\t\tserver_name _;\n\t\treturn 404;\n\t}\n\n");

    // The dashboard.
    s.push_str("\tserver {\n");
    s.push_str(&listens(http, https, ""));
    s.push_str(&format!("\t\tserver_name tez.localhost tez.{};\n", cfg.tld));
    s.push_str(&format!("\t\tlocation / {{\n\t\t\tproxy_pass http://127.0.0.1:{};\n", cfg.api_port));
    s.push_str("\t\t\tproxy_http_version 1.1;\n\t\t\tproxy_set_header Host $http_host;\n\t\t\tproxy_set_header Upgrade $http_upgrade;\n\t\t\tproxy_set_header Connection $connection_upgrade;\n");
    s.push_str("\t\t\tproxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n\t\t\tproxy_set_header X-Forwarded-Proto $scheme;\n");
    s.push_str("\t\t\tproxy_read_timeout 300s;\n\t\t\tproxy_buffering off;\n\t\t}\n\t}\n\n");

    for site in &site_list {
        s.push_str(&format!("\t# {}  ({})\n\tserver {{\n", site.name, site.path.display()));
        s.push_str(&listens(http, https, ""));
        s.push_str(&format!("\t\tserver_name {};\n", web::site_hosts(cfg, site).join(" ")));
        s.push_str(&format!("\t\troot {};\n\t\tindex index.php index.html index.htm;\n", q(&paths::fwd(&site.root))));
        // A JSON access log per site: the daemon reads it for the dashboard's request numbers.
        s.push_str(&format!("\t\taccess_log {} tez_json;\n", q(&paths::fwd(&crate::stats::access_log(&site.name)))));
        if !php::is_installed(&site.php) {
            s.push_str(&format!("\t\tlocation / {{\n\t\t\tdefault_type text/plain;\n\t\t\treturn 503 \"tez: PHP {} is not installed. Run: tez php install {}\\n\";\n\t\t}}\n\t}}\n\n", site.php, site.php));
            continue;
        }
        s.push_str("\t\tlocation / {\n\t\t\ttry_files $uri $uri/ /index.php?$query_string;\n\t\t}\n");
        s.push_str("\t\tlocation ~ /\\.(?!well-known) {\n\t\t\tdeny all;\n\t\t}\n");
        s.push_str("\t\tlocation ~ [^/]\\.php(/|$) {\n");
        s.push_str("\t\t\tfastcgi_split_path_info ^(.+?\\.php)(/.*)$;\n");
        // try_files empties $fastcgi_path_info, so keep it first.
        s.push_str("\t\t\tset $tez_path_info $fastcgi_path_info;\n\t\t\ttry_files $fastcgi_script_name =404;\n");
        s.push_str("\t\t\tinclude fastcgi_params;\n");
        s.push_str("\t\t\tfastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;\n\t\t\tfastcgi_param PATH_INFO $tez_path_info;\n");
        s.push_str(&format!("\t\t\tfastcgi_pass {};\n\t\t}}\n\t}}\n\n", upstream(&site.php)));
    }
    s.push_str("}\n");
    s
}

fn write_if_changed(path: &std::path::Path, text: &str) -> R<bool> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(text) { return Ok(false); }
    std::fs::write(path, text)?;
    Ok(true)
}

/// Write nginx.conf and everything it refers to: the site certificate, mime.types,
/// fastcgi_params and the prefix folders. True when anything changed.
pub fn write_conf(cfg: &Config) -> R<bool> {
    let (http, https, _) = web::effective_ports(cfg);
    std::fs::create_dir_all(conf_dir())?;
    // The access logs live with every other log; Nginx will not start if it cannot open them.
    std::fs::create_dir_all(paths::logs_dir())?;
    for d in ["logs", "temp/client_body", "temp/proxy", "temp/fastcgi", "temp/uwsgi", "temp/scgi"] {
        std::fs::create_dir_all(paths::join_rel(&prefix(), d))?;
    }
    let before = std::fs::read(conf_dir().join("tls").join("sites.crt")).unwrap_or_default();
    let (crt, key) = tls::ensure_leaf(&conf_dir().join("tls"), &all_hosts(cfg))?;
    let mut changed = std::fs::read(&crt).unwrap_or_default() != before;
    changed |= write_if_changed(&conf_dir().join("mime.types"), MIME_TYPES)?;
    changed |= write_if_changed(&conf_dir().join("fastcgi_params"), FASTCGI_PARAMS)?;
    changed |= write_if_changed(&conf(), &conf_text(cfg, http, https, &paths::fwd(&crt), &paths::fwd(&key)))?;
    Ok(changed)
}

/// `-p <prefix>/ -c <conf>`: the arguments every nginx command needs.
fn base_args() -> Vec<String> {
    vec!["-p".into(), format!("{}/", paths::fwd(&prefix())), "-c".into(), paths::fwd(&conf())]
}

pub fn spec(cfg: &Config) -> Spec {
    let (http, https, _) = web::effective_ports(cfg);
    let mut stop = base_args();
    stop.extend(["-s".into(), "quit".into()]);
    Spec::new("web", "web", exe())
        .args(base_args())
        .args(["-g".to_string(), "daemon off;".into()])
        .cwd(prefix())
        .port(https)
        .url(format!("http://127.0.0.1:{http}"))
        .stop_with(exe(), stop)
}

fn signal(what: &str) -> R<()> {
    let out = Command::new(exe()).args(base_args()).args(["-s", what]).current_dir(prefix())
        .hidden().stdin(Stdio::null()).output()?;
    if !out.status.success() { crate::bail!("nginx -s {what} failed: {}", String::from_utf8_lossy(&out.stderr).trim()); }
    Ok(())
}

/// Apply a changed nginx.conf (or certificate) without dropping connections. The new config is
/// tested first, so a mistake leaves the running server untouched.
pub fn reload() -> R<()> {
    let test = Command::new(exe()).args(base_args()).arg("-t").current_dir(prefix())
        .hidden().stdin(Stdio::null()).output()?;
    if !test.status.success() { crate::bail!("nginx config test failed: {}", String::from_utf8_lossy(&test.stderr).trim()); }
    signal("reload")
}

/// The ports a running tez Nginx was started with, read from its own nginx.conf.
pub fn running_ports() -> Option<(u16, u16)> {
    let pid: u32 = std::fs::read_to_string(pid_file()).ok()?.trim().parse().ok()?;
    if !sys::pid_alive(pid) { return None; }
    let text = std::fs::read_to_string(conf()).ok()?;
    let line = text.lines().find_map(|l| l.strip_prefix("# tez ports: "))?;
    let v: Vec<u16> = line.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    let (http, https) = (*v.first()?, *v.get(1)?);
    // A stale pid file can name another program: our server holds its HTTPS port.
    if !util::port_open(https) { return None; }
    Some((http, https))
}

/// Nginx never rolls its logs: past `max` bytes a site's access log moves to `<file>.1` and
/// Nginx reopens its files.
pub fn rotate_logs(cfg: &Config, max: u64) {
    let mut moved = false;
    for site in sites::all(cfg) {
        let log = crate::stats::access_log(&site.name);
        if std::fs::metadata(&log).map(|m| m.len() <= max).unwrap_or(true) { continue; }
        let old = log.with_extension("log.1");
        let _ = std::fs::remove_file(&old);
        moved |= std::fs::rename(&log, &old).is_ok();
    }
    if moved { let _ = signal("reopen"); }
}

pub const FASTCGI_PARAMS: &str = "# Written by tez.
fastcgi_param  QUERY_STRING       $query_string;
fastcgi_param  REQUEST_METHOD     $request_method;
fastcgi_param  CONTENT_TYPE       $content_type;
fastcgi_param  CONTENT_LENGTH     $content_length;
fastcgi_param  SCRIPT_NAME        $fastcgi_script_name;
fastcgi_param  REQUEST_URI        $request_uri;
fastcgi_param  DOCUMENT_URI       $document_uri;
fastcgi_param  DOCUMENT_ROOT      $document_root;
fastcgi_param  SERVER_PROTOCOL    $server_protocol;
fastcgi_param  REQUEST_SCHEME     $scheme;
fastcgi_param  HTTPS              $https if_not_empty;
fastcgi_param  GATEWAY_INTERFACE  CGI/1.1;
fastcgi_param  SERVER_SOFTWARE    nginx/$nginx_version;
fastcgi_param  REMOTE_ADDR        $remote_addr;
fastcgi_param  REMOTE_PORT        $remote_port;
fastcgi_param  SERVER_ADDR        $server_addr;
fastcgi_param  SERVER_PORT        $server_port;
fastcgi_param  SERVER_NAME        $host;
fastcgi_param  REDIRECT_STATUS    200;
# No Proxy header reaches PHP as HTTP_PROXY (httpoxy).
fastcgi_param  HTTP_PROXY         \"\";
";

pub const MIME_TYPES: &str = "# Written by tez.
types {
    text/html                                        html htm shtml;
    text/css                                         css;
    text/xml                                         xml;
    text/plain                                       txt log;
    text/markdown                                    md;
    text/csv                                         csv;
    text/javascript                                  js mjs;
    application/json                                 json map;
    application/manifest+json                        webmanifest;
    application/rss+xml                              rss;
    application/atom+xml                             atom;
    application/wasm                                 wasm;
    application/pdf                                  pdf;
    application/zip                                  zip;
    application/gzip                                 gz;
    application/x-tar                                tar;
    application/octet-stream                         bin exe dll iso dmg msi;
    application/vnd.openxmlformats-officedocument.spreadsheetml.sheet      xlsx;
    application/vnd.openxmlformats-officedocument.wordprocessingml.document docx;
    application/vnd.ms-excel                         xls;
    application/msword                               doc;
    image/png                                        png;
    image/jpeg                                       jpeg jpg;
    image/gif                                        gif;
    image/webp                                       webp;
    image/avif                                       avif;
    image/svg+xml                                    svg svgz;
    image/x-icon                                     ico;
    image/bmp                                        bmp;
    image/tiff                                       tif tiff;
    font/woff                                        woff;
    font/woff2                                       woff2;
    font/ttf                                         ttf;
    font/otf                                         otf;
    application/vnd.ms-fontobject                    eot;
    audio/mpeg                                       mp3;
    audio/ogg                                        ogg;
    audio/wav                                        wav;
    audio/webm                                       weba;
    video/mp4                                        mp4 m4v;
    video/webm                                       webm;
    video/quicktime                                  mov;
}
";

#[cfg(test)]
mod tests {
    use super::*;

    /// The log line Nginx writes, with LOG_FORMAT's variables filled in, parses like Caddy's.
    #[test]
    fn access_log_matches_stats() {
        let line = LOG_FORMAT.trim_matches('\'')
            .replace("$msec", "1795330000.250")
            .replace("$remote_addr", "127.0.0.1")
            .replace("$server_protocol", "HTTP/2.0")
            .replace("$request_method", "GET")
            .replace("$host", "acmeshop.localhost")
            .replace("$request_uri", "/login?x=1")
            .replace("$request_time", "0.012")
            .replace("$bytes_sent", "5120")
            .replace("$status", "200");
        assert!(!line.contains('$'), "a variable was left unfilled: {line}");
        let (ts, m, u, s, ms) = crate::stats::parse_access(&line).unwrap();
        assert_eq!((m.as_str(), u.as_str(), s), ("GET", "/login?x=1", 200));
        assert!((ms - 12.0).abs() < 0.01);
        assert!((ts - 1795330000.25).abs() < 0.001);
        // A connection that sent no request: Nginx logs a 400 with empty fields; not a request.
        let empty = r#"{"ts":1795337320.109,"request":{"remote_ip":"127.0.0.1","proto":"","method":"","host":"a.test","uri":""},"duration":1.008,"size":0,"status":400}"#;
        assert!(crate::stats::parse_access(empty).is_none());
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["request"]["host"], "acmeshop.localhost");
    }

    #[test]
    fn quoting() {
        assert_eq!(q("C:\\Users\\me\\My Sites\\app"), "\"C:/Users/me/My Sites/app\"");
        assert_eq!(upstream("8.4"), "php_8_4");
        let l = listens(80, 443, "");
        assert!(l.contains("listen 127.0.0.1:443 ssl;") && l.contains("listen [::1]:80;") && l.contains("http2 on;"));
    }
}
