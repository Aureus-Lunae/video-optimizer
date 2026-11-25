//! Thin Tauri shell over the session: the commands, the window's drop and
//! close, and events back to the window. `session` holds what the commands
//! do; `events` the payloads.
//!
//! The window never hands the backend a path. Paths come only from a native
//! dialog opened here, or from a drop that Tauri delivers here as a window
//! event; the window hears file and folder names, numbers and reasons.

pub mod events;
pub mod media;
pub mod session;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use serde_json::Value;
use tauri::{
    AppHandle, Context, DragDropEvent, Emitter, Manager, RunEvent, Runtime, UriSchemeContext,
    UriSchemeResponder, Window, WindowEvent, Wry,
};
use tauri_plugin_dialog::{DialogExt, FileDialogBuilder, FilePath, MessageDialogKind};
use video_optimizer_core::tools::{Tools, ToolsError};

use events::Event;
use session::{CLOSE_WAIT, Emit, FolderAnswer, Shared};

/// Extensions the file chooser shows. Any dropped file is tried; ffprobe
/// decides.
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "m4v", "mkv", "webm", "avi"];

/// What the file chooser calls the filter over [`VIDEO_EXTENSIONS`].
const VIDEO_FILTER_NAME: &str = "Videos";

/// What the system message box says when the app cannot start because
/// ffmpeg and ffprobe are not beside it.
const TOOLS_MISSING: &str =
    "Video optimizer cannot start: ffmpeg and ffprobe were not found beside the app. Reinstall it.";

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

/// `dialog` set up as the file chooser for a video: its title, and one filter
/// named [`VIDEO_FILTER_NAME`] over [`VIDEO_EXTENSIONS`].
fn video_chooser<R: Runtime>(dialog: FileDialogBuilder<R>) -> FileDialogBuilder<R> {
    dialog
        .set_title("Choose a video")
        .add_filter(VIDEO_FILTER_NAME, VIDEO_EXTENSIONS)
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
    video_chooser(file_dialog(&app)).pick_file(move |picked| {
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

/// Whether `event` is the app ending. Quitting from the macOS menu (Cmd+Q)
/// ends the app without any window event, so the session is closed here as
/// well; asking to exit and the exit itself both count (closing twice is
/// harmless).
fn ends_the_app(event: &RunEvent) -> bool {
    matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit)
}

/// The paths of a drop, if `event` is one: all of them, so that loading can
/// refuse several.
fn dropped_paths(event: &WindowEvent) -> Option<Vec<PathBuf>> {
    match event {
        WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => Some(paths.clone()),
        _ => None,
    }
}

/// Start `job` on a thread of its own named `media`: it reads a file the
/// `media` scheme serves.
fn spawn_media_thread(job: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("media".to_owned())
        .spawn(job)
        .map(drop)
}

/// The `media` scheme: a request is answered by [`media::serve`] on a thread
/// of its own, which reads the file, so the webview's thread never waits.
/// If no thread can be started, or the session is not there yet, the request
/// gets `404`.
fn media_protocol(
    context: UriSchemeContext<'_, Wry>,
    request: tauri::http::Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = context.app_handle().clone();
    media::on_own_thread_or_else(
        (app, request, responder),
        |(app, request, responder)| {
            let response = match app.try_state::<AppState>() {
                Some(state) => media::serve(&*state.shared, &request),
                None => media::not_found().into_http(),
            };
            responder.respond(response);
        },
        spawn_media_thread,
        |(_, _, responder)| responder.respond(media::not_found().into_http()),
    );
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

/// The programs, or the message to show instead of starting when they were
/// not found.
fn tools_or_message(located: Result<Tools, ToolsError>) -> Result<Tools, &'static str> {
    located.map_err(|ToolsError::NotFound| TOOLS_MISSING)
}

/// Tell the user why the app cannot start, in a system message box, and
/// exit with 1 once it is closed. A release build has no terminal for
/// stderr (Windows) or none that anyone sees (an app opened from Finder), so
/// without the box the app would end without a word. No window is opened:
/// the box stands alone.
///
/// The dialog plugin shows a message from the event loop, so the app runs
/// without windows, and a thread of its own waits for the box (blocking
/// there is allowed; on the main thread it is not) and then ends the app.
fn cannot_start(mut context: Context<Wry>, message: &'static str) -> ExitCode {
    eprintln!("video-optimizer: {message}");
    context.config_mut().app.windows.clear();
    let built = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                handle
                    .dialog()
                    .message(message)
                    .title("Video optimizer")
                    .kind(MessageDialogKind::Error)
                    .blocking_show();
                handle.exit(1);
            });
            Ok(())
        })
        .build(context);
    match built {
        Ok(app) => {
            app.run_return(|_, _| {});
        }
        Err(error) => eprintln!("video-optimizer: the message could not be shown: {error}"),
    }
    ExitCode::FAILURE
}

