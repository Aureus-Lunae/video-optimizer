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
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to run tauri-build");
}
