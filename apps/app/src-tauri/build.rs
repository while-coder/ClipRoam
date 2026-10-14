fn main() {
    println!("cargo:rerun-if-changed=../../../package.json");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // Tauri caches the development app icon before embedding it.
        println!("cargo:rerun-if-changed=icons/icon.icns");
    }
    tauri_build::build()
}