/// Start the window. The app cannot do anything without ffmpeg, so when the
/// programs are not found it says so in a system message box (see
/// [`cannot_start`]) and exits with 1 instead.
pub fn run() -> ExitCode {
    let context = tauri::generate_context!();
    let tools = match tools_or_message(Tools::locate()) {
        Ok(tools) => tools,
        Err(message) => return cannot_start(context, message),
    };
    // The dialog plugin must be registered for its Rust API; its commands
    // stay unreachable, as the window has no dialog permission.
    let built = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("media", media_protocol)
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
        .build(context);
    let app = match built {
        Ok(app) => app,
        Err(error) => {
            eprintln!("video-optimizer: the window could not start: {error}");
            return ExitCode::FAILURE;
        }
    };
    // The app can end without a window event (Cmd+Q on macOS): the session
    // is closed then too, so no ffmpeg or partial file is left behind.
    app.run(|app, event| {
        if ends_the_app(&event) {
            shared(app).close(CLOSE_WAIT);
        }
    });
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};
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

    /// What one event of a run was, and what the two decisions said of it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Seen {
        kind: &'static str,
        closing: bool,
        ends: bool,
    }

    /// Every event Tauri's mock runtime gives the app when its one window is
    /// closed, with [`is_closing`] (for a window event) and [`ends_the_app`]
    /// for each. The mock runtime makes `CloseRequested`, `ExitRequested` and
    /// `Exit` as Tauri does; a test cannot build the first two itself, as
    /// they have private fields.
    fn events_when_the_window_closes() -> Vec<Seen> {
        let app = mock_builder()
            .build(mock_context(noop_assets()))
            .expect("the mock app builds");
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("the mock window builds");

        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        let mut idle_turns = 0;
        app.run(move |_, event| {
            match &event {
                // The mock runtime takes a close only while it runs.
                RunEvent::Ready => window.close().expect("the close is sent"),
                // It waits a second per idle turn: end a run that hangs.
                RunEvent::MainEventsCleared => {
                    idle_turns += 1;
                    assert!(idle_turns < 10, "the mock app did not end");
                }
                _ => {}
            }
            let kind = match &event {
                RunEvent::WindowEvent {
                    event: WindowEvent::CloseRequested { .. },
                    ..
                } => "CloseRequested",
                RunEvent::WindowEvent {
                    event: WindowEvent::Destroyed,
                    ..
                } => "Destroyed",
                RunEvent::WindowEvent { .. } => "another window event",
                RunEvent::ExitRequested { .. } => "ExitRequested",
                RunEvent::Exit => "Exit",
                RunEvent::Ready => "Ready",
                _ => "another app event",
            };
            let closing = match &event {
                RunEvent::WindowEvent { event, .. } => is_closing(event),
                _ => false,
            };
            let ends = ends_the_app(&event);
            record.lock().unwrap().push(Seen {
                kind,
                closing,
                ends,
            });
        });
        seen.lock().unwrap().clone()
    }

    /// The one event of `kind` in `seen`.
    fn the_one(seen: &[Seen], kind: &str) -> Seen {
        let found: Vec<&Seen> = seen.iter().filter(|s| s.kind == kind).collect();
        assert_eq!(found.len(), 1, "one {kind} in {seen:?}");
        *found[0]
    }

    #[test]
    fn a_close_request_and_a_destroyed_window_are_closing() {
        let seen = events_when_the_window_closes();
        assert!(the_one(&seen, "CloseRequested").closing, "{seen:?}");
        assert!(is_closing(&WindowEvent::Destroyed));
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
    fn asking_to_exit_and_the_exit_both_close_the_session() {
        // Cmd+Q on macOS ends the app without a window event: the session is
        // closed on the app's own events.
        let seen = events_when_the_window_closes();
        assert!(the_one(&seen, "ExitRequested").ends, "{seen:?}");
        assert!(the_one(&seen, "Exit").ends, "{seen:?}");
    }

    #[test]
    fn other_app_events_do_not_close_the_session() {
        for event in [
            RunEvent::Ready,
            RunEvent::Resumed,
            RunEvent::MainEventsCleared,
        ] {
            assert!(!ends_the_app(&event), "{event:?}");
        }
        // A window asking to close is the window's to handle
        // (`on_window_event`); the app is not ending yet.
        let seen = events_when_the_window_closes();
        assert!(!the_one(&seen, "CloseRequested").ends, "{seen:?}");
        assert!(!the_one(&seen, "Ready").ends, "{seen:?}");
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

    /// A new file dialog of a mock app that has the dialog plugin, as its
    /// `Debug` text from its `filters` on: the builder's fields are private,
    /// and `Debug` shows them (before them comes the whole app handle).
    fn dialog_text(set_up: impl FnOnce(FileDialogBuilder<MockRuntime>) -> String) -> String {
        let app = mock_builder()
            .plugin(tauri_plugin_dialog::init())
            .build(mock_context(noop_assets()))
            .expect("the mock app builds");
        let text = set_up(app.dialog().file());
        let start = text
            .find("filters: ")
            .expect("the builder shows its filters");
        text[start..].to_owned()
    }

    #[test]
    fn the_file_chooser_offers_exactly_the_six_video_extensions_as_videos() {
        let chooser = dialog_text(|dialog| format!("{:?}", video_chooser(dialog)));
        assert!(
            chooser.contains(
                r#"filters: [Filter { name: "Videos", extensions: ["mp4", "mov", "m4v", "mkv", "webm", "avi"] }]"#
            ),
            "{chooser}"
        );
        assert!(
            chooser.contains(r#"title: Some("Choose a video")"#),
            "{chooser}"
        );
    }

    #[test]
    fn the_file_chooser_adds_one_filter_to_a_dialog_that_has_none() {
        // A dialog that is not the video chooser has no filter and no title,
        // so the text above comes from `video_chooser`.
        let plain = dialog_text(|dialog| format!("{dialog:?}"));
        assert!(plain.contains("filters: []"), "{plain}");
        assert!(plain.contains("title: None"), "{plain}");
        let chooser = dialog_text(|dialog| format!("{:?}", video_chooser(dialog)));
        assert_eq!(chooser.matches("Filter {").count(), 1, "{chooser}");
    }

    #[test]
    fn missing_programs_give_the_message_to_show() {
        assert_eq!(
            tools_or_message(Err(ToolsError::NotFound)),
            Err(
                "Video optimizer cannot start: ffmpeg and ffprobe were not found beside the app. \
                 Reinstall it."
            )
        );
    }

    #[test]
    fn found_programs_start_the_app_with_no_message() {
        let tools = Tools {
            ffmpeg: PathBuf::from("/app/ffmpeg"),
            ffprobe: PathBuf::from("/app/ffprobe"),
        };
        assert_eq!(tools_or_message(Ok(tools.clone())), Ok(tools));
    }
}
