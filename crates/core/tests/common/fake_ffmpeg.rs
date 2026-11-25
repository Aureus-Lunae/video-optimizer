//! A shell script standing in for ffmpeg, for tests that need an exact exit
//! code, a partial output file, a long silence or a log of the argument lists.
//! Unix only: the scripts are `#!/bin/sh`.
//!
//! Shared by every test crate that needs one: each includes this one file with
//! `#[path = "common/fake_ffmpeg.rs"] mod fake_ffmpeg;`. There are no copies.

// Each test crate that includes this file uses only part of it.
#![allow(dead_code)]

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use video_optimizer_core::tools::Tools;

/// Tools whose "ffmpeg" is a shell script with this `body`, in `dir`.
///
/// The script is started once with `--ready` (which it answers before running
/// the body) until the system lets it start. Another test thread that forks
/// while this one has the file open for writing holds a copy of that open
/// handle until it execs, and until then the system refuses to start the
/// script ("text file busy"). Waiting here makes that a pause instead of a
/// failure in the test that starts it.
pub fn script_tools(dir: &Path, body: &str) -> Tools {
    let path = dir.join("fake-ffmpeg");
    std::fs::write(
        &path,
        format!("#!/bin/sh\n[ \"$1\" = --ready ] && exit 0\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut tries = 0;
    loop {
        let started = Command::new(&path)
            .arg("--ready")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match started {
            Ok(status) => {
                assert!(status.success(), "the script does not answer --ready");
                break;
            }
            Err(error) if error.kind() == ErrorKind::ExecutableFileBusy && tries < 200 => {
                tries += 1;
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("the script cannot be started: {error}"),
        }
    }
    Tools {
        ffmpeg: path,
        ffprobe: dir.join("no-ffprobe"),
    }
}
