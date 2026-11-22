//! Sharing a site on the internet through a tunnel (`tez share`). Three ways, all with open-source
//! software on this computer:
//!
//! * `cloudflare`: a Cloudflare quick tunnel through `cloudflared` (Apache-2.0). No account; a
//!   random https://….trycloudflare.com address that lasts while the share runs.
//! * `ssh`: the OpenSSH client the system already has, to localhost.run (no account) or to your
//!   own sish server.
//! * `frp`: `frpc` (Apache-2.0) to your own frps server, on your own domain.
//!
//! Each share gets its own port on 127.0.0.1 where the web server serves that one site whatever
//! the host name, telling PHP the request is HTTPS, so Laravel builds links with the public
//! address. The tunnel runs as a daemon process. Shares are never kept across a daemon restart,
//! so a site is never left public by accident.

use crate::config::{Config, TunnelConfig};
use crate::fetch;
use crate::paths;
use crate::sites;
use crate::spec::Spec;
use crate::sys;
use crate::util::R;
use crate::bail;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const PROVIDERS: &[(&str, &str)] = &[
    ("cloudflare", "Cloudflare quick tunnel: no account, a random trycloudflare.com address"),
    ("ssh", "SSH: localhost.run with no account, or your own sish server"),
    ("frp", "frp: your own server and domain"),
];

/// One running share.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Share {
    pub site: String,
    pub provider: String,
    /// The local port the web server serves this site on for the tunnel.
    pub port: u16,
    /// Cloudflare only: let in just these email addresses or @domains (a one-time PIN by email).
    #[serde(default)]
    pub emails: Vec<String>,
    pub started: String,
}

fn file() -> PathBuf { paths::run_dir().join("shares.json") }

/// The shares running now.
pub fn active() -> Vec<Share> {
    std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(list: &[Share]) -> R<()> {
    std::fs::create_dir_all(paths::run_dir())?;
    std::fs::write(file(), serde_json::to_string_pretty(list)?)?;
    Ok(())
}

/// A new daemon starts with nothing shared.
pub fn clear_all() { let _ = std::fs::remove_file(file()); }

/// The first free port from 7100 that no other share holds.
fn free_port(taken: &[u16]) -> R<u16> {
    for port in 7100..7300u16 {
        if taken.contains(&port) { continue; }
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() { return Ok(port); }
    }
    bail!("no free local port between 7100 and 7300 for the share")
}

/// Start sharing `site` (or change how). Fetches the tunnel program the first time.
pub fn start(cfg: &Config, site: &str, provider: Option<&str>, emails: Vec<String>) -> R<Share> {
    if sites::find(cfg, site).is_none() { bail!("no site named {site}"); }
    let provider = provider.filter(|p| !p.is_empty()).unwrap_or(&cfg.tunnel.provider).to_ascii_lowercase();
    if !PROVIDERS.iter().any(|(p, _)| *p == provider) { bail!("'{provider}' is not a way to share; use cloudflare, ssh or frp"); }
    let emails: Vec<String> = emails.into_iter().map(|e| e.trim().to_ascii_lowercase()).filter(|e| !e.is_empty()).collect();
    if !emails.is_empty() && provider != "cloudflare" { bail!("only a Cloudflare share can be limited to email addresses"); }
    for e in &emails {
        if !e.contains('@') || e.contains([' ', ',', ';', '"', '\'']) { bail!("'{e}' is not an email address or @domain"); }
    }
    match provider.as_str() {
        "cloudflare" => { cloudflared()?; }
        "frp" => {
            check_frp(&cfg.tunnel)?;
            frpc()?;
        }
        _ => { if !ssh_exe().exists() && cfg!(windows) { bail!("the OpenSSH client is missing; add it in Windows Settings > System > Optional features"); } }
    }
    let mut list = active();
    let keep_port = list.iter().find(|s| s.site == site).map(|s| s.port);
    list.retain(|s| s.site != site);
    let taken: Vec<u16> = list.iter().map(|s| s.port).collect();
    let port = match keep_port { Some(p) => p, None => free_port(&taken)? };
    let share = Share { site: site.to_string(), provider, port, emails, started: crate::util::utc_now() };
    list.push(share.clone());
    save(&list)?;
    Ok(share)
}

/// Stop sharing `site`. True if it was shared.
pub fn stop(site: &str) -> R<bool> {
    let mut list = active();
    let before = list.len();
    list.retain(|s| s.site != site);
    save(&list)?;
    Ok(list.len() != before)
}

/// Whether PHP should see these requests as HTTPS (the public address is https).
pub fn https(cfg: &Config, share: &Share) -> bool { share.provider != "frp" || cfg.tunnel.frp_https }

// ------------------------------------------------------------------ the tunnel programs

fn tools_dir(name: &str) -> PathBuf { paths::home().join("tools").join(name) }

pub fn cloudflared_exe() -> PathBuf { tools_dir("cloudflared").join(sys::exe("cloudflared")) }
pub fn frpc_exe() -> PathBuf { tools_dir("frp").join(sys::exe("frpc")) }

fn ssh_exe() -> PathBuf {
    if cfg!(windows) { paths::system32("OpenSSH\\ssh.exe") } else { paths::tool("ssh") }
}

/// The newest release of a GitHub project: its tag and, for the asset named `want`, the download
/// address and SHA-256 (GitHub publishes a digest for each asset).
fn latest_asset(repo: &str, want: impl Fn(&str) -> bool) -> R<(String, String, Option<String>, String)> {
    let body = fetch::get_text_retry(&format!("https://api.github.com/repos/{repo}/releases/latest"), 3)?;
    let v: serde_json::Value = serde_json::from_str(&body)?;
    let tag = v["tag_name"].as_str().unwrap_or_default().to_string();
    for a in v["assets"].as_array().into_iter().flatten() {
        let name = a["name"].as_str().unwrap_or_default();
        if want(name) {
            let url = a["browser_download_url"].as_str().unwrap_or_default().to_string();
            let sha = a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|s| s.to_string());
            return Ok((tag, url, sha, name.to_string()));
        }
    }
    bail!("{repo} {tag} has no build for this computer ({} {})", sys::os(), sys::arch())
}

