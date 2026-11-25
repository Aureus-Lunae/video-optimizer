//! `Tools::locate()` run for real, on the real process environment: the three
//! places it looks, with the variable and `PATH` set the way a user's would be.
//!
//! The environment is process-wide, so everything that changes it is in this
//! one test, in this one test binary: nothing else runs beside it. The
//! "beside the app" place is tried in a child process: a copy of this test
//! program in a folder of its own (the same test, which then only reports what
//! `locate()` found).

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

/// Set in the child process only (see [`locate_in_a_copy_of_this_program`]).
const CHILD_VAR: &str = "VIDEO_OPTIMIZER_LOCATE_CHILD";
const FFMPEG_LINE: &str = "located ffmpeg: ";
const FFPROBE_LINE: &str = "located ffprobe: ";

/// What the copy of this program does when the test runs in it: print what
/// `Tools::locate()` finds, then stop. The real test below never gets here.
fn report_located_tools() {
    match Tools::locate() {
        Ok(tools) => {
            println!("{FFMPEG_LINE}{}", tools.ffmpeg.display());
            println!("{FFPROBE_LINE}{}", tools.ffprobe.display());
        }
        Err(error) => println!("located nothing: {error}"),
    }
}

/// Run a copy of this test program from a folder that holds both tools, with
/// the variable and `PATH` naming other folders that also hold both, and
/// return what `Tools::locate()` found in it. The copy sits in its own temp
/// folder: nothing is ever written beside the real test program, whose folder
/// every test of this workspace shares. The child gets its environment from
/// `Command::env`, so this process's own is not touched.
fn locate_in_a_copy_of_this_program() -> (TempDir, Tools) {
    let exe = std::env::current_exe().unwrap();
    let app = folder_with_both();
    let copy = app.path().join(exe.file_name().unwrap());
    // `fs::copy` keeps the permission bits, so the copy is executable.
    fs::copy(&exe, &copy).unwrap();
    let other_variable = folder_with_both();
    let other_path = folder_with_both();

    let output = std::process::Command::new(&copy)
        .args([
            "--exact",
            "locate_reads_the_real_environment",
            "--nocapture",
        ])
        .env(CHILD_VAR, "1")
        .env(TOOLS_DIR_VAR, other_variable.path())
        .env("PATH", other_path.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}{}", {
        String::from_utf8_lossy(&output.stderr)
    });
    let line = |prefix: &str| {
        let found = stdout.lines().find_map(|line| line.strip_prefix(prefix));
        found
            .unwrap_or_else(|| panic!("no {prefix:?} line in the child's output:\n{stdout}"))
            .into()
    };
    let tools = Tools {
        ffmpeg: line(FFMPEG_LINE),
        ffprobe: line(FFPROBE_LINE),
    };
    (app, tools)
}

#[test]
fn locate_reads_the_real_environment() {
    if std::env::var_os(CHILD_VAR).is_some() {
        report_located_tools();
        return;
    }

    // Beside the app, through the real `current_exe()`, wins over a variable
    // and a `PATH` that both name folders with tools.
    let (app, from_the_copy) = locate_in_a_copy_of_this_program();
    assert_eq!(
        from_the_copy,
        Tools {
            ffmpeg: app.path().join(FFMPEG),
            ffprobe: app.path().join(FFPROBE),
        }
    );

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
