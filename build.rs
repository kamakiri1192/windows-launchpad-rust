fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon-liquid-glass-neutral.ico");

    configure_macos_swift_runtime();

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

fn configure_macos_swift_runtime() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    // Review artifacts bundle Swift libraries inside Launchpad.app. Keep the
    // executable relocatable instead of relying only on the build machine's
    // selected Xcode toolchain. Local runs fall back to the OS Swift runtime;
    // pointing at Xcode's back-deployment copy can load two Swift runtimes.
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
