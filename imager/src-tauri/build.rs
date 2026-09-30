fn main() {
    let mut attrs = tauri_build::Attributes::new();
    // Windows: raw disk access needs administrator rights, so the whole app asks for
    // elevation at start (like Raspberry Pi Imager). The manifest also keeps the
    // Common-Controls v6 dependency Tauri's default manifest provides.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        attrs = attrs.windows_attributes(
            tauri_build::WindowsAttributes::new()
                .app_manifest(include_str!("windows-app-manifest.xml")),
        );
    }
    tauri_build::try_build(attrs).expect("failed to run tauri-build");
}
