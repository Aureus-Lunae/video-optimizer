//! Thin Tauri shell over the session: the commands, the window's drop and
//! close, and events back to the window. `session` holds what the commands
//! do; `events` the payloads.
//!
//! The window never hands the backend a path. Paths come only from a native
//! dialog opened here, or from a drop that Tauri delivers here as a window
//! event; the window hears file and folder names, numbers and reasons.

pub mod events;
pub mod session;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, Window, WindowEvent, Wry};
use tauri_plugin_dialog::{DialogExt, FileDialogBuilder, FilePath};
use video_optimizer_core::tools::Tools;

use events::Event;
use session::{CLOSE_WAIT, Emit, FolderAnswer, Shared};

/// Extensions the file chooser shows. Any dropped file is tried; ffprobe
/// decides.
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "m4v", "mkv", "webm", "avi"];

/// What the file chooser calls the filter over [`VIDEO_EXTENSIONS`].
const VIDEO_FILTER_NAME: &str = "Videos";

/// Everything the backend keeps between calls.
struct AppState {
    shared: Arc<Shared>,
}

/// Sends each event to the window under its name. A failure to send is
/// printed; the job goes on.
fn emitter(app: AppHandle) -> Emit {
    Arc::new(move |event: Event| {
        if let Err(error) = app.emit(event.name(), &event) {
            eprintln!(
                "video-optimizer: could not send {} to the window: {error}",
                event.name()
            );
        }
    })
}

/// The session in the app's state.
fn shared(app: &AppHandle) -> Arc<Shared> {
    Arc::clone(&app.state::<AppState>().shared)
}

/// A file dialog that is modal to the main window.
fn file_dialog(app: &AppHandle) -> FileDialogBuilder<Wry> {
    let dialog = app.dialog().file();
    match app.get_webview_window("main") {
        Some(window) => dialog.set_parent(&window),
        None => dialog,
    }
}

/// A dialog's answer as a path: `None` for a closed dialog, and for an entry
/// that is not a file path (desktop dialogs give paths only).
fn one_path(picked: Option<FilePath>) -> Option<PathBuf> {
    picked.and_then(|path| path.into_path().ok())
}

/// Open the file chooser; the video picked is loaded (see
/// [`Shared::load`]). Returns at once; the outcome comes as a `video` or
/// `notice` event, from the dialog plugin's thread. A closed dialog sends
/// nothing.
#[tauri::command(async)]
fn choose_video(app: AppHandle) {
    let answer = app.clone();
    file_dialog(&app)
        .set_title("Choose a video")
        .add_filter(VIDEO_FILTER_NAME, VIDEO_EXTENSIONS)
        .pick_file(move |picked| {
            if let Some(path) = one_path(picked) {
                shared(&answer).load(&[path]);
            }
        });
}

/// Open the folder chooser and answer with the name of the folder a save
/// goes to now (see [`Shared::choose_folder`]). Not on the main thread
/// (`async`), as it waits for the dialog.
#[tauri::command(async)]
fn choose_folder(app: AppHandle) -> FolderAnswer {
    let picked = file_dialog(&app)
        .set_title("Choose the folder to save in")
        .blocking_pick_folder();
    shared(&app).choose_folder(one_path(picked))
}

/// Back to saving next to the original.
#[tauri::command(async)]
fn clear_folder(app: AppHandle) {
    shared(&app).clear_folder();
}

/// See [`Shared::make_sample`]. `crf` and `audio` are taken as any JSON
/// value, so the core's checks decide, and their sentence is the error.
#[tauri::command(async)]
fn make_sample(
    app: AppHandle,
    video: u64,
    request: u64,
    crf: Value,
    audio: Value,
) -> Result<(), String> {
    shared(&app).make_sample(video, request, &crf, &audio)
}

/// See [`Shared::save`].
#[tauri::command(async)]
fn save(app: AppHandle, video: u64, crf: Value, audio: Value) -> Result<(), String> {
    shared(&app).save(video, &crf, &audio)
}

/// See [`Shared::cancel_save`].
#[tauri::command(async)]
fn cancel_save(app: AppHandle) {
    shared(&app).cancel_save();
}

/// Whether `event` is the window closing. Asking to close and the window
/// going away both count (closing twice is harmless), so a close that skips
/// the request is covered too.
fn is_closing(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
    )
}

