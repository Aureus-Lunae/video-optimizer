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
//! failed, and then no file is left behind), and 2 when the usage was wrong.
//!
//! Ctrl-C is not handled here. ffmpeg gets the same signal and stops, but the
//! program is ended without running its cleanup, so the hidden
//! `.optimized-*.part` file next to the result stays: `tempfile` removes it
//! only when the program ends normally.

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
    let info = match probe(&tools, &file) {
        Ok(info) => info,
        Err(error) => return failed(&error),
    };

    let mut progress = Progress::new(io::stderr(), io::stderr().is_terminal());
    // Nothing sets the flag: Ctrl-C ends the program, as the top comment says.
    let cancel = CancelFlag::new();
    let saved = save(
        &tools,
        &info,
        settings,
        out.as_deref(),
        &cancel,
        &mut |percent| progress.report(percent),
    );
    progress.finish();
    match saved {
        Ok(outcome) => {
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
        Err(error) => failed(&error),
    }
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
