//! The taskbar tray icon: left click opens the window, right click shows the menu.

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum TrayAction { Open, Dashboard, Site(String), Start, Restart, Stop, Quit }

pub struct Tray {
    icon: TrayIcon,
    open: MenuId,
    dashboard: MenuId,
    start: MenuId,
    restart: MenuId,
    stop: MenuId,
    quit: MenuId,
    sites: Vec<(MenuId, String)>,
    sites_menu: Submenu,
    site_items: Vec<MenuItem>,
    shown_sites: Vec<String>,
}

/// The tray icon, 32x32 RGBA drawn by scripts/icons.py: the violet tile with the white bolt. The
/// macOS menu bar wants a template image instead: the bolt alone, which macOS tints to match.
fn icon() -> Icon {
    const N: u32 = 32;
    let rgba: &[u8] = if cfg!(target_os = "macos") { include_bytes!("../assets/icon/tray-template.rgba") } else { include_bytes!("../assets/icon/tray.rgba") };
    Icon::from_rgba(rgba.to_vec(), N, N).expect("tray icon")
}

impl Tray {
    pub fn new() -> Option<Tray> {
        let menu = Menu::new();
        let open = MenuItem::new("Open tez", true, None);
        let dashboard = MenuItem::new("Open web dashboard", true, None);
        let sites_menu = Submenu::new("Sites", true);
        let start = MenuItem::new("Start everything", true, None);
        let restart = MenuItem::new("Restart everything", true, None);
        let stop = MenuItem::new("Stop everything", true, None);
        let quit = MenuItem::new("Quit tez app", true, None);
        let _ = menu.append_items(&[&open, &dashboard, &PredefinedMenuItem::separator(), &sites_menu, &PredefinedMenuItem::separator(), &start, &restart, &stop, &PredefinedMenuItem::separator(), &quit]);
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("tez")
            .with_icon(icon())
            .with_icon_as_template(cfg!(target_os = "macos"))
            .build()
            .ok()?;
        Some(Tray {
            icon,
            open: open.id().clone(),
            dashboard: dashboard.id().clone(),
            start: start.id().clone(),
            restart: restart.id().clone(),
            stop: stop.id().clone(),
            quit: quit.id().clone(),
            sites: Vec::new(),
            sites_menu,
            site_items: Vec::new(),
            shown_sites: vec!["\u{0}".into()],
        })
    }

    pub fn set_tooltip(&self, text: &str) { let _ = self.icon.set_tooltip(Some(text)); }

    /// Keep the Sites submenu in step with the daemon's sites (name, url).
    pub fn set_sites(&mut self, sites: &[(String, String)]) {
        let names: Vec<String> = sites.iter().map(|(n, _)| n.clone()).collect();
        if names == self.shown_sites { return; }
        for item in self.site_items.drain(..) { let _ = self.sites_menu.remove(&item); }
        self.sites.clear();
        for (name, url) in sites {
            let item = MenuItem::new(format!("{name}    {}", url.trim_start_matches("https://")), true, None);
            self.sites.push((item.id().clone(), url.clone()));
            let _ = self.sites_menu.append(&item);
            self.site_items.push(item);
        }
        if sites.is_empty() {
            let item = MenuItem::new("No sites yet", false, None);
            let _ = self.sites_menu.append(&item);
            self.site_items.push(item);
        }
        self.shown_sites = names;
    }

    /// Drain pending tray clicks and menu choices.
    pub fn poll(&self) -> Vec<TrayAction> {
        let mut out = Vec::new();
        while let Ok(e) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e { out.push(TrayAction::Open); }
            if let TrayIconEvent::DoubleClick { .. } = e { out.push(TrayAction::Open); }
        }
        while let Ok(e) = MenuEvent::receiver().try_recv() {
            let id = e.id;
            if id == self.open { out.push(TrayAction::Open); }
            else if id == self.dashboard { out.push(TrayAction::Dashboard); }
            else if id == self.start { out.push(TrayAction::Start); }
            else if id == self.restart { out.push(TrayAction::Restart); }
            else if id == self.stop { out.push(TrayAction::Stop); }
            else if id == self.quit { out.push(TrayAction::Quit); }
            else if let Some((_, url)) = self.sites.iter().find(|(i, _)| *i == id) { out.push(TrayAction::Site(url.clone())); }
        }
        out
    }
}
