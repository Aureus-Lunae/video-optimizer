//! Finding ffmpeg and ffprobe, and starting them in a way that a file's name
//! cannot turn into an option.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The name of the ffmpeg program in a folder on this platform.
const FFMPEG: &str = if cfg!(windows) {
    "ffmpeg.exe"
} else {
    "ffmpeg"
};

/// The name of the ffprobe program in a folder on this platform.
const FFPROBE: &str = if cfg!(windows) {
    "ffprobe.exe"
} else {
    "ffprobe"
};

/// The environment variable that names a folder holding both programs, for
/// tests and CI.
const TOOLS_DIR_VAR: &str = "VIDEO_OPTIMIZER_TOOLS_DIR";

/// Windows' `CREATE_NO_WINDOW` process creation flag.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Why the programs could not be found. The message is shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ToolsError {
    #[error("ffmpeg and ffprobe were not found beside the app or on the PATH")]
    NotFound,
}

/// Why a path was refused as an argument for ffmpeg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// Only an absolute path is passed on: a relative one depends on the
    /// folder the program runs in, and one starting with `-` would be an
    /// option.
    #[error("the path is not absolute")]
    NotAbsolute,
}

/// The two programs, as paths that can be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Tools {
    /// Both programs, if both exist as files in `dir`.
    pub fn beside(dir: &Path) -> Option<Tools> {
        let ffmpeg = dir.join(FFMPEG);
        let ffprobe = dir.join(FFPROBE);
        (ffmpeg.is_file() && ffprobe.is_file()).then_some(Tools { ffmpeg, ffprobe })
    }

    /// Find the programs: beside the running program (the bundled sidecars)
    /// first, then in the folder named by `VIDEO_OPTIMIZER_TOOLS_DIR`, then on
    /// the `PATH`.
    pub fn locate() -> Result<Tools, ToolsError> {
        let exe = std::env::current_exe().ok();
        locate_from(
            exe.as_deref().and_then(Path::parent),
            std::env::var_os(TOOLS_DIR_VAR).as_deref(),
            std::env::var_os("PATH").as_deref(),
        )
    }
}

/// [`Tools::locate`], with the three places to look given. Both programs must
/// come from the same folder, and only an absolute folder is searched: a
/// relative one would be read from the working folder, where a file could be
/// planted.
fn locate_from(
    exe_dir: Option<&Path>,
    env_dir: Option<&OsStr>,
    path: Option<&OsStr>,
) -> Result<Tools, ToolsError> {
    let from_env = env_dir.map(PathBuf::from);
    let on_path = path.into_iter().flat_map(std::env::split_paths);
    exe_dir
        .map(Path::to_path_buf)
        .into_iter()
        .chain(from_env)
        .chain(on_path)
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| Tools::beside(&dir))
        .ok_or(ToolsError::NotFound)
}

/// A command for `program` with no stdin and both outputs piped. On Windows
/// the programs are console programs: without `CREATE_NO_WINDOW`, each start
/// from the window app would flash a console. Both pipes must be read to the
/// end, or the program blocks on a full pipe: [`Command::output`] does.
///
/// Public so the test clip maker (`tests/common/clips.rs`) starts ffmpeg the
/// same way the library does.
pub fn command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// `path` as the argument that names a file to ffmpeg: `file:` followed by the
/// path. The prefix stops ffmpeg reading a name as an option (`-y.mp4`) or as
/// another protocol (`http:`, `concat:`). A relative path is refused.
pub fn file_arg(path: &Path) -> Result<OsString, PathError> {
    if !path.is_absolute() {
        return Err(PathError::NotAbsolute);
    }
    let mut arg = OsString::from("file:");
    arg.push(path);
    Ok(arg)
}

