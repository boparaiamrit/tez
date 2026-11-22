//! A process tez should keep running: what to start, where, and how to stop it gently.

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    /// Unique id: "postgres", "php-8.4-0", "web", "acmeshop/horizon".
    pub name: String,
    /// "service", "php", "web" or "site:<name>".
    pub group: String,
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    /// A command that asks the process to stop cleanly (pg_ctl stop, redis-cli shutdown).
    pub stop: Option<(PathBuf, Vec<String>)>,
    pub port: Option<u16>,
    /// A link shown on the dashboard (a web UI, the Horizon page).
    pub url: Option<String>,
    /// Pass `args` to the program exactly as written (cmd.exe needs its own quoting).
    pub raw: bool,
}

impl Spec {
    pub fn new(name: impl Into<String>, group: impl Into<String>, exe: PathBuf) -> Spec {
        Spec { name: name.into(), group: group.into(), exe, args: Vec::new(), cwd: crate::paths::home(), env: Vec::new(), stop: None, port: None, url: None, raw: false }
    }
    pub fn args<I: IntoIterator<Item = S>, S: Into<String>>(mut self, a: I) -> Spec { self.args.extend(a.into_iter().map(Into::into)); self }
    pub fn cwd(mut self, d: PathBuf) -> Spec { self.cwd = d; self }
    pub fn env(mut self, k: &str, v: impl Into<String>) -> Spec { self.env.push((k.to_string(), v.into())); self }
    pub fn port(mut self, p: u16) -> Spec { self.port = Some(p); self }
    pub fn url(mut self, u: impl Into<String>) -> Spec { self.url = Some(u.into()); self }
    pub fn stop_with(mut self, exe: PathBuf, args: Vec<String>) -> Spec { self.stop = Some((exe, args)); self }
}
