//! The few Win32 calls tez needs, declared by hand (no bindings crate).
//!
//! Every process tez starts goes into its own Job Object set to "kill on close". If tez is
//! stopped, crashes or is killed, Windows ends every child and grandchild with it, so nothing
//! is ever left running behind (php-cgi, postgres backends, node, horizon workers).

#![allow(non_snake_case, clippy::upper_case_acronyms)]

use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::process::Child;

pub type HANDLE = *mut c_void;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const DETACHED_PROCESS: u32 = 0x0000_0008;
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
pub const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
const JOB_OBJECT_LIMIT_BREAKAWAY_OK: u32 = 0x0800;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
const JOB_OBJECT_BASIC_PROCESS_ID_LIST: i32 = 3;
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

#[link(name = "kernel32")]
unsafe extern "system" {
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
