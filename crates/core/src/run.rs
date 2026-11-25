//! Running ffmpeg: progress to the caller, a cancel that kills and reaps the
//! process, and the exit code and last error line when it fails.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read};
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use crate::progress::parse_out_time;
use crate::tools::{Tools, command};

/// How often a run that hears nothing from ffmpeg looks at the cancel flag.
const CANCEL_CHECK: Duration = Duration::from_millis(100);

/// The most characters of ffmpeg's last error line that a failure carries.
const MAX_REASON_CHARS: usize = 300;

/// A cancel request, shared by clones: one side calls [`cancel`], the running
/// side looks at [`is_cancelled`].
///
/// [`cancel`]: CancelFlag::cancel
/// [`is_cancelled`]: CancelFlag::is_cancelled
#[derive(Clone, Debug, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> CancelFlag {
        CancelFlag::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Why a run did not finish.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// ffmpeg could not be started.
    #[error("ffmpeg could not be started: {0}")]
    Start(io::Error),
    /// The cancel flag was set; ffmpeg was killed and reaped.
    #[error("cancelled")]
    Cancelled,
    /// ffmpeg ran and exited with an error. `code` is `None` when a signal
    /// ended it. `reason` is its last non-empty line on stderr, cut at 300
    /// characters (empty when it printed nothing).
    #[error("ffmpeg failed: {reason}")]
    Failed { code: Option<i32>, reason: String },
}

/// Run ffmpeg with `args`, and return what it printed on stderr.
///
/// Every `out_time_us` line on its standard output (`-progress pipe:1`) is
/// passed to `on_time` on the calling thread. Standard error is read to the
/// end on a second thread, so a full pipe never blocks ffmpeg, and standard
/// output is read on a third that hands the lines over, so the calling thread
/// can look at `cancel` every 100 ms even when ffmpeg is quiet, and after each
/// line. When `cancel` is set the process is killed and reaped before this
/// returns [`RunError::Cancelled`]. A cancel that comes after ffmpeg has
/// finished is too late: the result is the run's own. Any file ffmpeg was
/// writing when it was cancelled is left for the caller to remove.
pub fn run_ffmpeg(
    tools: &Tools,
    args: &[OsString],
    cancel: &CancelFlag,
    on_time: &mut dyn FnMut(u64),
) -> Result<String, RunError> {
    let mut child = command(&tools.ffmpeg)
        .args(args)
        .spawn()
        .map_err(RunError::Start)?;
    // `command` pipes both; they are taken here, so the child holds the only
    // other ends.
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");

    let stderr_reader = thread::spawn(move || read_all_lossy(stderr));
    let (lines, from_stdout) = mpsc::channel::<String>();
    let stdout_reader = thread::spawn(move || {
        let mut stdout = BufReader::new(stdout);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match stdout.read_until(b'\n', &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            // A receiver that is gone means the run is over.
            if lines
                .send(String::from_utf8_lossy(&buffer).into_owned())
                .is_err()
            {
                break;
            }
        }
    });

    // Until the end of stdout (ffmpeg has exited) or a cancel.
    let mut was_cancelled = false;
    loop {
        if cancel.is_cancelled() {
            was_cancelled = true;
            break;
        }
        match from_stdout.recv_timeout(CANCEL_CHECK) {
            Ok(line) => {
                if let Some(out_us) = parse_out_time(&line) {
                    on_time(out_us);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    if was_cancelled {
        kill_and_reap(&mut child);
        // The pipes are closed now, so both readers end.
        drop(from_stdout);
        let _ = stdout_reader.join();
        let _ = stderr_reader.join();
        return Err(RunError::Cancelled);
    }

    // `wait` fails only when the OS cannot say; then there is no exit code.
    let status = child.wait().map_err(|error| RunError::Failed {
        code: None,
        reason: error.to_string(),
    })?;
    let _ = stdout_reader.join();
    let stderr = stderr_reader.join().unwrap_or_default();
    if status.success() {
        return Ok(stderr);
    }
    Err(RunError::Failed {
        code: status.code(),
        reason: last_line(&stderr),
    })
}

/// Kill `child` and wait for it, so no zombie is left. Both can fail only when
/// the child is already gone.
fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// All of `reader`, as text (bytes that are not UTF-8 become `�`).
fn read_all_lossy(mut reader: impl Read) -> String {
    let mut bytes = Vec::new();
    let _ = reader.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The last line of `text` that holds more than white space, cut at
/// [`MAX_REASON_CHARS`] characters; empty when there is none.
fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(MAX_REASON_CHARS).collect())
        .unwrap_or_default()
}
