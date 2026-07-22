fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon-liquid-glass-neutral.ico");

    #[cfg(windows)]
    {
        // `cfg(windows)` describes the host that compiles this build script.
        // A Windows developer may still be checking a macOS target, in which
        // case invoking rc.exe would incorrectly try to embed a PE resource in
        // a Mach-O binary.
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
            return;
        }
        winresource::WindowsResource::new()
            .set_icon("assets/app-icon-liquid-glass-neutral.ico")
            .compile()
            .expect("embed Windows app icon resource");
    }
}
