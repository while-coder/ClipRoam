fn main() {
    println!("cargo:rerun-if-changed=../../../package.json");
    println!("cargo:rerun-if-changed=../../../apps/icon");
    println!("cargo:rerun-if-changed=native/sync-icons.cjs");
    let platform = std::env::var("CARGO_CFG_TARGET_OS").expect("missing target platform");
    println!("cargo:rerun-if-changed=native/{platform}/sync-icons.cjs");
    // This also covers direct cargo build/check, without relying on Tauri hooks.
    let status = std::process::Command::new("node")
        .args(["native/sync-icons.cjs", &platform])
        .status()
        .expect("failed to run native icon generator; install Node.js and pnpm dependencies first");
    assert!(status.success(), "native icon generation failed");
    println!("cargo:rerun-if-changed=native/{platform}/icons");
    tauri_build::build()
}
