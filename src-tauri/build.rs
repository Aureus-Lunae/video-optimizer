fn main() {
    // Declaring the app's commands makes each one need a permission
    // (`allow-<command>`), so the capability lists exactly what the window
    // may call. Without this, every app command would be open to the window.
    let manifest = tauri_build::AppManifest::new().commands(&[
        "choose_video",
        "choose_folder",
        "clear_folder",
        "make_sample",
        "save",
        "cancel_save",
    ]);

    // The message box shown when ffmpeg is missing uses TaskDialogIndirect,
    // which only exists in Common Controls 6, so every Windows program of this
    // package needs the manifest that asks for it. tauri-build embeds it only
    // into the app itself (`rustc-link-arg-bins`), so the test programs failed
    // to start (STATUS_ENTRYPOINT_NOT_FOUND). Embed it for every program here
    // instead, and tell tauri-build not to add a second copy to the app.
    let mut windows = tauri_build::WindowsAttributes::new();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", path.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", path.display());
    }

    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(manifest)
            .windows_attributes(windows),
    )
    .expect("failed to run tauri-build");
}
