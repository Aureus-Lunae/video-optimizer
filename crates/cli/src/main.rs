//! `video-optimizer`: re-encode one video to an H.264 MP4 from the command
//! line, with the same quality and audio choices as the window.
//!
//! ```text
//! video-optimizer [--crf <20-36>] [--no-audio] [--out <folder>] [--] <file>
//! ```
//!
//! `--crf` is a whole number from 20 to 36 (28 when it is not given),
//! `--no-audio` drops the sound, `--out` names an existing folder for the
//! result instead of the original's own, and `--` lets a file named `-x.mp4`
//! be given. The result is `optimized-<name>.mp4`, or the first free
//! `optimized-<name>-1.mp4` and so on: nothing is ever overwritten.
//!
//! Prints one line to stdout: `optimized-a.mp4: 68.3 MB -> 5.8 MB (91%
//! smaller)`, or `optimized-a.mov: original kept: result was not smaller`. The
//! percent of the encode goes to stderr as `\r42%`, and only when stderr is a
//! terminal. The exit code is 0 when it is done (also when the original was
//! kept), 1 when the video failed (it could not be read, or the encode
//! failed, and then no file is left behind), 2 when the usage was wrong, and
//! 130 when Ctrl-C stopped it.
//!
//! Ctrl-C sets the save's cancel flag. The save then stops ffmpeg (on Windows
//! ffmpeg runs without a console and does not get the Ctrl-C itself) and
//! deletes its hidden `.optimized-*.part` file, so the folder is as it was;
//! `cancelled` goes to stderr and nothing to stdout. A Ctrl-C that comes after
//! the save has finished changes nothing: the file is kept and its line is
//! printed.

mod args;
mod report;

use std::fmt::Display;
use std::io::{self, IsTerminal};
use std::process::ExitCode;

use video_optimizer_core::probe::probe;
use video_optimizer_core::run::CancelFlag;
use video_optimizer_core::save::save;
use video_optimizer_core::tools::Tools;

use args::{Args, USAGE, UsageError};
use report::{Progress, output_line};

fn main() -> ExitCode {
    let Args {
        file,
        settings,
        out,
    } = match args::parse(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(error) => return usage_error(&error),
    };
    let tools = match Tools::locate() {
        Ok(tools) => tools,
        Err(error) => return failed(&error),
    };
    let cancel = CancelFlag::new();
    on_ctrl_c(&cancel);
    let info = match ending(probe(&tools, &file), cancel.is_cancelled()) {
        Ending::Done(info) => info,
        Ending::Cancelled => return cancelled(),
        Ending::Failed(error) => return failed(&error),
    };

    let mut progress = Progress::new(io::stderr(), io::stderr().is_terminal());
    let saved = save(
        &tools,
        &info,
        settings,
        out.as_deref(),
        &cancel,
        &mut |percent| progress.report(percent),
    );
    progress.finish();
    match ending(saved, cancel.is_cancelled()) {
        Ending::Done(outcome) => {
            println!(
                "{}",
                output_line(
                    &outcome.name,
                    info.bytes,
                    outcome.bytes,
                    outcome.kept_original
                )
            );
            ExitCode::SUCCESS
        }
        Ending::Cancelled => cancelled(),
        Ending::Failed(error) => failed(&error),
    }
}

/// How a step of the run ends.
#[derive(Debug, PartialEq, Eq)]
enum Ending<T, E> {
    Done(T),
    Cancelled,
    Failed(E),
}

/// The ending of a step, given whether Ctrl-C has set the cancel flag. A step
/// that failed after Ctrl-C was cancelled: ffmpeg was stopped by the flag, or
/// (in a terminal, where the whole process group gets the signal) ended by
/// the Ctrl-C itself before the flag was looked at. A step that finished is
/// done, even when Ctrl-C came after it.
fn ending<T, E>(result: Result<T, E>, cancelled: bool) -> Ending<T, E> {
    match result {
        Ok(value) => Ending::Done(value),
        Err(_) if cancelled => Ending::Cancelled,
        Err(error) => Ending::Failed(error),
    }
}

/// Make Ctrl-C set `cancel` instead of ending the program. When the handler
/// cannot be set (the system refuses it), Ctrl-C keeps its default, which
/// ends the program, and the run goes on.
fn on_ctrl_c(cancel: &CancelFlag) {
    let cancel = cancel.clone();
    let _ = ctrlc::set_handler(move || cancel.cancel());
}

/// Say that Ctrl-C stopped the run, and exit with 130 (128 + SIGINT).
fn cancelled() -> ExitCode {
    eprintln!("video-optimizer: cancelled");
    ExitCode::from(130)
}

/// Print the reason and the usage, and exit with 2.
fn usage_error(error: &UsageError) -> ExitCode {
    eprintln!("video-optimizer: {error}\n{USAGE}");
    ExitCode::from(2)
}

/// Print why the video failed, and exit with 1.
fn failed(error: &dyn Display) -> ExitCode {
    eprintln!("video-optimizer: {error}");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_that_failed_after_ctrl_c_is_cancelled() {
        assert_eq!(
            ending::<u8, &str>(Err("ffmpeg failed: signal 2"), true),
            Ending::Cancelled
        );
        assert_eq!(
            ending::<u8, &str>(Err("cancelled"), true),
            Ending::Cancelled
        );
    }

    #[test]
    fn a_finished_step_and_a_failure_without_ctrl_c_are_not_cancelled() {
        // A save that finished before Ctrl-C was seen has written its file.
        assert_eq!(ending::<u8, &str>(Ok(7), true), Ending::Done(7));
        assert_eq!(ending::<u8, &str>(Ok(7), false), Ending::Done(7));
        assert_eq!(
            ending::<u8, &str>(Err("ffmpeg failed: boom"), false),
            Ending::Failed("ffmpeg failed: boom")
        );
    }
}