/// `path` made absolute against the working folder, without touching the file
/// system. Not `canonicalize`, which gives `\\?\` paths on Windows.
pub fn absolute(path: &Path) -> io::Result<PathBuf> {
    std::path::absolute(path)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    /// A folder holding empty files with these names.
    fn folder_with(names: &[&str]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for name in names {
            fs::write(dir.path().join(name), b"").unwrap();
        }
        dir
    }

    fn both() -> TempDir {
        folder_with(&[FFMPEG, FFPROBE])
    }

    // ---- file_arg ----

    #[cfg(unix)]
    #[test]
    fn file_arg_prefixes_an_absolute_path() {
        let arg = file_arg(Path::new("/videos/a b ü.mp4")).unwrap();
        assert_eq!(arg, OsString::from("file:/videos/a b ü.mp4"));
    }

    #[cfg(unix)]
    #[test]
    fn file_arg_keeps_a_name_starting_with_a_dash_as_a_path() {
        let arg = file_arg(Path::new("/v/-y.mp4")).unwrap();
        assert_eq!(arg, OsString::from("file:/v/-y.mp4"));
        assert!(!arg.to_string_lossy().starts_with('-'));
    }

    #[cfg(unix)]
    #[test]
    fn file_arg_keeps_a_protocol_like_name_as_a_path() {
        let arg = file_arg(Path::new("/v/concat:x.mp4")).unwrap();
        assert!(arg.to_string_lossy().starts_with("file:/"), "{arg:?}");
        assert_eq!(arg, OsString::from("file:/v/concat:x.mp4"));
    }

    #[test]
    fn file_arg_rejects_a_relative_path() {
        assert_eq!(file_arg(Path::new("a.mp4")), Err(PathError::NotAbsolute));
        assert_eq!(file_arg(Path::new("-i")), Err(PathError::NotAbsolute));
        assert_eq!(file_arg(Path::new("")), Err(PathError::NotAbsolute));
        assert_eq!(file_arg(Path::new("./a.mp4")), Err(PathError::NotAbsolute));
    }

    #[cfg(windows)]
    #[test]
    fn file_arg_keeps_backslashes() {
        let arg = file_arg(Path::new(r"C:\Video's\a.mp4")).unwrap();
        assert_eq!(arg, OsString::from(r"file:C:\Video's\a.mp4"));
    }

    #[test]
    fn a_path_error_says_what_is_wrong() {
        assert_eq!(
            PathError::NotAbsolute.to_string(),
            "the path is not absolute"
        );
    }

    // ---- absolute ----

    #[test]
    fn absolute_leaves_an_absolute_path_alone() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a.mp4");
        assert_eq!(absolute(&path).unwrap(), path);
    }

    #[test]
    fn absolute_puts_the_working_folder_before_a_relative_path() {
        let result = absolute(Path::new("a.mp4")).unwrap();
        assert!(result.is_absolute(), "{result:?}");
        assert_eq!(result, std::env::current_dir().unwrap().join("a.mp4"));
    }

    #[test]
    fn absolute_refuses_an_empty_path() {
        assert!(absolute(Path::new("")).is_err());
    }

    // ---- beside ----

    #[test]
    fn beside_finds_both_tools() {
        let dir = both();
        let tools = Tools::beside(dir.path()).expect("both are there");
        assert_eq!(tools.ffmpeg, dir.path().join(FFMPEG));
        assert_eq!(tools.ffprobe, dir.path().join(FFPROBE));
    }

    #[test]
    fn beside_refuses_when_one_is_missing() {
        let only_ffmpeg = folder_with(&[FFMPEG]);
        assert!(Tools::beside(only_ffmpeg.path()).is_none());
        let only_ffprobe = folder_with(&[FFPROBE]);
        assert!(Tools::beside(only_ffprobe.path()).is_none());
        let neither = folder_with(&[]);
        assert!(Tools::beside(neither.path()).is_none());
    }

    #[test]
    fn beside_refuses_a_folder_with_those_names() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(FFMPEG)).unwrap();
        fs::create_dir(dir.path().join(FFPROBE)).unwrap();
        assert!(Tools::beside(dir.path()).is_none());
    }

    #[test]
    fn beside_refuses_a_folder_that_does_not_exist() {
        let dir = TempDir::new().unwrap();
        assert!(Tools::beside(&dir.path().join("missing")).is_none());
    }

    // ---- locate order ----

    fn path_var(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[test]
    fn locate_prefers_the_folder_beside_the_app() {
        let beside_app = both();
        let env_dir = both();
        let on_path = both();
        let tools = locate_from(
            Some(beside_app.path()),
            Some(env_dir.path().as_os_str()),
            Some(&path_var(&[on_path.path()])),
        )
        .unwrap();
        assert_eq!(tools.ffmpeg, beside_app.path().join(FFMPEG));
    }

    #[test]
    fn locate_uses_the_env_folder_when_nothing_is_beside_the_app() {
        let beside_app = folder_with(&[FFMPEG]);
        let env_dir = both();
        let on_path = both();
        let tools = locate_from(
            Some(beside_app.path()),
            Some(env_dir.path().as_os_str()),
            Some(&path_var(&[on_path.path()])),
        )
        .unwrap();
        assert_eq!(tools.ffprobe, env_dir.path().join(FFPROBE));
    }

    #[test]
    fn locate_uses_the_path_when_the_first_two_have_nothing() {
        let empty = folder_with(&[]);
        let half = folder_with(&[FFPROBE]);
        let on_path = both();
        let tools = locate_from(
            Some(empty.path()),
            Some(half.path().as_os_str()),
            Some(&path_var(&[half.path(), on_path.path()])),
        )
        .unwrap();
        assert_eq!(tools.ffmpeg, on_path.path().join(FFMPEG));
    }

    #[test]
    fn locate_does_not_mix_tools_from_two_folders() {
        let a = folder_with(&[FFMPEG]);
        let b = folder_with(&[FFPROBE]);
        let result = locate_from(None, None, Some(&path_var(&[a.path(), b.path()])));
        assert!(matches!(result, Err(ToolsError::NotFound)), "{result:?}");
    }

    #[test]
    fn locate_skips_a_relative_folder() {
        // A relative folder is searched from the working folder, where a
        // planted ffmpeg could sit. This folder, inside the working folder,
        // has both tools and is named relatively everywhere.
        let dir = TempDir::new_in(".").unwrap();
        for name in [FFMPEG, FFPROBE] {
            fs::write(dir.path().join(name), b"").unwrap();
        }
        let relative = dir.path().file_name().unwrap();
        assert!(Path::new(relative).join(FFMPEG).is_file());
        // Control: the same folder, named absolutely, is found.
        let absolute_dir = absolute(dir.path()).unwrap();
        assert!(locate_from(Some(&absolute_dir), None, None).is_ok());

        let result = locate_from(
            Some(Path::new(relative)),
            Some(relative),
            Some(&path_var(&[Path::new(relative), Path::new("")])),
        );
        assert!(matches!(result, Err(ToolsError::NotFound)), "{result:?}");
    }

    #[test]
    fn locate_says_what_was_not_found() {
        let error = locate_from(None, None, None).expect_err("nothing to find");
        assert!(matches!(error, ToolsError::NotFound));
        assert_eq!(
            error.to_string(),
            "ffmpeg and ffprobe were not found beside the app or on the PATH"
        );
    }

    // ---- command ----

    #[cfg(unix)]
    #[test]
    fn command_pipes_stdout_and_stderr() {
        let mut command = command(Path::new("/bin/sh"));
        command.args(["-c", "echo out; echo err >&2"]);
        let output = command.output().unwrap();
        assert_eq!(output.stdout, b"out\n");
        assert_eq!(output.stderr, b"err\n");
    }

    #[cfg(unix)]
    #[test]
    fn command_gives_the_child_no_stdin_pipe() {
        let mut command = command(Path::new("/bin/sh"));
        command.args(["-c", "true"]);
        let mut child = command.spawn().unwrap();
        assert!(child.stdin.is_none());
        assert!(child.stdout.is_some());
        assert!(child.stderr.is_some());
        child.wait().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn command_reads_stdin_from_the_null_device() {
        let mut command = command(Path::new("/bin/sh"));
        command.args(["-c", "readlink /proc/self/fd/0"]);
        let output = command.output().unwrap();
        assert_eq!(output.stdout, b"/dev/null\n");
    }

    /// Only Windows CI runs this. It only pins the constant to the Windows
    /// SDK's value and shows that the command still starts and hands back
    /// output; it passes just as well without the `creation_flags` call, so it
    /// does not prove that the console is hidden. std has no getter for the
    /// creation flags: that is proven by the manual Windows check.
    #[cfg(windows)]
    #[test]
    fn command_hides_the_console() {
        assert_eq!(
            CREATE_NO_WINDOW,
            windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
        );
        let mut command = command(Path::new("cmd"));
        command.args(["/C", "echo", "started"]);
        let output = command.output().unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("started"));
    }
}
