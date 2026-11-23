//! New terminals find tez's `php`, `composer`, `node`, `npm` (and the tools `npm install -g` adds)
//! before any other copy on the computer.
//!
//! Windows reads the computer's PATH before the user's, so a Node.js or PHP installed for the whole
//! computer (C:\Program Files\nodejs, often an nvm link) wins over tez's folder on the user PATH,
//! and changing the computer's PATH needs an administrator. So tez's folders go on the user PATH
//! (for programs started outside a terminal) and each shell also puts them first as it starts:
//! PowerShell 7 and Windows PowerShell from their profile, cmd from its AutoRun, Git Bash from its
//! start-up file. macOS and Linux shells read one line in .zshrc, .bashrc or .profile.

use crate::bail;
use crate::paths;
use crate::sys::Hidden;
use crate::util::R;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Every start-up line tez writes carries this, so it can find and update its own.
const MARK: &str = "# tez";

/// cmd runs this key's AutoRun as it starts.
const CMD_KEY: &str = r"HKCU\Software\Microsoft\Command Processor";
/// Set once tez's folders are first, so a cmd started from a cmd does not add them again.
const CMD_GUARD: &str = "TEZ_BIN_FIRST";

/// The folders that go first: tez's commands, then what `npm install -g` adds.
fn first_dirs() -> Vec<PathBuf> { vec![paths::bin(), crate::node::global_bin()] }

/// Make sure new terminals find tez's commands first. Returns the places that changed.
pub fn ensure() -> R<Vec<String>> {
    let _ = std::fs::create_dir_all(crate::node::global_bin());
    let mut changed = Vec::new();
    if cfg!(windows) {
        if user_path_first(&first_dirs())? { changed.push("your PATH".into()); }
        for (exe, label) in [("pwsh", "PowerShell 7"), ("powershell.exe", "Windows PowerShell")] {
            let Some((profile, allowed)) = powershell_profile(exe) else { continue };
            // Restricted or AllSigned would show an error in every new window instead.
            if !allowed { continue; }
            if let Some(dir) = profile.parent() { std::fs::create_dir_all(dir)?; }
            if upsert(&profile, &powershell_line(), true)? { changed.push(format!("{label} ({})", profile.display())); }
        }
        if cmd_autorun()? { changed.push("cmd".into()); }
        for f in bash_files() {
            if upsert(&f, &sh_line(), false)? { changed.push(format!("Git Bash ({})", f.display())); }
        }
    } else {
        for f in bash_files() {
            if upsert(&f, &sh_line(), false)? { changed.push(f.display().to_string()); }
        }
    }
    Ok(changed)
}

#[derive(Clone, Copy)]
enum Style { PowerShell, Cmd, Sh }

/// A folder as a shell line writes it: under the user's home through $env:USERPROFILE,
/// %USERPROFILE% or $HOME, so the line stays plain ASCII whatever the account is called.
fn for_shell(dir: &Path, style: Style) -> String {
    let rest = dir.strip_prefix(paths::user_home()).ok().map(|r| r.to_string_lossy().to_string());
    match (style, rest) {
        (Style::PowerShell, Some(r)) => format!("$env:USERPROFILE\\{r}"),
        (Style::Cmd, Some(r)) => format!("%USERPROFILE%\\{r}"),
        (Style::Sh, Some(r)) => format!("$HOME/{}", r.replace('\\', "/")),
        (Style::Sh, None) => sh_path(dir),
        (_, None) => dir.display().to_string(),
    }
}

/// C:\tools\tez as Git Bash writes it: /c/tools/tez.
fn sh_path(dir: &Path) -> String {
    let s = dir.display().to_string();
    if !cfg!(windows) { return s; }
    let s = s.replace('\\', "/");
    match s.split_once(":/") {
        Some((drive, rest)) if drive.len() == 1 => format!("/{}/{rest}", drive.to_ascii_lowercase()),
        _ => s,
    }
}

fn powershell_line() -> String {
    let dirs: Vec<String> = first_dirs().iter().map(|d| format!("\"{}\"", for_shell(d, Style::PowerShell))).collect();
    format!("$__tez = {}; $env:Path = (@($__tez) + @($env:Path -split ';' | Where-Object {{ $_ -and $_ -notin $__tez }})) -join ';'; Remove-Variable __tez  {MARK}: its php, node and npm come first",
        dirs.join(", "))
}

fn sh_line() -> String {
    let dirs: Vec<String> = first_dirs().iter().map(|d| for_shell(d, Style::Sh)).collect();
    format!("export PATH=\"{}:$PATH\"  {MARK}", dirs.join(":"))
}

fn cmd_line() -> String {
    let dirs: Vec<String> = first_dirs().iter().map(|d| for_shell(d, Style::Cmd)).collect();
    // The quotes keep a ")" in the PATH (Program Files (x86)) from closing the block.
    format!("if not defined {CMD_GUARD} (set \"{CMD_GUARD}=1\" & set \"PATH={};%PATH%\")", dirs.join(";"))
}

