fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon-liquid-glass-neutral.ico");

    #[cfg(windows)]
    {
        winresource::WindowsResource::new()
            .set_icon("assets/app-icon-liquid-glass-neutral.ico")
            .compile()
            .expect("embed Windows app icon resource");
    }
}