fn arch_word() -> &'static str { if sys::arch() == "aarch64" { "arm64" } else { "amd64" } }

/// cloudflared, downloaded the first time and checked against GitHub's digest.
pub fn cloudflared() -> R<PathBuf> {
    let exe = cloudflared_exe();
    if exe.exists() { return Ok(exe); }
    let want = match sys::os() {
        "windows" => format!("cloudflared-windows-{}.exe", arch_word()),
        "macos" => format!("cloudflared-darwin-{}.tgz", arch_word()),
        _ => format!("cloudflared-linux-{}", arch_word()),
    };
    let (tag, url, sha, name) = latest_asset("cloudflare/cloudflared", |n| n == want)?;
    crate::util::step(&format!("Downloading cloudflared {tag}"));
    let file = fetch::download(&url, &format!("{tag}-{name}"))?;
    if let Some(sha) = sha { if let Err(e) = fetch::verify_sha256(&file, &sha) { let _ = std::fs::remove_file(&file); return Err(e); } }
    let dir = tools_dir("cloudflared");
    std::fs::create_dir_all(&dir)?;
    if name.ends_with(".tgz") { fetch::extract(&file, &dir, &[])?; } else { std::fs::copy(&file, &exe)?; }
    fetch::make_executable(&exe)?;
    if !exe.exists() { bail!("cloudflared did not unpack to {}", exe.display()); }
    Ok(exe)
}

/// frpc from frp's newest release, checked against GitHub's digest.
pub fn frpc() -> R<PathBuf> {
    let exe = frpc_exe();
    if exe.exists() { return Ok(exe); }
    let os = match sys::os() { "windows" => "windows", "macos" => "darwin", _ => "linux" };
    let suffix = format!("_{os}_{}.{}", arch_word(), if os == "windows" { "zip" } else { "tar.gz" });
    let (tag, url, sha, name) = latest_asset("fatedier/frp", |n| n.starts_with("frp_") && n.ends_with(&suffix))?;
    crate::util::step(&format!("Downloading frp {tag}"));
    let file = fetch::download(&url, &name)?;
    if let Some(sha) = sha { if let Err(e) = fetch::verify_sha256(&file, &sha) { let _ = std::fs::remove_file(&file); return Err(e); } }
    let dir = tools_dir("frp");
    let _ = std::fs::remove_dir_all(&dir);
    fetch::extract_flat(&file, &dir, &[])?;
    fetch::make_executable(&exe)?;
    if !exe.exists() { bail!("frpc did not unpack to {}", exe.display()); }
    Ok(exe)
}