/// One of tez's own lines: it carries the mark and sets the PATH.
fn ours(line: &str) -> bool { line.contains(MARK) && line.to_ascii_lowercase().contains("path") }

/// Put `line` in a start-up file, in place of any older line of tez's. The rest of the file keeps
/// its bytes, line endings and encoding (UTF-8, UTF-8 with a BOM, or UTF-16 as Windows PowerShell
/// writes it). True when the file changed.
fn upsert(file: &Path, line: &str, crlf: bool) -> R<bool> {
    let raw = std::fs::read(file).unwrap_or_default();
    let utf16 = raw.starts_with(&[0xFF, 0xFE]);
    let bom = raw.starts_with(&[0xEF, 0xBB, 0xBF]);
    let text = if utf16 {
        let units: Vec<u16> = raw[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        match std::str::from_utf8(if bom { &raw[3..] } else { &raw }) {
            Ok(t) => t.to_string(),
            // A legacy code page: leave the file as it is and add the (ASCII) line at the end.
            Err(_) => {
                if raw.split(|b| *b == b'\n').any(|l| String::from_utf8_lossy(l).trim_end() == line) { return Ok(false); }
                let nl: &[u8] = if crlf { b"\r\n" } else { b"\n" };
                let mut out = raw.clone();
                if !out.ends_with(b"\n") { out.extend_from_slice(nl); }
                out.extend_from_slice(line.as_bytes());
                out.extend_from_slice(nl);
                std::fs::write(file, out)?;
                return Ok(true);
            }
        }
    };
    let mine: Vec<&str> = text.lines().filter(|l| ours(l)).collect();
    if mine.len() == 1 && mine[0].trim_end() == line { return Ok(false); }
    // Shell scripts get "\n" (a "\r" would end up in the PATH); PowerShell follows the file.
    let nl = if !crlf { "\n" } else if text.is_empty() || text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out: String = text.split_inclusive('\n').filter(|l| !ours(l)).collect();
    if !out.is_empty() && !out.ends_with('\n') { out.push_str(nl); }
    out.push_str(line);
    out.push_str(nl);
    let bytes: Vec<u8> = if utf16 {
        let mut b = vec![0xFF, 0xFE];
        for u in out.encode_utf16() { b.extend_from_slice(&u.to_le_bytes()); }
        b
    } else {
        let mut b = if bom { vec![0xEF, 0xBB, 0xBF] } else { Vec::new() };
        b.extend_from_slice(out.as_bytes());
        b
    };
    std::fs::write(file, bytes)?;
    Ok(true)
}

// ---------------------------------------------------------------- Windows

/// The user PATH as it is stored, read through PowerShell like it is written.
fn user_path() -> String {
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "[Environment]::GetEnvironmentVariable('Path','User')"])
        .hidden().output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

/// tez's folders on the user PATH: a missing one goes first (after the one before it), and one
/// already there stays where the user put it.
fn user_path_first(dirs: &[PathBuf]) -> R<bool> {
    let current = user_path();
    let mut entries: Vec<String> = current.split(';').filter(|p| !p.trim().is_empty()).map(|s| s.to_string()).collect();
    let same = |e: &str, d: &Path| e.trim().trim_end_matches('\\').eq_ignore_ascii_case(d.display().to_string().trim_end_matches('\\'));
    let mut at = 0;
    let mut changed = false;
    for d in dirs {
        match entries.iter().position(|e| same(e, d)) {
            Some(i) => at = i + 1,
            None => { entries.insert(at, d.display().to_string()); at += 1; changed = true; }
        }
    }
    if !changed { return Ok(false); }
    let script = format!("[Environment]::SetEnvironmentVariable('Path', '{}', 'User')", entries.join(";").replace('\'', "''"));
    let st = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]).hidden().status()?;
    if !st.success() { bail!("could not add tez's folders to your PATH"); }
    Ok(true)
}

/// (the profile every host of this PowerShell reads, whether it may run scripts), or None when
/// this PowerShell is not installed.
fn powershell_profile(exe: &str) -> Option<(PathBuf, bool)> {
    // Without the caller's module path (from PowerShell 7 it stops Windows PowerShell loading
    // Get-ExecutionPolicy) and policy override, so the answer is the one a new window gets.
    let o = Command::new(exe)
        .args(["-NoProfile", "-NonInteractive", "-Command", "$PROFILE.CurrentUserAllHosts; Get-ExecutionPolicy"])
        .env_remove("PSModulePath").env_remove("PSExecutionPolicyPreference")
        .hidden().output().ok()?;
    if !o.status.success() { return None; }
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    let mut lines = out.lines().map(str::trim).filter(|l| !l.is_empty());
    let file = PathBuf::from(lines.next()?);
    let policy = lines.next().unwrap_or_default().to_ascii_lowercase();
    Some((file, !matches!(policy.as_str(), "restricted" | "allsigned" | "undefined" | "")))
}

