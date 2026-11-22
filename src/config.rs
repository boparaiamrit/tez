//! `config/tez.json`: the whole machine-wide state, in one human-editable file.

use crate::paths;
use crate::util::R;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Sites answer on `<name>.<tld>` and always on `<name>.localhost` too.
    pub tld: String,
    /// PHP version used where no site, `.php-version` or `tez.json` picks one.
    pub default_php: String,
    pub http_port: u16,
    pub https_port: u16,
    /// The web server in front of every site: "caddy" (the default) or "nginx".
    pub web_server: String,
    /// The tez daemon's own API and dashboard (loopback only).
    pub api_port: u16,
    /// php-cgi processes per PHP version (each serves one request at a time).
    pub php_workers: u16,
    pub xdebug: bool,
    /// Extra PHP extensions to enable on every version, beyond tez's defaults.
    pub extensions: Vec<String>,
    /// Folders whose sub-folders are each a site (like Herd's parked paths).
    pub parked: Vec<String>,
    /// Single linked sites: name -> folder.
    pub links: BTreeMap<String, String>,
    /// Folders inside a parked path that tez does not serve ("removed from tez"; the folder stays).
    pub hidden_sites: Vec<String>,
    /// Per-site settings, for parked and linked sites alike.
    pub sites: BTreeMap<String, SiteSettings>,
    pub services: BTreeMap<String, ServiceConfig>,
    /// php.ini settings for every version (key -> value), on top of tez's defaults.
    pub php_ini: BTreeMap<String, String>,
    /// php.ini settings for one version ("8.4" -> key -> value); these win over `php_ini`.
    pub php_ini_for: BTreeMap<String, BTreeMap<String, String>>,
    /// Extensions turned on (beyond the defaults) or off, per version.
    pub php_ext_on: BTreeMap<String, Vec<String>>,
    pub php_ext_off: BTreeMap<String, Vec<String>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct SiteSettings {
    /// Pin the site to a PHP version (`tez isolate 8.1`).
    pub php: Option<String>,
    /// Run the project's processes (Horizon or queue, Reverb, Vite, scheduler).
    pub up: bool,
    /// Turn single processes on or off ("schedule": true, "vite": false), over what tez detects.
    pub processes: BTreeMap<String, bool>,
    /// Extra host names this site answers on (multi-domain apps): "admin.local.example.com".
    pub domains: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ServiceConfig {
    pub version: String,
    pub port: u16,
    pub enabled: bool,
    /// Service settings (PostgreSQL: shared_buffers, max_connections...; Redis: maxmemory...).
    pub options: BTreeMap<String, String>,
}

impl Default for ServiceConfig {
    fn default() -> Self { ServiceConfig { version: String::new(), port: 0, enabled: true, options: BTreeMap::new() } }
}

impl Default for Config {
    fn default() -> Self {
        let sites_dir = paths::user_home().join("Sites").display().to_string();
        Config {
            tld: "test".into(),
            default_php: "8.4".into(),
            http_port: 80,
            https_port: 443,
            web_server: "caddy".into(),
            api_port: 7979,
            php_workers: 6,
            xdebug: false,
            extensions: Vec::new(),
            parked: vec![sites_dir],
            links: BTreeMap::new(),
            hidden_sites: Vec::new(),
            sites: BTreeMap::new(),
            services: BTreeMap::new(),
            php_ini: BTreeMap::new(),
            php_ini_for: BTreeMap::new(),
            php_ext_on: BTreeMap::new(),
            php_ext_off: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn load() -> Config {
        match std::fs::read_to_string(paths::config_file()) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("tez: {} is not valid JSON ({e}); using defaults", paths::config_file().display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) -> R<()> {
        std::fs::create_dir_all(paths::config_dir())?;
        let text = serde_json::to_string_pretty(self)?;
        let tmp = paths::config_file().with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, paths::config_file())?;
        Ok(())
    }

    pub fn site_mut(&mut self, name: &str) -> &mut SiteSettings {
        self.sites.entry(name.to_string()).or_default()
    }
}
