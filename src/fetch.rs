//! Downloading and unpacking, with the curl.exe and tar.exe (bsdtar) that ship inside Windows.
//! No HTTP or archive libraries are compiled into tez.

use crate::paths;
use crate::util::R;
use crate::bail;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const UA: &str = "tez/0.1 (+https://github.com/boparaiamrit/tez)";

/// GET a URL and return the body as text.
pub fn get_text(url: &str) -> R<String> {
    let out = Command::new(paths::system32("curl.exe"))
        .args(["-fsSL", "--retry", "2", "--connect-timeout", "15", "-A", UA, url])
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        bail!("download failed ({}): {url}", out.status.code().unwrap_or(-1));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// True when the URL answers 200 to a HEAD request (after redirects).
pub fn exists(url: &str) -> bool {
    Command::new(paths::system32("curl.exe"))
        .args(["-fsIL", "-o", "NUL", "--connect-timeout", "10", "-A", UA, url])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Never let tez fill the disk: Windows, WSL and databases misbehave when C: hits zero.
pub const MIN_FREE: u64 = 2 << 30;

pub fn ensure_space(near: &Path, need: u64) -> R<()> {
    let free = crate::win::free_bytes(near);
    if free < need.saturating_add(MIN_FREE) {
        bail!(
            "not enough disk space: {} free on the drive holding {}, and this needs about {} plus 2 GB to spare",
            crate::util::human_bytes(free), near.display(), crate::util::human_bytes(need)
        );
    }
    Ok(())
}

/// Download into the cache (reused when present) and return the file's path.
pub fn download(url: &str, file_name: &str) -> R<PathBuf> {
    let dest = paths::cache_dir().join(file_name);
    if dest.exists() && dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(dest);
    }
    ensure_space(&paths::cache_dir(), 0)?;
    std::fs::create_dir_all(paths::cache_dir())?;
    let part = dest.with_extension("part");
    let mut cmd = Command::new(paths::system32("curl.exe"));
    cmd.args(["-fL", "--retry", "3", "--connect-timeout", "20", "-A", UA, "-o"]).arg(&part).arg(url);
    if std::io::stderr().is_terminal() { cmd.arg("-#"); } else { cmd.arg("-s"); }
    let status = cmd.stdin(Stdio::null()).status()?;
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        bail!("download failed: {url}");
    }
    std::fs::rename(&part, &dest)?;
    Ok(dest)
}

/// SHA-256 of a file, with Windows' certutil.
pub fn sha256(path: &Path) -> R<String> {
    let out = Command::new(paths::system32("certutil.exe")).arg("-hashfile").arg(path).arg("SHA256").output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines().skip(1) {
        let h: String = line.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        if h.len() == 64 { return Ok(h.to_ascii_lowercase()); }
    }
    bail!("could not hash {}", path.display())
}

pub fn verify_sha256(path: &Path, expected: &str) -> R<()> {
    let got = sha256(path)?;
    if !got.eq_ignore_ascii_case(expected.trim()) {
        let _ = std::fs::remove_file(path);
        bail!("checksum mismatch for {} (expected {expected}, got {got}); the download was removed", path.display());
    }
    Ok(())
}

/// Unpack an archive (zip, tar, txz, ...) into `dest` with bsdtar. `excludes` are tar patterns.
pub fn extract(archive: &Path, dest: &Path, excludes: &[&str]) -> R<()> {
    // Unpacked archives are usually 2 to 4 times their compressed size.
    let size = archive.metadata().map(|m| m.len()).unwrap_or(0);
    ensure_space(dest, size.saturating_mul(3))?;
    std::fs::create_dir_all(dest)?;
    let mut cmd = Command::new(paths::system32("tar.exe"));
    cmd.arg("-xf").arg(archive).arg("-C").arg(dest);
    for e in excludes { cmd.arg("--exclude").arg(e); }
    let out = cmd.stdin(Stdio::null()).output()?;
    if !out.status.success() {
        bail!("could not unpack {}: {}", archive.display(), String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

/// Unpack, and if the archive holds a single top folder, lift its contents into `dest`.
pub fn extract_flat(archive: &Path, dest: &Path, excludes: &[&str]) -> R<()> {
    let tmp = dest.with_extension("unpack");
    let _ = std::fs::remove_dir_all(&tmp);
    extract(archive, &tmp, excludes)?;
    let entries: Vec<_> = std::fs::read_dir(&tmp)?.filter_map(|e| e.ok()).collect();
    let src = if entries.len() == 1 && entries[0].path().is_dir() { entries[0].path() } else { tmp.clone() };
    if dest.exists() { std::fs::remove_dir_all(dest)?; }
    if let Some(parent) = dest.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::rename(&src, dest)?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

/// Every `href="..."` target in an HTML directory listing.
pub fn hrefs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("href=\"") {
        rest = &rest[i + 6..];
        if let Some(j) = rest.find('"') {
            out.push(rest[..j].to_string());
            rest = &rest[j..];
        }
    }
    out
}

/// Latest release tag of a GitHub repository (unauthenticated API).
pub fn github_latest_tag(repo: &str) -> R<String> {
    let body = get_text(&format!("https://api.github.com/repos/{repo}/releases/latest"))?;
    let v: serde_json::Value = serde_json::from_str(&body)?;
    match v["tag_name"].as_str() {
        Some(t) => Ok(t.to_string()),
        None => bail!("no release found for {repo}"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_hrefs() {
        let h = r#"<a href="6.2.0/">6.2.0</a> <a href="6.3.0/">6.3.0</a>"#;
        assert_eq!(super::hrefs(h), vec!["6.2.0/", "6.3.0/"]);
    }
}
