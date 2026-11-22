//! The few Win32 calls tez needs, declared by hand (no bindings crate).
//!
//! Every process tez starts goes into its own Job Object set to "kill on close". If tez is
//! stopped, crashes or is killed, Windows ends every child and grandchild with it, so nothing
//! is ever left running behind (php-cgi, postgres backends, node, horizon workers).

#![allow(non_snake_case, clippy::upper_case_acronyms, dead_code)]

use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};

pub type HANDLE = *mut c_void;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const DETACHED_PROCESS: u32 = 0x0000_0008;
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
pub const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
const JOB_OBJECT_LIMIT_BREAKAWAY_OK: u32 = 0x0800;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
const JOB_OBJECT_BASIC_PROCESS_ID_LIST: i32 = 3;
const JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION: i32 = 1;
const TH32CS_SNAPPROCESS: u32 = 0x2;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const PROCESS_VM_READ: u32 = 0x0010;
const STILL_ACTIVE: u32 = 259;

#[repr(C)]
#[derive(Default)]
struct BasicLimit {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[repr(C)]
#[derive(Default)]
struct IoCounters { r_ops: u64, w_ops: u64, o_ops: u64, r_bytes: u64, w_bytes: u64, o_bytes: u64 }

#[repr(C)]
#[derive(Default)]
struct ExtendedLimit {
    basic: BasicLimit,
    io: IoCounters,
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

#[repr(C)]
struct PidList { assigned: u32, in_list: u32, ids: [usize; 512] }

#[repr(C)]
#[derive(Default)]
struct MemCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

/// CPU time used by every process that has been in a job (live and exited), in 100 ns units.
#[repr(C)]
#[derive(Default)]
struct Accounting {
    total_user: i64,
    total_kernel: i64,
    period_user: i64,
    period_kernel: i64,
    page_faults: u32,
    total_processes: u32,
    active_processes: u32,
    terminated_processes: u32,
}

#[repr(C)]
#[derive(Default)]
struct MemStatus {
    length: u32,
    load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page: u64,
    avail_page: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended: u64,
}

#[repr(C)]
struct ProcEntry {
    size: u32,
    usage: u32,
    pid: u32,
    default_heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    exe: [u16; 260],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalMemoryStatusEx(status: *mut MemStatus) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> HANDLE;
    fn Process32FirstW(snap: HANDLE, entry: *mut ProcEntry) -> i32;
    fn Process32NextW(snap: HANDLE, entry: *mut ProcEntry) -> i32;
    fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> HANDLE;
    fn SetInformationJobObject(job: HANDLE, class: i32, info: *const c_void, len: u32) -> i32;
    fn QueryInformationJobObject(job: HANDLE, class: i32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
    fn AssignProcessToJobObject(job: HANDLE, process: HANDLE) -> i32;
    fn TerminateJobObject(job: HANDLE, code: u32) -> i32;
    fn CloseHandle(h: HANDLE) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> HANDLE;
    fn GetExitCodeProcess(h: HANDLE, code: *mut u32) -> i32;
    fn SetConsoleCtrlHandler(handler: *const c_void, add: i32) -> i32;
    fn K32GetProcessMemoryInfo(process: HANDLE, counters: *mut MemCounters, cb: u32) -> i32;
    fn GetDiskFreeSpaceExW(dir: *const u16, free_to_caller: *mut u64, total: *mut u64, total_free: *mut u64) -> i32;
    fn GetStdHandle(which: u32) -> HANDLE;
    fn SetHandleInformation(h: HANDLE, mask: u32, flags: u32) -> i32;
    fn GetSystemTimes(idle: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
}

/// Bytes free for this user on the volume holding `path`.
pub fn free_bytes(path: &std::path::Path) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    let mut p = path.to_path_buf();
    while !p.exists() { if !p.pop() { break; } }
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is NUL-terminated and outlives the call; the out pointers are valid or null.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    if ok == 0 { u64::MAX } else { free }
}

/// Stop this process's console handles from leaking into a detached child. Without this the
/// daemon would hold the terminal's output pipe open and the `tez start` caller would never finish.
pub fn stop_std_inheritance() {
    const HANDLE_FLAG_INHERIT: u32 = 1;
    for which in [-10i32 as u32, -11i32 as u32, -12i32 as u32] {
        // SAFETY: GetStdHandle returns a handle owned by the process (or null/invalid, which
        // SetHandleInformation rejects harmlessly); we only clear its inherit flag.
        unsafe {
            let h = GetStdHandle(which);
            if !h.is_null() && h as isize != -1 { SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0); }
        }
    }
}

/// A Job Object that kills everything in it when tez lets go of it.
pub struct Job(HANDLE);
unsafe impl Send for Job {}

impl Job {
    pub fn new() -> Option<Job> {
        // SAFETY: plain Win32 calls with valid, owned arguments; the handle is checked before use.
        unsafe {
            let h = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if h.is_null() { return None; }
            let mut info = ExtendedLimit::default();
            info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
            let ok = SetInformationJobObject(
                h,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &info as *const ExtendedLimit as *const c_void,
                std::mem::size_of::<ExtendedLimit>() as u32,
            );
            if ok == 0 { CloseHandle(h); return None; }
            Some(Job(h))
        }
    }

