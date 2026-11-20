// Embeds the icon into chumly.exe so it shows in Explorer and on shortcuts.
// Executables have no icon of their own on other platforms, so nothing to do there.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        // Task Manager shows FileDescription; it defaults to the package name "client".
        res.set("FileDescription", "Chumly");
        res.set("ProductName", "Chumly");
        res.set("OriginalFilename", "chumly.exe");
        res.set("InternalName", "chumly");
        if let Err(e) = res.compile() {
            println!("cargo:warning=failed to embed icon: {e}");
        }
    }
}
