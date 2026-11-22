//! Downloading and unpacking with the system's own curl and tar: the copies inside Windows
//! (curl.exe, bsdtar), or curl, tar and unzip on macOS and Linux. No HTTP or archive libraries
//! are compiled into tez.

use crate::paths;
use crate::sys::Hidden;
use crate::util::R;
use crate::bail;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const UA: &str = "tez/0.1 (+https://github.com/boparaiamrit/tez)";

fn curl() -> Command {
    let mut c = Command::new(paths::tool("curl"));
    // The daemon has no console: without this every request would flash a window.
    c.stdin(Stdio::null()).hidden();
    c
}

/// GitHub's API allows 60 anonymous calls an hour; CI runners and gh users have a token.
fn github_auth(cmd: &mut Command, url: &str) {
    if !url.starts_with("https://api.github.com/") { return; }
    for var in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(t) = std::env::var(var) {
            if !t.trim().is_empty() { cmd.arg("-H").arg(format!("Authorization: Bearer {}", t.trim())); return; }
        }
    }
}

/// The user agent to send: tez's own, except to Oracle's MySQL downloads, which answer 403 to
/// anything but curl's default.
fn ua(url: &str) -> Vec<&'static str> {
    if url.contains("mysql.com/") { Vec::new() } else { vec!["-A", UA] }
}