    /// A new job holding `child` (and, from then on, everything it starts).
    pub fn attach(child: &Child) -> Option<Job> {
        let job = Job::new()?;
        job.assign(child);
        Some(job)
    }

    pub fn assign(&self, child: &Child) -> bool {
        // SAFETY: the child's handle stays valid while `child` is alive, which it is here.
        unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) != 0 }
    }

    pub fn terminate(&self) {
        // SAFETY: the job handle is owned by self and valid until drop.
        unsafe { TerminateJobObject(self.0, 1); }
    }

    pub fn pids(&self) -> Vec<u32> {
        let mut list = PidList { assigned: 0, in_list: 0, ids: [0; 512] };
        // SAFETY: `list` is a correctly sized, writable buffer for this information class.
        let ok = unsafe {
            QueryInformationJobObject(
                self.0,
                JOB_OBJECT_BASIC_PROCESS_ID_LIST,
                &mut list as *mut PidList as *mut c_void,
                std::mem::size_of::<PidList>() as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 { return Vec::new(); }
        list.ids[..(list.in_list as usize).min(512)].iter().map(|&p| p as u32).collect()
    }

    /// Working-set memory of every process in the job, in bytes.
    pub fn memory(&self) -> u64 { self.pids().into_iter().map(process_memory).sum() }

    /// CPU time (user + kernel) used by everything that has run in the job, in nanoseconds.
    pub fn cpu_time(&self) -> u64 {
        let mut info = Accounting::default();
        // SAFETY: `info` is a correctly sized, writable buffer for this information class.
        let ok = unsafe {
            QueryInformationJobObject(
                self.0,
                JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION,
                &mut info as *mut Accounting as *mut c_void,
                std::mem::size_of::<Accounting>() as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 { return 0; }
        ((info.total_user.max(0) + info.total_kernel.max(0)) as u64).saturating_mul(100)
    }
}

/// Physical memory of this computer: (total, available) in bytes.
pub fn memory_status() -> (u64, u64) {
    let mut s = MemStatus { length: std::mem::size_of::<MemStatus>() as u32, ..Default::default() };
    // SAFETY: `s` is sized via `length` as the call requires.
    let ok = unsafe { GlobalMemoryStatusEx(&mut s) };
    if ok == 0 { (0, 0) } else { (s.total_phys, s.avail_phys) }
}

/// Free and total bytes of the volume holding `path`.
pub fn disk_space(path: &std::path::Path) -> (u64, u64) {
    use std::os::windows::ffi::OsStrExt;
    let mut p = path.to_path_buf();
    while !p.exists() { if !p.pop() { break; } }
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: `wide` is NUL-terminated and outlives the call; the out pointers are valid or null.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    if ok == 0 { (0, 0) } else { (free, total) }
}

/// The pid of the first running process whose image name matches one of `names` (case-insensitive).
fn find_process(names: &[&str]) -> Option<u32> {
    // SAFETY: the snapshot handle is checked and closed; `entry` is sized via `size`.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 { return None; }
        let mut entry: ProcEntry = std::mem::zeroed();
        entry.size = std::mem::size_of::<ProcEntry>() as u32;
        let mut found = None;
        let mut ok = Process32FirstW(snap, &mut entry);
        while ok != 0 {
            let len = entry.exe.iter().position(|&c| c == 0).unwrap_or(entry.exe.len());
            let exe = String::from_utf16_lossy(&entry.exe[..len]).to_ascii_lowercase();
            let stem = exe.trim_end_matches(".exe");
            if names.iter().any(|n| stem == n.to_ascii_lowercase()) { found = Some(entry.pid); break; }
            ok = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
        found
    }
}

/// Memory held by the WSL virtual machine (the vmmemWSL process), or None when WSL is not running.
/// Windows usually lets a normal user read it; when it does not, `tasklist` reports it.
pub fn wsl_memory() -> Option<u64> {
    let pid = find_process(&["vmmemWSL", "vmmem"])?;
    // SAFETY: handle checked for null and closed; counters buffer is sized via `cb`.
    let direct = unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() { 0 } else {
            let mut c = MemCounters { cb: std::mem::size_of::<MemCounters>() as u32, ..Default::default() };
            let ok = K32GetProcessMemoryInfo(h, &mut c, c.cb);
            CloseHandle(h);
            if ok == 0 { 0 } else { c.working_set_size as u64 }
        }
    };
    if direct > 0 { return Some(direct); }
    for image in ["vmmemWSL", "vmmem"] {
        let out = Command::new(crate::paths::system32("tasklist.exe"))
            .args(["/FI", &format!("IMAGENAME eq {image}"), "/FO", "CSV", "/NH"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .output().ok()?;
        if let Some(kb) = parse_tasklist_kb(&String::from_utf8_lossy(&out.stdout)) { return Some(kb * 1024); }
    }
    None
}

/// The "Mem Usage" column (KB) of tasklist's CSV output, whatever the locale's digit grouping.
pub fn parse_tasklist_kb(csv: &str) -> Option<u64> {
    let line = csv.lines().find(|l| l.starts_with('"'))?;
    let last = line.trim().trim_end_matches('"').rsplit("\",\"").next()?;
    let digits: String = last.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Stop every WSL distribution and its virtual machine.
pub fn wsl_shutdown() -> crate::util::R<()> {
    let status = Command::new(crate::paths::system32("wsl.exe")).arg("--shutdown").creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() { crate::bail!("wsl --shutdown failed ({status})"); }
    Ok(())
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: closing our own handle once. Closing it kills the job's processes (KILL_ON_CLOSE).
        unsafe { CloseHandle(self.0); }
    }
}

pub fn process_memory(pid: u32) -> u64 {
    // SAFETY: handle checked for null and closed; counters buffer is sized via `cb`.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
        if h.is_null() { return 0; }
        let mut c = MemCounters { cb: std::mem::size_of::<MemCounters>() as u32, ..Default::default() };
        let ok = K32GetProcessMemoryInfo(h, &mut c, c.cb);
        CloseHandle(h);
        if ok == 0 { 0 } else { c.working_set_size as u64 }
    }
}

pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 { return false; }
    // SAFETY: handle checked for null and closed after one query.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() { return false; }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code);
        CloseHandle(h);
        ok != 0 && code == STILL_ACTIVE
    }
}

/// Let Ctrl+C reach the child (php, composer) and not end the shim that is waiting for it.
pub fn ignore_ctrl_c() {
    // SAFETY: a null handler with add=TRUE makes this process ignore Ctrl+C; always valid.
    unsafe { SetConsoleCtrlHandler(std::ptr::null(), 1); }
}

/// Run a command elevated (UAC prompt) and wait for it. Returns its exit code.
pub fn run_elevated(exe: &std::path::Path, args: &[String]) -> crate::util::R<i32> {
    let quoted: Vec<String> = args.iter().map(|a| format!("'{}'", a.replace('\'', "''"))).collect();
    let script = format!(
        "$p = Start-Process -FilePath '{}' -ArgumentList @({}) -Verb RunAs -Wait -PassThru -WindowStyle Hidden; exit $p.ExitCode",
        exe.display().to_string().replace('\'', "''"),
        quoted.join(",")
    );
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()?;
    Ok(status.code().unwrap_or(1))
}

pub fn is_elevated() -> bool {
    // `net session` only succeeds in an elevated process.
    std::process::Command::new(crate::paths::system32("net.exe"))
        .arg("session")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ------------------------------------------------------------------ the API every OS provides

/// Set up a managed child before it starts: no console window.
pub fn prepare_child(cmd: &mut Command) { cmd.creation_flags(CREATE_NO_WINDOW); }

/// End a managed process and everything it started (its job). The caller has already asked it
/// to stop nicely (the spec's stop command) where it has one.
pub fn kill_tree(job: Option<Job>, child: Option<Child>) {
    if let Some(job) = job { job.terminate(); }
    if let Some(mut child) = child { let _ = child.kill(); let _ = child.wait(); }
}

/// Start a program that outlives this one (the daemon, the tray app), detached from the console
/// and, where allowed, from the terminal's job so closing the terminal keeps it running.
pub fn spawn_detached(cmd: &mut Command) -> std::io::Result<Child> {
    stop_std_inheritance();
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    cmd.creation_flags(base | CREATE_BREAKAWAY_FROM_JOB).spawn().or_else(|_| cmd.creation_flags(base).spawn())
}

/// Open a URL in the default browser.
pub fn open_url(url: &str) -> std::io::Result<()> {
    Command::new(crate::paths::system32("cmd.exe")).args(["/c", "start", "", url]).creation_flags(CREATE_NO_WINDOW).spawn().map(|_| ())
}

/// Job Objects already end every child when tez exits, however it exits.
pub fn on_shutdown_signal(_f: fn()) {}
pub fn remember_groups(_procs: &[u32]) {}
pub fn kill_stale_groups() {}

/// Whole-machine CPU counters as (idle, total) in 100 ns units; the caller takes deltas.
pub fn system_cpu_times() -> Option<(u64, u64)> {
    let (mut idle, mut kernel, mut user) = (0u64, 0u64, 0u64);
    // SAFETY: three valid out pointers to FILETIME-sized integers.
    let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    // Kernel time already includes idle time.
    if ok == 0 { None } else { Some((idle, kernel + user)) }
}

/// Only used where `system_cpu_times` is unavailable (macOS).
pub fn system_cpu_percent() -> Option<f64> { None }

// ------------------------------------------------------------------ the Recycle Bin

const FO_DELETE: u32 = 3;
const FOF_SILENT: u16 = 0x0004;
const FOF_NOCONFIRMATION: u16 = 0x0010;
const FOF_ALLOWUNDO: u16 = 0x0040;
const FOF_NOERRORUI: u16 = 0x0400;
/// If Windows would delete outright instead of recycling (the bin is too small, say), it asks
/// first rather than deleting silently. tez checks those cases before; this is the last guard.
const FOF_WANTNUKEWARNING: u16 = 0x4000;
const DRIVE_FIXED: u32 = 3;
const COINIT_APARTMENTTHREADED: u32 = 0x2;
const COINIT_DISABLE_OLE1DDE: u32 = 0x4;

/// SHFILEOPSTRUCTW, natural alignment on 64-bit Windows.
#[repr(C)]
struct ShFileOp {
    hwnd: *mut c_void,
    func: u32,
    from: *const u16,
    to: *const u16,
    flags: u16,
    aborted: i32,
    name_mappings: *mut c_void,
    progress_title: *const u16,
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHFileOperationW(op: *mut ShFileOp) -> i32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
    fn CoUninitialize();
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetDriveTypeW(root: *const u16) -> u32;
    fn GetVolumePathNameW(path: *const u16, out: *mut u16, len: u32) -> i32;
    fn GetVolumeNameForVolumeMountPointW(mount: *const u16, out: *mut u16, len: u32) -> i32;
}

/// A path as Windows APIs want it: no `\\?\` prefix, NUL-terminated UTF-16.
fn wide_path(path: &std::path::Path) -> Vec<u16> {
    let s = path.to_string_lossy().to_string();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The root of the volume holding `path` ("C:\", or a mount folder), NUL-terminated.
fn volume_root(path: &std::path::Path) -> Option<Vec<u16>> {
    let p = wide_path(path);
    let mut out = vec![0u16; 1024];
    // SAFETY: `p` is NUL-terminated; `out` has room for 1024 UTF-16 units, the length we pass.
    let ok = unsafe { GetVolumePathNameW(p.as_ptr(), out.as_mut_ptr(), out.len() as u32) };
    if ok == 0 { return None; }
    let n = out.iter().position(|c| *c == 0).unwrap_or(out.len());
    out.truncate(n);
    out.push(0);
    Some(out)
}

/// "{GUID}" of the volume whose root is `root` (NUL-terminated, with a trailing backslash).
fn volume_guid(root: &[u16]) -> Option<String> {
    let mut out = vec![0u16; 64];
    // SAFETY: `root` is NUL-terminated; `out` holds 64 units, which a volume GUID path fits in.
    let ok = unsafe { GetVolumeNameForVolumeMountPointW(root.as_ptr(), out.as_mut_ptr(), out.len() as u32) };
    if ok == 0 { return None; }
    let s = String::from_utf16_lossy(&out[..out.iter().position(|c| *c == 0).unwrap_or(out.len())]);
    let start = s.find('{')?;
    let end = s[start..].find('}')? + start;
    Some(s[start..=end].to_string())
}

/// A DWORD under HKCU, read with reg.exe (no window).
fn reg_dword(key: &str, value: &str) -> Option<u64> {
    let out = Command::new(crate::paths::system32("reg.exe")).args(["query", key, "/v", value])
        .creation_flags(CREATE_NO_WINDOW).stdin(std::process::Stdio::null()).output().ok()?;
    if !out.status.success() { return None; }
    parse_reg_dword(&String::from_utf8_lossy(&out.stdout), value)
}

/// The value in reg.exe's output line "    MaxCapacity    REG_DWORD    0x3e8".
pub fn parse_reg_dword(text: &str, value: &str) -> Option<u64> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        if it.next()? != value || it.next()? != "REG_DWORD" { return None; }
        u64::from_str_radix(it.next()?.trim_start_matches("0x"), 16).ok()
    })
}

/// Whether `path` (`size` bytes) can go to the Recycle Bin, or why not. Network and removable
/// drives have no Recycle Bin, and a bin that is off or too small means Windows deletes outright,
/// which tez never does.
pub fn trash_check(path: &std::path::Path, size: u64) -> Result<&'static str, String> {
    let root = volume_root(path).ok_or_else(|| "Windows could not tell which drive it is on".to_string())?;
    // SAFETY: `root` is NUL-terminated.
    let kind = unsafe { GetDriveTypeW(root.as_ptr()) };
    if kind != DRIVE_FIXED { return Err("it is on a network or removable drive, which has no Recycle Bin".into()); }
    if let Some(guid) = volume_guid(&root) {
        let key = format!(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{guid}");
        if reg_dword(&key, "NukeOnDelete") == Some(1) {
            return Err("the Recycle Bin is turned off for this drive, so Windows would delete it outright".into());
        }
        if let Some(mb) = reg_dword(&key, "MaxCapacity") {
            if size > mb << 20 {
                return Err(format!(
                    "it is {} and this drive's Recycle Bin holds {}, so Windows would delete it outright",
                    crate::util::human_bytes(size),
                    crate::util::human_bytes(mb << 20)
                ));
            }
        }
    }
    Ok("Recycle Bin")
}

/// Move a folder to the Recycle Bin. Never deletes outright: after `trash_check`, Windows
/// recycles it, and if it still would not, it asks (FOF_WANTNUKEWARNING) instead of deleting.
pub fn move_to_trash(path: &std::path::Path) -> crate::util::R<&'static str> {
    let mut from = wide_path(path);
    if from.len() > 260 { crate::bail!("the path is longer than the 260 characters the Recycle Bin accepts"); }
    from.push(0); // a list of paths ends with a second NUL
    let mut op = ShFileOp {
        hwnd: std::ptr::null_mut(),
        func: FO_DELETE,
        from: from.as_ptr(),
        to: std::ptr::null(),
        flags: FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI | FOF_WANTNUKEWARNING,
        aborted: 0,
        name_mappings: std::ptr::null_mut(),
        progress_title: std::ptr::null(),
    };
    // SAFETY: COM is initialised on this thread for the shell call and released afterwards (only
    // when this call initialised it); `op` and `from` (double-NUL-terminated) outlive the call.
    let rc = unsafe {
        let init = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let rc = SHFileOperationW(&mut op);
        if init >= 0 { CoUninitialize(); }
        rc
    };
    if rc != 0 { crate::bail!("{}", shfileop_error(rc)); }
    if op.aborted != 0 { crate::bail!("Windows stopped before recycling it"); }
    if path.exists() { crate::bail!("Windows reported success but the folder is still there"); }
    Ok("Recycle Bin")
}

/// SHFileOperation's error codes, in words.
fn shfileop_error(rc: i32) -> String {
    match rc {
        5 | 0x78 => "Windows denied access to it or to something inside it".into(),
        32 | 33 => "a program is using a file in it (an editor, terminal or Explorer window open there?)".into(),
        0x7C | 0x402 => "Windows could not find it".into(),
        0x75 => "it was cancelled".into(),
        _ => format!("Windows error {rc:#x}"),
    }
}