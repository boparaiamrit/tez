//! Windows: put tez's icon into tez-app.exe as icon resource 1, the one GPUI loads for the window
//! class, so the window, taskbar, Alt-Tab and Explorer all show it. The icons are drawn by
//! scripts/icons.py.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/tez.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") { return; }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon/tez.ico")
        .set("ProductName", "tez")
        .set("FileDescription", "tez")
        .set("CompanyName", "tez");
    if let Err(e) = res.compile() {
        // A missing resource compiler costs the icon, not the build.
        println!("cargo:warning=tez-app has no icon: {e}");
    }
}
