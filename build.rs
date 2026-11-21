fn main() {
    // Embed the .exe icon + version info when building for Windows.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "SoftVault")
            .set("FileDescription", "SoftVault – zero-knowledge password & notes vault")
            .set("LegalCopyright", "MIT");
        res.compile().expect("failed to compile Windows resources");
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
