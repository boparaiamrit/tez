//! A tiny client for the tez daemon's API on 127.0.0.1 (no HTTP library), and the few things
//! the app asks the operating system to do: open a URL, a folder, a terminal or an editor.

use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// tez's home folder: TEZ_HOME, else ~/.tez.
pub fn home() -> PathBuf {
    if let Some(h) = std::env::var_os("TEZ_HOME").filter(|s| !s.is_empty()) { return PathBuf::from(h); }
    let user = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default();
    PathBuf::from(user).join(".tez")
}

pub fn port() -> u16 {
    std::fs::read_to_string(home().join("config").join("tez.json")).ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["api_port"].as_u64())
        .map(|p| p as u16)
        .unwrap_or(7979)
}

fn raw(port: u16, method: &str, path: &str, body: Option<&Value>, timeout: Duration) -> Result<(bool, String), String> {
    let mut s = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(500))
        .map_err(|_| "tez is not running".to_string())?;
    let _ = s.set_read_timeout(Some(timeout));
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Tez: app\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    s.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    s.write_all(payload.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    // The daemon never uses chunked encoding; Mailpit may, so strip chunk sizes when present.
    let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") { dechunk(body) } else { body.to_string() };
    Ok((head.starts_with("HTTP/1.1 200"), body))
}

fn dechunk(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some((size, after)) = rest.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 || after.len() < n { break; }
        out.push_str(&after[..n]);
        rest = after[n..].trim_start_matches("\r\n");
    }
    out
}

fn request(method: &str, path: &str, body: Option<&Value>, timeout: Duration) -> Result<Value, String> {
    let (ok, body) = raw(port(), method, path, body, timeout)?;
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::String(body.clone()));
    if !ok { return Err(v["error"].as_str().map(|s| s.to_string()).unwrap_or_else(|| body.trim().to_string())); }
    Ok(v)
}

pub fn get(path: &str) -> Result<Value, String> { request("GET", path, None, Duration::from_secs(20)) }
pub fn post(path: &str, body: &Value) -> Result<Value, String> { request("POST", path, Some(body), Duration::from_secs(120)) }
pub fn delete(path: &str) -> Result<Value, String> { request("DELETE", path, None, Duration::from_secs(20)) }

/// A plain-text endpoint (logs, dumps).
pub fn get_text(path: &str) -> Result<String, String> {
    let (ok, body) = raw(port(), "GET", path, None, Duration::from_secs(20))?;
    if ok { Ok(body) } else { Err(body) }
}

/// JSON from another local service (Mailpit's API).
pub fn get_local(port: u16, path: &str) -> Result<Value, String> {
    let (ok, body) = raw(port, "GET", path, None, Duration::from_secs(5))?;
    if !ok { return Err(body); }
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ the operating system

fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

fn exe_name(name: &str) -> String { if cfg!(windows) { format!("{name}.exe") } else { name.to_string() } }

/// The tez command line: beside this app (also inside Tez.app), else ~/.tez/bin.
pub fn tez_exe() -> PathBuf {
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(exe_name("tez"))));
    match beside {
        Some(p) if p.exists() => p,
        _ => home().join("bin").join(exe_name("tez")),
    }
}

pub fn start_daemon() { let _ = quiet(Command::new(tez_exe()).args(["start", "--quiet"])).spawn(); }

pub fn open_url(url: &str) {
    #[cfg(windows)]
    let _ = quiet(Command::new("cmd.exe").args(["/c", "start", "", url])).spawn();
    #[cfg(target_os = "macos")]
    let _ = quiet(Command::new("open").arg(url)).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = quiet(Command::new("xdg-open").arg(url)).spawn();
}

pub fn open_path(path: &str) { open_url(path) }

pub fn open_terminal(dir: &str) {
    #[cfg(windows)]
    {
        if quiet(Command::new("wt.exe").args(["-d", dir])).spawn().is_err() {
            let mut c = Command::new("cmd.exe");
            c.args(["/c", "start", "", "cmd.exe", "/k", "cd", "/d", dir]);
            let _ = quiet(&mut c).spawn();
        }
    }
    #[cfg(target_os = "macos")]
    let _ = quiet(Command::new("open").args(["-a", "Terminal", dir])).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let tries: [(&str, Vec<String>); 4] = [
            ("x-terminal-emulator", vec![format!("--working-directory={dir}")]),
            ("gnome-terminal", vec![format!("--working-directory={dir}")]),
            ("konsole", vec!["--workdir".into(), dir.into()]),
            ("xterm", vec![]),
        ];
        for (prog, args) in tries {
            let mut c = Command::new(prog);
            c.args(&args).current_dir(dir);
            if quiet(&mut c).spawn().is_ok() { break; }
        }
    }
}

pub fn open_editor(dir: &str) {
    #[cfg(windows)]
    let _ = quiet(Command::new("cmd.exe").args(["/c", "code", dir])).spawn();
    #[cfg(not(windows))]
    {
        if quiet(Command::new("code").arg(dir)).spawn().is_err() { open_path(dir); }
    }
}

/// The current git branch of a folder, read straight from .git/HEAD.
pub fn git_branch(dir: &str) -> Option<String> {
    let head = std::fs::read_to_string(Path::new(dir).join(".git").join("HEAD")).ok()?;
    let head = head.trim();
    Some(head.strip_prefix("ref: refs/heads/").map(|b| b.to_string()).unwrap_or_else(|| head.chars().take(7).collect()))
}

/// The app's own preferences (theme), kept beside tez.json.
pub fn app_prefs() -> Value {
    std::fs::read_to_string(home().join("config").join("app.json")).ok()
        .and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

pub fn save_app_prefs(v: &Value) {
    let dir = home().join("config");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("app.json"), serde_json::to_string_pretty(v).unwrap_or_default());
}