/// The paths of a drop, if `event` is one: all of them, so that loading can
/// refuse several.
fn dropped_paths(event: &WindowEvent) -> Option<Vec<PathBuf>> {
    match event {
        WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => Some(paths.clone()),
        _ => None,
    }
}

/// The window closing and dropping, handled here so no path reaches the
/// window. Closing stops the jobs and waits for them (at most
/// [`CLOSE_WAIT`]) before the window goes, so a stopped save leaves no
/// partial file. A drop is loaded off the main thread, as loading probes the
/// file.
fn on_window_event(window: &Window, event: &WindowEvent) {
    if is_closing(event) {
        shared(window.app_handle()).close(CLOSE_WAIT);
    }
    if let Some(paths) = dropped_paths(event) {
        let shared = shared(window.app_handle());
        tauri::async_runtime::spawn_blocking(move || shared.load(&paths));
    }
}

/// Start the window. The app cannot do anything without ffmpeg, so it does
/// not start when the programs are not found.
pub fn run() -> ExitCode {
    let tools = match Tools::locate() {
        Ok(tools) => tools,
        Err(error) => {
            eprintln!("video-optimizer: {error}");
            return ExitCode::FAILURE;
        }
    };
    // The dialog plugin must be registered for its Rust API; its commands
    // stay unreachable, as the window has no dialog permission.
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let emit = emitter(app.handle().clone());
            let shared = Shared::new(tools, std::env::temp_dir(), emit);
            app.manage(AppState { shared });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            choose_video,
            choose_folder,
            clear_folder,
            make_sample,
            save,
            cancel_save
        ])
        .on_window_event(on_window_event)
        .run(tauri::generate_context!());
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("video-optimizer: the window could not start: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use tauri::{PhysicalPosition, PhysicalSize};

    use super::*;

    #[test]
    fn one_picked_path_is_that_path() {
        let path = PathBuf::from("/videos/Showreel V1.mp4");
        assert_eq!(one_path(Some(FilePath::Path(path.clone()))), Some(path));
    }

    #[test]
    fn no_pick_or_a_non_file_url_is_none() {
        assert_eq!(one_path(None), None);
        let web = tauri::Url::parse("https://example.com/a.mp4").unwrap();
        assert_eq!(one_path(Some(FilePath::Url(web))), None);
    }

    fn drop_of(paths: &[&str]) -> WindowEvent {
        WindowEvent::DragDrop(DragDropEvent::Drop {
            paths: paths.iter().map(PathBuf::from).collect(),
            position: PhysicalPosition::new(10.0, 10.0),
        })
    }

    #[test]
    fn a_close_request_and_a_destroyed_window_are_closing() {
        assert!(is_closing(&WindowEvent::Destroyed));
        // CloseRequested has a private field; it is matched in `is_closing`
        // with `{ .. }`, which this test cannot build.
    }

    #[test]
    fn other_window_events_are_not_closing() {
        for event in [
            WindowEvent::Focused(true),
            WindowEvent::Resized(PhysicalSize::new(800, 600)),
            drop_of(&["/v/a.mp4"]),
        ] {
            assert!(!is_closing(&event), "{event:?}");
        }
    }

    #[test]
    fn a_drop_gives_its_paths_all_of_them() {
        assert_eq!(
            dropped_paths(&drop_of(&["/v/a.mp4", "/v/b.mp4"])),
            Some(vec![PathBuf::from("/v/a.mp4"), PathBuf::from("/v/b.mp4")])
        );
    }

    #[test]
    fn a_drag_that_is_not_a_drop_gives_no_paths() {
        for event in [
            WindowEvent::DragDrop(DragDropEvent::Enter {
                paths: vec![PathBuf::from("/v/a.mp4")],
                position: PhysicalPosition::new(1.0, 1.0),
            }),
            WindowEvent::DragDrop(DragDropEvent::Over {
                position: PhysicalPosition::new(1.0, 1.0),
            }),
            WindowEvent::DragDrop(DragDropEvent::Leave),
            WindowEvent::Focused(true),
        ] {
            assert_eq!(dropped_paths(&event), None, "{event:?}");
        }
    }

    #[test]
    fn the_file_chooser_lists_exactly_the_six_video_extensions() {
        assert_eq!(
            VIDEO_EXTENSIONS,
            ["mp4", "mov", "m4v", "mkv", "webm", "avi"]
        );
    }
}
