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

    let Ok(output) = std::process::Command::new("xcrun")
        .args(["--find", "swift"])
        .output()
    else {
        println!("cargo:warning=Could not invoke xcrun to locate the Swift runtime");
        return;
    };
    if !output.status.success() {
        println!("cargo:warning=xcrun could not locate the Swift compiler");
        return;
    }

    let swift = String::from_utf8_lossy(&output.stdout);
    let swift = std::path::Path::new(swift.trim());
    let Some(toolchain_usr) = swift.parent().and_then(std::path::Path::parent) else {
        println!(
            "cargo:warning=Unexpected Swift compiler path: {}",
            swift.display()
        );
        return;
    };
    let runtime = toolchain_usr.join("lib/swift/macosx");
    if runtime.is_dir() {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", runtime.display());
    } else {
        println!(
            "cargo:warning=Swift runtime directory was not found at {}",
            runtime.display()
        );
    }
}
