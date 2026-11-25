//! `Tools::locate()` run for real, on the real process environment: the three
//! places it looks, with the variable and `PATH` set the way a user's would be.
//!
//! The environment is process-wide, so everything that changes it is in this
//! one test, in this one test binary: nothing else runs beside it.

use std::ffi::OsStr;
use std::fs;

use tempfile::TempDir;
use video_optimizer_core::tools::{Tools, ToolsError};

const FFMPEG: &str = if cfg!(windows) {
    "ffmpeg.exe"
} else {
    "ffmpeg"
};
const FFPROBE: &str = if cfg!(windows) {
    "ffprobe.exe"
} else {
    "ffprobe"
};
const TOOLS_DIR_VAR: &str = "VIDEO_OPTIMIZER_TOOLS_DIR";

fn folder_with_both() -> TempDir {
    let dir = TempDir::new().unwrap();
    for name in [FFMPEG, FFPROBE] {
        fs::write(dir.path().join(name), b"").unwrap();
    }
    dir
}

/// Set an environment variable.
fn set_var(name: &str, value: &OsStr) {
    // SAFETY: `set_var` is unsafe in edition 2024 only because another thread
    // may read the environment at the same moment. This is the only test in
    // this test binary, so the harness's one test thread is the only thread
    // that runs and nothing reads the environment while it changes.
    unsafe { std::env::set_var(name, value) }
}

/// Remove an environment variable (same reasoning as [`set_var`]).
fn remove_var(name: &str) {
    // SAFETY: see `set_var`.
    unsafe { std::env::remove_var(name) }
}

#[test]
fn locate_reads_the_real_environment() {
    // The assumption the rest rests on: this test program's own folder has no
    // tools, so "beside the app" cannot answer instead of the other places.
    let exe = std::env::current_exe().unwrap();
    let exe_dir = exe.parent().expect("the test program is in a folder");
    assert!(
        Tools::beside(exe_dir).is_none(),
        "{} holds ffmpeg and ffprobe",
        exe_dir.display()
    );

    let tools_dir = folder_with_both();
    let empty = TempDir::new().unwrap();
    let expected = Tools {
        ffmpeg: tools_dir.path().join(FFMPEG),
        ffprobe: tools_dir.path().join(FFPROBE),
    };
    let old_path = std::env::var_os("PATH");
    let old_var = std::env::var_os(TOOLS_DIR_VAR);

    // (a) The variable names the folder; PATH has nothing.
    set_var(TOOLS_DIR_VAR, tools_dir.path().as_os_str());
    set_var("PATH", empty.path().as_os_str());
    let from_variable = Tools::locate();

    // (b) No variable; the folder is on PATH.
    remove_var(TOOLS_DIR_VAR);
    set_var("PATH", tools_dir.path().as_os_str());
    let from_path = Tools::locate();

    // (c) A variable naming a folder without the tools, and an empty PATH.
    set_var(TOOLS_DIR_VAR, empty.path().as_os_str());
    set_var("PATH", empty.path().as_os_str());
    let from_nowhere = Tools::locate();

    // And with the variable gone as well.
    remove_var(TOOLS_DIR_VAR);
    let without_variable = Tools::locate();

    // Put the environment back before any assertion can unwind.
    match old_path {
        Some(path) => set_var("PATH", &path),
        None => remove_var("PATH"),
    }
    if let Some(value) = old_var {
        set_var(TOOLS_DIR_VAR, &value);
    }

    assert_eq!(from_variable, Ok(expected.clone()));
    assert_eq!(from_path, Ok(expected));
    assert_eq!(from_nowhere, Err(ToolsError::NotFound));
    assert_eq!(without_variable, Err(ToolsError::NotFound));
}