/// GET a URL and return the body as text.
pub fn get_text(url: &str) -> R<String> {
    let mut cmd = curl();
    cmd.args(["-fsSL", "--retry", "2", "--connect-timeout", "15"]).args(ua(url));
    github_auth(&mut cmd, url);
    let out = cmd.arg(url).output()?;
    if !out.status.success() {
        bail!("download failed ({}): {url}", out.status.code().unwrap_or(-1));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// GET with a few retries and a short pause between them, for listings fetched in parallel.
pub fn get_text_retry(url: &str, tries: u32) -> R<String> {
    let mut last = None;
    for i in 0..tries.max(1) {
        match get_text(url) { Ok(t) => return Ok(t), Err(e) => last = Some(e) }
        std::thread::sleep(std::time::Duration::from_millis(400 * (i as u64 + 1)));
    }
    Err(last.unwrap_or_else(|| "no attempt made".into()))
}

/// True when the URL answers 200 to a HEAD request (after redirects).
pub fn exists(url: &str) -> bool {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    curl()
        .args(["-fsIL", "-o", null, "--connect-timeout", "10"]).args(ua(url)).arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Never let tez fill the disk: the OS, WSL and databases misbehave when the disk hits zero.
pub const MIN_FREE: u64 = 2 << 30;

pub fn ensure_space(near: &Path, need: u64) -> R<()> {
    let free = crate::sys::free_bytes(near);
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
    let mut cmd = curl();
    cmd.args(["-fL", "--retry", "3", "--connect-timeout", "20"]).args(ua(url)).arg("-o").arg(&part).arg(url);
    if std::io::stderr().is_terminal() { cmd.arg("-#"); } else { cmd.arg("-s"); }
    let status = cmd.status()?;
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        bail!("download failed: {url}");
    }
    std::fs::rename(&part, &dest)?;
    Ok(dest)
}

/// SHA-256 of a file: certutil on Windows, shasum on macOS, sha256sum on Linux.
pub fn sha256(path: &Path) -> R<String> {
    let out = if cfg!(windows) {
        Command::new(paths::tool("certutil")).arg("-hashfile").arg(path).arg("SHA256").output()?
    } else if cfg!(target_os = "macos") {
        Command::new("shasum").args(["-a", "256"]).arg(path).output()?
    } else {
        Command::new("sha256sum").arg(path).output()?
    };
    let text = String::from_utf8_lossy(&out.stdout);
    // certutil prints a header line first; shasum/sha256sum print "<hash>  <file>".
    for line in text.lines().skip(if cfg!(windows) { 1 } else { 0 }) {
        let first = line.split_whitespace().next().unwrap_or_default();
        let h: String = if cfg!(windows) { line.chars().filter(|c| c.is_ascii_hexdigit()).collect() } else { first.to_string() };
        if h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()) { return Ok(h.to_ascii_lowercase()); }
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

/// Unpack an archive (zip, tar, tgz, txz, ...) into `dest`. `excludes` are tar patterns.
/// Windows' bsdtar reads zips too; on macOS and Linux zips go through unzip.
pub fn extract(archive: &Path, dest: &Path, excludes: &[&str]) -> R<()> {
    // Unpacked archives are usually 2 to 4 times their compressed size.
    let size = archive.metadata().map(|m| m.len()).unwrap_or(0);
    ensure_space(dest, size.saturating_mul(3))?;
    std::fs::create_dir_all(dest)?;
    let is_zip = archive.extension().map(|e| e.eq_ignore_ascii_case("zip")).unwrap_or(false);
    let mut cmd = if is_zip && !cfg!(windows) {
        let mut c = Command::new("unzip");
        c.arg("-q").arg("-o").arg(archive).arg("-d").arg(dest);
        if !excludes.is_empty() { c.arg("-x").args(excludes); }
        c
    } else {
        let mut c = Command::new(paths::tool("tar"));
        c.arg("-xf").arg(archive).arg("-C").arg(dest);
        for e in excludes { c.arg("--exclude").arg(e); }
        c
    };
    let out = cmd.stdin(Stdio::null()).output()
        .map_err(|e| format!("could not run {} to unpack {}: {e}", if is_zip && !cfg!(windows) { "unzip" } else { "tar" }, archive.display()))?;
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

/// Make a downloaded program runnable (macOS and Linux; nothing to do on Windows).
pub fn make_executable(path: &Path) -> R<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    let _ = path;
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

/// Latest release tag of a GitHub repository.
pub fn github_latest_tag(repo: &str) -> R<String> {
    let body = get_text(&format!("https://api.github.com/repos/{repo}/releases/latest"))?;
    let v: serde_json::Value = serde_json::from_str(&body)?;
    match v["tag_name"].as_str() {
        Some(t) => Ok(t.to_string()),
        None => bail!("no release found for {repo}"),
    }
}

/// Every release tag of a GitHub repository, newest first (one page of 100).
pub fn github_tags(repo: &str) -> R<Vec<String>> {
    let body = get_text(&format!("https://api.github.com/repos/{repo}/releases?per_page=100"))?;
    let v: serde_json::Value = serde_json::from_str(&body)?;
    Ok(v.as_array().into_iter().flatten()
        .filter(|r| !r["prerelease"].as_bool().unwrap_or(false) && !r["draft"].as_bool().unwrap_or(false))
        .filter_map(|r| r["tag_name"].as_str().map(|s| s.to_string()))
        .collect())
}

/// One GitHub release: its tag, whether it is a pre-release, and its asset file names.
pub struct GhRelease { pub tag: String, pub prerelease: bool, pub assets: Vec<String> }

/// Releases of a GitHub repository, newest first, up to `pages` pages of 100 (drafts left out).
pub fn github_releases(repo: &str, pages: u32) -> R<Vec<GhRelease>> {
    let mut out = Vec::new();
    for page in 1..=pages.max(1) {
        let body = get_text(&format!("https://api.github.com/repos/{repo}/releases?per_page=100&page={page}"))?;
        let v: serde_json::Value = serde_json::from_str(&body)?;
        let list = v.as_array().cloned().unwrap_or_default();
        let n = list.len();
        for r in list {
            if r["draft"].as_bool().unwrap_or(false) { continue; }
            let Some(tag) = r["tag_name"].as_str() else { continue };
            out.push(GhRelease {
                tag: tag.to_string(),
                prerelease: r["prerelease"].as_bool().unwrap_or(false),
                assets: r["assets"].as_array().into_iter().flatten().filter_map(|a| a["name"].as_str().map(|s| s.to_string())).collect(),
            });
        }
        if n < 100 { break; }
    }
    Ok(out)
}

/// Run `f` over `items` on up to `threads` threads; results keep the input order. Used for the
/// many small HEAD requests behind the version catalog.
pub fn par_map<T: Sync, U: Send>(items: &[T], threads: usize, f: impl Fn(&T) -> U + Sync) -> Vec<U> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<U>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..threads.max(1).min(items.len().max(1)) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= items.len() { break; }
                let r = f(&items[i]);
                slots.lock().unwrap()[i] = Some(r);
            });
        }
    });
    slots.into_inner().unwrap().into_iter().map(|o| o.expect("every item mapped")).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn par_map_keeps_order() {
        let v: Vec<u32> = (0..50).collect();
        assert_eq!(super::par_map(&v, 8, |x| x * 2), (0..50).map(|x| x * 2).collect::<Vec<_>>());
    }

    #[test]
    fn finds_hrefs() {
        let h = r#"<a href="6.2.0/">6.2.0</a> <a href="6.3.0/">6.3.0</a>"#;
        assert_eq!(super::hrefs(h), vec!["6.2.0/", "6.3.0/"]);
    }
}