/// (value, type) of cmd's AutoRun, if it has one.
fn read_autorun() -> Option<(String, String)> {
    let o = Command::new("reg").args(["query", CMD_KEY, "/v", "AutoRun"]).hidden().output().ok()?;
    if !o.status.success() { return None; }
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    out.lines().find_map(|l| {
        let l = l.trim_start();
        let rest = l.strip_prefix("AutoRun")?.trim_start();
        let (kind, value) = rest.split_once(char::is_whitespace)?;
        Some((value.trim().to_string(), kind.to_string()))
    })
}

/// cmd: tez's line in AutoRun, after whatever the user already runs there. True when it changed.
fn cmd_autorun() -> R<bool> {
    let (old, kind) = read_autorun().unwrap_or_else(|| (String::new(), "REG_SZ".into()));
    if old.contains(CMD_GUARD) { return Ok(false); }
    let new = if old.trim().is_empty() { cmd_line() } else { format!("{old} & {}", cmd_line()) };
    let kind = if kind == "REG_EXPAND_SZ" { "REG_EXPAND_SZ" } else { "REG_SZ" };
    let st = Command::new("reg").args(["add", CMD_KEY, "/v", "AutoRun", "/t", kind, "/d", &new, "/f"]).hidden().output()?;
    if !st.status.success() || read_autorun().map(|(v, _)| v) != Some(new) {
        bail!("could not set cmd's AutoRun ({})", String::from_utf8_lossy(&st.stderr).trim());
    }
    Ok(true)
}

/// The shell start-up files to hold tez's line: the ones that exist, else the one a new shell
/// reads (on Windows only when Git Bash is installed).
fn bash_files() -> Vec<PathBuf> {
    let home = paths::user_home();
    let names: &[&str] = if cfg!(windows) { &[".bash_profile", ".bashrc", ".profile"] } else { &[".zshrc", ".bashrc", ".profile"] };
    let mut files: Vec<PathBuf> = names.iter().map(|f| home.join(f)).filter(|p| p.exists()).collect();
    if files.is_empty() {
        if cfg!(windows) {
            let git_bash = std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("Git").join("bin").join("bash.exe"));
            if git_bash.map(|p| p.exists()).unwrap_or(false) { files.push(home.join(".bash_profile")); }
        } else {
            let zsh = std::env::var("SHELL").unwrap_or_default().ends_with("zsh");
            files.push(home.join(if zsh { ".zshrc" } else { ".profile" }));
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tez-shellpath-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("profile")
    }

    #[test]
    fn upsert_adds_once_and_replaces_an_older_line() {
        let f = temp("plain");
        std::fs::write(&f, "alias ll='ls -l'\r\nexport PATH=\"/old/.tez/bin:$PATH\"  # tez\r\n").unwrap();
        assert!(upsert(&f, "export PATH=\"$HOME/.tez/bin:$PATH\"  # tez", false).unwrap());
        assert!(!upsert(&f, "export PATH=\"$HOME/.tez/bin:$PATH\"  # tez", false).unwrap());
        let text = std::fs::read_to_string(&f).unwrap();
        // The user's line keeps its CRLF, the old tez line is gone, the new one ends in "\n".
        assert_eq!(text, "alias ll='ls -l'\r\nexport PATH=\"$HOME/.tez/bin:$PATH\"  # tez\n");
    }

    #[test]
    fn upsert_keeps_utf16_and_bom_files_as_they_are() {
        let f = temp("utf16");
        let mut b = vec![0xFF, 0xFE];
        for u in "Set-Alias g git\r\n".encode_utf16() { b.extend_from_slice(&u.to_le_bytes()); }
        std::fs::write(&f, b).unwrap();
        assert!(upsert(&f, "$x = 1  # tez: path", true).unwrap());
        let raw = std::fs::read(&f).unwrap();
        assert!(raw.starts_with(&[0xFF, 0xFE]));
        let units: Vec<u16> = raw[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&units).unwrap(), "Set-Alias g git\r\n$x = 1  # tez: path\r\n");

        let g = temp("bom");
        std::fs::write(&g, b"\xEF\xBB\xBF# mine\n").unwrap();
        assert!(upsert(&g, "$x = 1  # tez: path", true).unwrap());
        assert_eq!(std::fs::read(&g).unwrap(), b"\xEF\xBB\xBF# mine\n$x = 1  # tez: path\n");
    }

    #[test]
    fn lines_use_the_home_variable() {
        let home = paths::user_home();
        assert_eq!(for_shell(&home.join(".tez").join("bin"), Style::Sh), "$HOME/.tez/bin");
        if cfg!(windows) {
            assert_eq!(for_shell(&home.join(".tez").join("bin"), Style::PowerShell), "$env:USERPROFILE\\.tez\\bin");
            assert_eq!(for_shell(&home.join(".tez").join("bin"), Style::Cmd), "%USERPROFILE%\\.tez\\bin");
            assert_eq!(sh_path(Path::new(r"D:\tools\tez\bin")), "/d/tools/tez/bin");
        }
        assert!(cmd_line().starts_with("if not defined TEZ_BIN_FIRST (set \"TEZ_BIN_FIRST=1\" & set \"PATH="));
        assert!(ours(&powershell_line()) && ours(&sh_line()));
    }
}