fn check_frp(t: &TunnelConfig) -> R<()> {
    if t.frp_server.trim().is_empty() { bail!("set your frp server first (Settings > Sharing, or tez share setup frp)"); }
    if t.frp_domain.trim().is_empty() { bail!("set the domain your frp server serves sites on (Settings > Sharing)"); }
    Ok(())
}

// ------------------------------------------------------------------ processes and addresses

pub fn proc_name(site: &str) -> String { format!("share:{site}") }

/// The daemon process for each share.
pub fn specs(cfg: &Config) -> Vec<Spec> {
    active().iter().filter_map(|s| spec(cfg, s).ok()).collect()
}

fn spec(cfg: &Config, s: &Share) -> R<Spec> {
    let local = format!("127.0.0.1:{}", s.port);
    let t = &cfg.tunnel;
    let spec = match s.provider.as_str() {
        "cloudflare" => {
            let mut args = vec!["tunnel".to_string(), "--no-autoupdate".into(), "--url".into(), format!("http://{local}")];
            for e in &s.emails { args.push("--allowed-mail".into()); args.push(e.clone()); }
            Spec::new(proc_name(&s.site), "share", cloudflared_exe()).args(args)
        }
        "ssh" => {
            let host = if t.ssh_host.trim().is_empty() { "localhost.run".to_string() } else { t.ssh_host.trim().to_string() };
            let user = if t.ssh_user.trim().is_empty() { "nokey".to_string() } else { t.ssh_user.trim().to_string() };
            let remote = if t.ssh_subdomain.trim().is_empty() { "80".to_string() } else { format!("{}:80", t.ssh_subdomain.trim()) };
            let known = paths::run_dir().join("ssh_known_hosts");
            Spec::new(proc_name(&s.site), "share", ssh_exe()).args([
                "-T".to_string(), "-o".into(), "StrictHostKeyChecking=accept-new".into(), "-o".into(), format!("UserKnownHostsFile={}", known.display()),
                "-o".into(), "ServerAliveInterval=30".into(), "-o".into(), "ServerAliveCountMax=3".into(), "-o".into(), "ExitOnForwardFailure=yes".into(),
                "-o".into(), "BatchMode=yes".into(), "-p".into(), t.ssh_port.max(1).to_string(),
                "-R".into(), format!("{remote}:{local}"), format!("{user}@{host}"),
            ])
        }
        "frp" => {
            check_frp(t)?;
            let conf = paths::run_dir().join(format!("frpc-{}.toml", s.site));
            std::fs::write(&conf, frpc_toml(t, s))?;
            Spec::new(proc_name(&s.site), "share", frpc_exe()).args(["-c".to_string(), conf.display().to_string()])
        }
        other => bail!("unknown share provider {other}"),
    };
    Ok(spec.url(url_hint(cfg, s).unwrap_or_default()))
}

