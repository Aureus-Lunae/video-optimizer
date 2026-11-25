//! `parse_out_time` and `percent` on text, and `run_ffmpeg` run for real:
//! against the real ffmpeg for what ffmpeg itself does (progress, a failing
//! input, cancel), and against small shell scripts that stand in for ffmpeg
//! where the test needs an exact exit code, a long silence or a flood of
//! output (Unix only: they are `#!/bin/sh` scripts).

#[path = "common/clips.rs"]
mod clips;

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use video_optimizer_core::args::{Span, encode_args};
use video_optimizer_core::probe::probe;
use video_optimizer_core::progress::{parse_out_time, percent};
use video_optimizer_core::run::{CancelFlag, RunError, run_ffmpeg};
use video_optimizer_core::settings::Settings;
use video_optimizer_core::tools::{Tools, file_arg};

use clips::Clip;

fn tools() -> Tools {
    Tools::locate().expect("ffmpeg and ffprobe are needed for these tests")
}

fn os(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

// ---- parse_out_time ----

#[test]
fn parses_out_time_us() {
    assert_eq!(parse_out_time("out_time_us=1500000"), Some(1_500_000));
    assert_eq!(parse_out_time("out_time_us=0"), Some(0));
    // The line as it comes off the pipe, with its line end.
    assert_eq!(parse_out_time("out_time_us=42\r\n"), Some(42));
    assert_eq!(parse_out_time("out_time_us=42\n"), Some(42));
}

#[test]
fn ignores_na_negative_and_other_keys() {
    for line in [
        "out_time_us=N/A",
        "out_time_us=-5",
        "out_time_us=+5",
        "out_time_us=",
        "out_time_us=1.5",
        "out_time_us=12abc",
        "out_time_us=99999999999999999999999",
        "out_time_ms=10",
        "out_time=00:00:01.5",
        "progress=end",
        "",
    ] {
        assert_eq!(parse_out_time(line), None, "{line:?}");
    }
}

// ---- percent ----

#[test]
fn percent_runs_0_to_100() {
    assert_eq!(percent(0, Some(10.0)), Some(0));
    assert_eq!(percent(5_000_000, Some(10.0)), Some(50));
    assert_eq!(percent(10_000_000, Some(10.0)), Some(100));
    // Past the end (ffmpeg's last timestamp can overshoot) stays at 100.
    assert_eq!(percent(25_000_000, Some(10.0)), Some(100));
    // Rounded, not cut off: 33.3 and 66.7.
    assert_eq!(percent(1_000_000, Some(3.0)), Some(33));
    assert_eq!(percent(2_000_000, Some(3.0)), Some(67));
    assert_eq!(percent(u64::MAX, Some(0.001)), Some(100));
}

#[test]
fn percent_is_none_without_a_usable_duration() {
    for duration in [
        None,
        Some(0.0),
        Some(-1.0),
        Some(f64::NAN),
        Some(f64::INFINITY),
        Some(f64::NEG_INFINITY),
    ] {
        assert_eq!(percent(5_000_000, duration), None, "{duration:?}");
    }
}

// ---- run_ffmpeg, real ffmpeg ----

#[test]
fn reports_progress_and_succeeds() {
    let dir = TempDir::new().unwrap();
    let clip = clips::make(
        dir.path(),
        Clip {
            name: "in.mp4".into(),
            seconds: 3.0,
            ..Clip::default()
        },
    );
    let tools = tools();
    let info = probe(&tools, &clip).unwrap();
    let output = dir.path().join("out.mp4");
    let args = encode_args(&info, Settings::default(), &output, Span::Whole).unwrap();

    let mut seen = Vec::new();
    let stderr = run_ffmpeg(&tools, &args, &CancelFlag::new(), &mut |us| seen.push(us))
        .expect("the encode succeeds");

    assert!(!seen.is_empty(), "on_time was never called");
    let last = *seen.last().unwrap();
    assert!(last >= 2_000_000, "the last progress was {last} us");
    assert!(output.metadata().unwrap().len() > 0);
    // What ffmpeg printed on stderr comes back (the encoder's own summary).
    assert!(stderr.contains("libx264"), "{stderr}");
}

#[test]
fn a_failing_run_returns_the_exit_code_and_reason() {
    let args = os(&[
        "-hide_banner",
        "-nostdin",
        "-y",
        "-i",
        "file:/nonexistent.mp4",
        "-f",
        "null",
        "-",
    ]);
    let mut calls = 0;
    let result = run_ffmpeg(&tools(), &args, &CancelFlag::new(), &mut |_| calls += 1);
    match result {
        Err(RunError::Failed { code, reason }) => {
            assert!(code.is_some_and(|code| code != 0), "{code:?}");
            // ffmpeg's own wording changes between versions; it says
            // something, on one line.
            assert!(!reason.trim().is_empty(), "{reason:?}");
            assert!(!reason.contains('\n'), "{reason:?}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    assert_eq!(calls, 0);
}

/// A 60 s 1280x720 source at `-preset slow`, to `out`: far longer than the
/// tests wait.
fn long_encode(out: &Path) -> Vec<OsString> {
    let mut args = os(&[
        "-hide_banner",
        "-nostdin",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=1280x720:rate=30:duration=60",
        "-c:v",
        "libx264",
        "-preset",
        "slow",
        "-crf",
        "20",
        "-progress",
        "pipe:1",
        "-nostats",
        "-f",
        "mp4",
    ]);
    args.push(file_arg(out).unwrap());
    args
}

/// The pid of the one process whose command line holds `needle`.
#[cfg(unix)]
fn pid_of(needle: &str) -> u32 {
    let output = std::process::Command::new("pgrep")
        .args(["-f", "--", needle])
        .output()
        .expect("pgrep is needed to find ffmpeg's pid");
    let text = String::from_utf8_lossy(&output.stdout);
    let pids: Vec<u32> = text
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect();
    assert_eq!(pids.len(), 1, "expected one process for {needle}: {text}");
    pids[0]
}

/// Whether process `pid` is gone from the process table: killed and reaped,
/// not even a zombie left. `kill(pid, 0)` sends nothing and only asks whether
/// the pid exists, which a zombie still does.
#[cfg(unix)]
fn process_is_gone(pid: u32) -> bool {
    let pid = libc::pid_t::try_from(pid).expect("pid fits pid_t");
    // SAFETY: signal 0 sends nothing; it only asks whether the pid exists.
    let answer = unsafe { libc::kill(pid, 0) };
    // `EPERM` would mean the process exists, so only `ESRCH` is "gone".
    answer == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[test]
fn cancel_kills_ffmpeg_and_returns_cancelled() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("cancel-out.mp4");
    let args = long_encode(&out);
    let cancel = CancelFlag::new();

    // The second thread cancels after the first progress report.
    let (first_progress, wait_for_it) = mpsc::channel::<()>();
    let canceller = {
        let cancel = cancel.clone();
        thread::spawn(move || {
            wait_for_it.recv().expect("a progress report arrives");
            let at = Instant::now();
            cancel.cancel();
            at
        })
    };

    #[cfg(unix)]
    let mut pid = None;
    let mut first = Some(first_progress);
    let result = run_ffmpeg(&tools(), &args, &cancel, &mut |_| {
        let Some(first_progress) = first.take() else {
            return;
        };
        // ffmpeg is running now: it is in the process table, so the "gone"
        // check below can fail.
        #[cfg(unix)]
        {
            let found = pid_of(out.to_str().unwrap());
            assert!(!process_is_gone(found), "ffmpeg should be running");
            pid = Some(found);
        }
        first_progress.send(()).unwrap();
    });
    let cancelled_at = canceller.join().unwrap();

    assert!(matches!(result, Err(RunError::Cancelled)), "{result:?}");
    assert!(
        cancelled_at.elapsed() < Duration::from_secs(2),
        "run_ffmpeg took {:?} to return after the cancel",
        cancelled_at.elapsed()
    );
    #[cfg(unix)]
    assert!(
        process_is_gone(pid.expect("the first progress report came")),
        "ffmpeg was killed but not reaped"
    );
}

#[test]
fn an_uncancelled_run_finishes() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("short.mp4");
    let mut args = os(&[
        "-hide_banner",
        "-nostdin",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x180:rate=10:duration=1",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-progress",
        "pipe:1",
        "-nostats",
        "-f",
        "mp4",
    ]);
    args.push(file_arg(&out).unwrap());
    let cancel = CancelFlag::new();
    assert!(!cancel.is_cancelled());
    let result = run_ffmpeg(&tools(), &args, &cancel, &mut |_| {});
    assert!(result.is_ok(), "{result:?}");
    assert!(out.is_file());
    assert!(!cancel.is_cancelled());
}

#[test]
fn a_cancel_flag_is_shared_by_its_clones() {
    let flag = CancelFlag::new();
    let clone = flag.clone();
    assert!(!flag.is_cancelled());
    assert!(!clone.is_cancelled());
    clone.cancel();
    assert!(flag.is_cancelled());
    assert!(clone.is_cancelled());
    // A new flag is not cancelled by it.
    assert!(!CancelFlag::new().is_cancelled());
}

#[test]
fn a_program_that_cannot_start_returns_start() {
    let dir = TempDir::new().unwrap();
    let tools = Tools {
        ffmpeg: dir.path().join("no-such-ffmpeg"),
        ffprobe: dir.path().join("no-such-ffprobe"),
    };
    let mut calls = 0;
    let result = run_ffmpeg(&tools, &[], &CancelFlag::new(), &mut |_| calls += 1);
    match result {
        Err(RunError::Start(error)) => assert_eq!(error.kind(), ErrorKind::NotFound),
        other => panic!("expected Start, got {other:?}"),
    }
    assert_eq!(calls, 0);
}

// ---- run_ffmpeg, a shell script standing in for ffmpeg ----

/// Tools whose "ffmpeg" is a shell script with this `body`, in `dir`.
#[cfg(unix)]
fn script_tools(dir: &Path, body: &str) -> Tools {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-ffmpeg");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    Tools {
        ffmpeg: path,
        ffprobe: dir.join("no-ffprobe"),
    }
}

#[cfg(unix)]
#[test]
fn the_exit_code_and_the_last_non_empty_stderr_line_come_back() {
    let dir = TempDir::new().unwrap();
    let tools = script_tools(
        dir.path(),
        "echo first >&2; echo 'the real reason' >&2; echo >&2; echo '   ' >&2; exit 3",
    );
    let result = run_ffmpeg(&tools, &[], &CancelFlag::new(), &mut |_| {});
    match result {
        Err(RunError::Failed { code, reason }) => {
            assert_eq!(code, Some(3));
            assert_eq!(reason, "the real reason");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn a_long_reason_is_cut_at_300_characters() {
    // The last line is 400 two-byte characters: 300 are kept, counted as
    // characters (a cut by bytes would give 150, or split a character).
    let dir = TempDir::new().unwrap();
    let tools = script_tools(
        dir.path(),
        "echo short >&2; printf '%.0sé' $(seq 400) >&2; exit 1",
    );
    let result = run_ffmpeg(&tools, &[], &CancelFlag::new(), &mut |_| {});
    match result {
        Err(RunError::Failed { reason, .. }) => assert_eq!(reason, "é".repeat(300)),
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn a_reason_of_exactly_300_characters_is_kept_whole() {
    let dir = TempDir::new().unwrap();
    let tools = script_tools(dir.path(), "printf '%.0sx' $(seq 300) >&2; exit 2");
    let result = run_ffmpeg(&tools, &[], &CancelFlag::new(), &mut |_| {});
    match result {
        Err(RunError::Failed { code, reason }) => {
            assert_eq!(code, Some(2));
            assert_eq!(reason, "x".repeat(300));
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn a_run_that_prints_nothing_fails_with_an_empty_reason() {
    let dir = TempDir::new().unwrap();
    let tools = script_tools(dir.path(), "exit 1");
    let result = run_ffmpeg(&tools, &[], &CancelFlag::new(), &mut |_| {});
    match result {
        Err(RunError::Failed { code, reason }) => {
            assert_eq!(code, Some(1));
            assert_eq!(reason, "");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn a_run_killed_by_a_signal_has_no_exit_code() {
    let dir = TempDir::new().unwrap();
    let tools = script_tools(dir.path(), "echo 'killed' >&2; kill -9 $$");
    let result = run_ffmpeg(&tools, &[], &CancelFlag::new(), &mut |_| {});
    match result {
        Err(RunError::Failed { code, reason }) => {
            assert_eq!(code, None);
            assert_eq!(reason, "killed");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn only_out_time_us_lines_reach_on_time_and_stderr_comes_back_on_success() {
    let dir = TempDir::new().unwrap();
    let tools = script_tools(
        dir.path(),
        "echo 'out_time_us=N/A'; echo 'frame=1'; echo 'out_time_us=1000000'; \
         printf 'out_time_us=2500000\\r\\n'; printf 'caf\\351\\n'; echo 'progress=end'; \
         echo 'all done' >&2",
    );
    let mut seen = Vec::new();
    let stderr = run_ffmpeg(&tools, &[], &CancelFlag::new(), &mut |us| seen.push(us)).unwrap();
    assert_eq!(seen, vec![1_000_000, 2_500_000]);
    assert_eq!(stderr.trim(), "all done");
}

#[cfg(unix)]
#[test]
fn a_flood_of_stderr_does_not_block_the_run() {
    // 4 MiB on stderr, far more than a pipe holds: it only ends if stderr is
    // read while the run waits.
    let dir = TempDir::new().unwrap();
    let tools = script_tools(
        dir.path(),
        "head -c 4194304 /dev/zero | tr '\\0' 'x' >&2; echo >&2; echo end >&2",
    );
    let (done, result) = mpsc::channel();
    thread::spawn(move || {
        let stderr = run_ffmpeg(&tools, &[], &CancelFlag::new(), &mut |_| {});
        done.send(stderr).unwrap();
    });
    let stderr = result
        .recv_timeout(Duration::from_secs(20))
        .expect("the run did not finish: stderr was not drained")
        .unwrap();
    assert!(stderr.len() > 4_000_000, "{}", stderr.len());
    assert!(stderr.trim_end().ends_with("end"));
}

#[cfg(unix)]
#[test]
fn a_cancel_during_a_long_silence_is_noticed_within_a_fraction_of_a_second() {
    // No line ever arrives: only the 100 ms check can see the cancel.
    let dir = TempDir::new().unwrap();
    let tools = script_tools(dir.path(), "exec sleep 30");
    let cancel = CancelFlag::new();
    let canceller = {
        let cancel = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            let at = Instant::now();
            cancel.cancel();
            at
        })
    };
    let result = run_ffmpeg(&tools, &[], &cancel, &mut |_| {});
    let at = canceller.join().unwrap();
    assert!(matches!(result, Err(RunError::Cancelled)), "{result:?}");
    assert!(at.elapsed() < Duration::from_secs(2), "{:?}", at.elapsed());
}

#[cfg(unix)]
#[test]
fn a_cancel_from_on_time_stops_the_run_at_that_line() {
    // The cancel comes from on_time itself, then more lines follow at once:
    // the run must stop without reporting them.
    let dir = TempDir::new().unwrap();
    let tools = script_tools(
        dir.path(),
        "echo out_time_us=1; echo out_time_us=2; echo out_time_us=3; exec sleep 30",
    );
    let cancel = CancelFlag::new();
    let mut seen = Vec::new();
    let started = Instant::now();
    let result = run_ffmpeg(&tools, &[], &cancel, &mut |us| {
        seen.push(us);
        cancel.cancel();
    });
    assert!(matches!(result, Err(RunError::Cancelled)), "{result:?}");
    assert_eq!(seen, vec![1]);
    assert!(started.elapsed() < Duration::from_secs(5));
}
