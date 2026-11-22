//! A tiny client for the tez daemon's API on 127.0.0.1 (no HTTP library).

use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub fn port() -> u16 {
    let home = std::env::var("TEZ_HOME").ok().filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}\\.tez", std::env::var("USERPROFILE").unwrap_or_default()));
    std::fs::read_to_string(format!("{home}\\config\\tez.json")).ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["api_port"].as_u64())
        .map(|p| p as u16)
        .unwrap_or(7979)
}

fn request(method: &str, path: &str, body: Option<&Value>, timeout: Duration) -> Result<Value, String> {
    let port = port();
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
    let v: Value = serde_json::from_str(body).unwrap_or(Value::String(body.to_string()));
    if !head.starts_with("HTTP/1.1 200") {
        return Err(v["error"].as_str().map(|s| s.to_string()).unwrap_or_else(|| body.trim().to_string()));
    }
    Ok(v)
}

pub fn get(path: &str) -> Result<Value, String> { request("GET", path, None, Duration::from_secs(20)) }
pub fn post(path: &str, body: &Value) -> Result<Value, String> { request("POST", path, Some(body), Duration::from_secs(120)) }

/// Start the daemon through the CLI that sits beside this app.
pub fn start_daemon() {
    use std::os::windows::process::CommandExt;
    let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("tez.exe")));
    if let Some(exe) = exe {
        let _ = std::process::Command::new(exe).arg("start").creation_flags(0x0800_0000).spawn();
    }
}

pub fn open_url(url: &str) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("cmd.exe").args(["/c", "start", "", url]).creation_flags(0x0800_0000).spawn();
}