fn toml_str(s: &str) -> String { format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")) }

fn frp_host(t: &TunnelConfig, site: &str) -> String { format!("{}.{}", crate::util::host_label(site), t.frp_domain.trim().trim_start_matches('.')) }

pub fn frpc_toml(t: &TunnelConfig, s: &Share) -> String {
    let mut out = String::from("# Written by tez for `tez share`.\n");
    out.push_str(&format!("serverAddr = {}\nserverPort = {}\n", toml_str(t.frp_server.trim()), t.frp_port.max(1)));
    if !t.frp_token.is_empty() { out.push_str(&format!("auth.method = \"token\"\nauth.token = {}\n", toml_str(&t.frp_token))); }
    out.push_str("loginFailExit = false\n\n[[proxies]]\n");
    out.push_str(&format!("name = {}\ntype = \"http\"\nlocalIP = \"127.0.0.1\"\nlocalPort = {}\n", toml_str(&format!("tez-{}", s.site)), s.port));
    out.push_str(&format!("customDomains = [{}]\n", toml_str(&frp_host(t, &s.site))));
    out
}

/// The address known without reading the log (frp's comes from your settings).
fn url_hint(cfg: &Config, s: &Share) -> Option<String> {
    (s.provider == "frp").then(|| format!("{}://{}", if cfg.tunnel.frp_https { "https" } else { "http" }, frp_host(&cfg.tunnel, &s.site)))
}

/// The public address of a share: from settings (frp) or from what the tunnel printed since it
/// last started.
pub fn public_url(cfg: &Config, s: &Share) -> Option<String> {
    if let Some(u) = url_hint(cfg, s) { return Some(u); }
    let text = std::fs::read_to_string(crate::daemon::log_path(&proc_name(&s.site))).ok()?;
    let since = text.rfind("==== tez: starting").map(|i| &text[i..]).unwrap_or(&text);
    url_in(since, &s.provider, &cfg.tunnel)
}

/// The tunnel's address in its output: the last https address that belongs to the service.
pub fn url_in(text: &str, provider: &str, t: &TunnelConfig) -> Option<String> {
    let host_ok = |h: &str| match provider {
        "cloudflare" => h.ends_with(".trycloudflare.com") && h != "api.trycloudflare.com",
        _ => {
            let base = if t.ssh_host.trim().is_empty() || t.ssh_host.trim() == "localhost.run" { None } else { Some(t.ssh_host.trim().to_ascii_lowercase()) };
            match base {
                None => h.ends_with(".lhr.life") || (h.ends_with(".localhost.run") && h != "admin.localhost.run"),
                Some(b) => h.ends_with(&format!(".{b}")) || h == b,
            }
        }
    };
    let mut found = None;
    let mut rest = text;
    while let Some(i) = rest.find("https://") {
        let tail = &rest[i + 8..];
        let end = tail.find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-')).unwrap_or(tail.len());
        let host = tail[..end].trim_end_matches('.').to_ascii_lowercase();
        if !host.is_empty() && host_ok(&host) { found = Some(format!("https://{host}")); }
        rest = &tail[end..];
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_cloudflare_address() {
        let log = "2026-11-22T10:00:00Z INF Requesting new quick Tunnel on trycloudflare.com...\n\
            INF +--------------------------------------------------------------------------------------------+\n\
            INF |  Your quick Tunnel has been created! Visit it at (it may take some time to be reachable):  |\n\
            INF |  https://shaped-winter-garden-example.trycloudflare.com                                    |\n\
            INF Version 2026.11.0 see https://developers.cloudflare.com/cloudflare-one/";
        assert_eq!(url_in(log, "cloudflare", &TunnelConfig::default()).as_deref(), Some("https://shaped-winter-garden-example.trycloudflare.com"));
    }

    #[test]
    fn finds_the_localhost_run_address() {
        let log = "Welcome to localhost.run!\nhttps://admin.localhost.run/ to get a custom domain\n\
            7a2b9c1d.lhr.life tunneled with tls termination, https://7a2b9c1d.lhr.life\n";
        assert_eq!(url_in(log, "ssh", &TunnelConfig::default()).as_deref(), Some("https://7a2b9c1d.lhr.life"));
    }

    #[test]
    fn finds_a_sish_address_on_your_domain() {
        let t = TunnelConfig { ssh_host: "tunnel.example.com".into(), ..TunnelConfig::default() };
        let log = "Starting SSH Forwarding service for http:80. Forwarded connections can be accessed via the following methods:\nHTTPS: https://hello.tunnel.example.com\n";
        assert_eq!(url_in(log, "ssh", &t).as_deref(), Some("https://hello.tunnel.example.com"));
    }

    #[test]
    fn writes_frpc_config() {
        let t = TunnelConfig { frp_server: "frp.example.com".into(), frp_port: 7000, frp_token: "s3\"cret".into(), frp_domain: "dev.example.com".into(), ..TunnelConfig::default() };
        let s = Share { site: "hello".into(), provider: "frp".into(), port: 7100, emails: vec![], started: String::new() };
        let text = frpc_toml(&t, &s);
        assert!(text.contains("serverAddr = \"frp.example.com\""));
        assert!(text.contains("auth.token = \"s3\\\"cret\""));
        assert!(text.contains("localPort = 7100"));
        assert!(text.contains("customDomains = [\"hello.dev.example.com\"]"));
    }
}
