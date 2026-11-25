//! The `video-optimizer` command line, run as the built binary on clips made
//! and probed by the real ffmpeg.
//!
//! The tests leave `VIDEO_OPTIMIZER_TOOLS_DIR` and `PATH` as they are, so the
//! binary finds ffmpeg the way the clip maker does (the PATH locally, the
//! folder CI sets on Windows). Only the two tests that need no ffmpeg, or a
//! different one, set them for the one run.
//!
//! Every test that can leave a file behind compares the folder's listing
//! before and after, and every usage error also checks that no
//! `optimized-*` file exists: the clips are valid, so a run that wrongly
//! went ahead would write one.

#[path = "../../core/tests/common/clips.rs"]
mod clips;
#[cfg(unix)]
#[path = "../../core/tests/common/fake_ffmpeg.rs"]
mod fake_ffmpeg;

use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;
use video_optimizer_core::probe::probe;
use video_optimizer_core::tools::Tools;

use clips::Clip;

/// The built `video-optimizer`, with no stdin, started in `dir`.
fn command(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_video-optimizer"));
    command.current_dir(dir).stdin(Stdio::null());
    command
}

fn run(dir: &Path, args: &[&str]) -> Output {
    command(dir).args(args).output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The names in `dir`, sorted.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn optimized_names(dir: &Path) -> Vec<String> {
    listing(dir)
        .into_iter()
        .filter(|name| name.starts_with("optimized-"))
        .collect()
}

/// A 3 second 640x360 clip at CRF 0, so that CRF 28 is far smaller.
fn high_bitrate(name: &str) -> Clip {
    Clip {
        name: name.into(),
        seconds: 3.0,
        crf: Some(0),
        ..Clip::default()
    }
}

/// A small, quick clip, for the tests that only need a file that would be
/// encoded if the run went ahead.
fn small(name: &str) -> Clip {
    Clip {
        name: name.into(),
        width: 160,
        height: 90,
        seconds: 1.0,
        ..Clip::default()
    }
}

fn size(path: &Path) -> u64 {
    fs::metadata(path).unwrap().len()
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_000_000.0)
}

/// Whether `line` has the shape
/// `optimized-<name>: <n.n> MB -> <n.n> MB (<n>% smaller)`.
fn is_sizes_line(line: &str, name: &str) -> bool {
    let Some(rest) = line.strip_prefix(&format!("{name}: ")) else {
        return false;
    };
    let Some((input, rest)) = rest.split_once(" MB -> ") else {
        return false;
    };
    let Some((output, rest)) = rest.split_once(" MB (") else {
        return false;
    };
    let Some(percent) = rest.strip_suffix("% smaller)") else {
        return false;
    };
    let decimal = |text: &str| {
        text.split_once('.').is_some_and(|(whole, tenth)| {
            !whole.is_empty()
                && whole.bytes().all(|b| b.is_ascii_digit())
                && tenth.len() == 1
                && tenth.bytes().all(|b| b.is_ascii_digit())
        })
    };
    decimal(input)
        && decimal(output)
        && !percent.is_empty()
        && percent.bytes().all(|b| b.is_ascii_digit())
}

/// A usage error: exit code 2, the reason and the usage on stderr, nothing on
/// stdout, and the folder exactly as it was (so no `optimized-*` file, no
/// `.part` file and no folder made for `--out`).
fn assert_usage_error(dir: &Path, args: &[&str], reason: &str) {
    let before = listing(dir);
    let output = run(dir, args);
    let error = stderr(&output);
    assert_eq!(output.status.code(), Some(2), "{args:?}: {error}");
    assert!(error.contains(reason), "{args:?}: {error}");
    assert!(error.contains("usage:"), "{args:?}: {error}");
    assert_eq!(stdout(&output), "", "{args:?}");
    assert_eq!(optimized_names(dir), Vec::<String>::new(), "{args:?}");
    assert_eq!(listing(dir), before, "{args:?}");
}

#[test]
fn the_test_matcher_accepts_the_line_and_rejects_other_shapes() {
    let name = "optimized-a.mp4";
    assert!(is_sizes_line(
        "optimized-a.mp4: 68.3 MB -> 5.8 MB (91% smaller)",
        name
    ));
    for bad in [
        "optimized-a.mp4: 68 MB -> 5.8 MB (91% smaller)",
        "optimized-a.mp4: 68.3 MB -> 5.8 MB (91.5% smaller)",
        "optimized-a.mp4: 68.3 MB -> 5.8 MB (% smaller)",
        "optimized-a.mp4: 68.33 MB -> 5.8 MB (91% smaller)",
        "optimized-b.mp4: 68.3 MB -> 5.8 MB (91% smaller)",
        "optimized-a.mp4: original kept: result was not smaller",
        "",
    ] {
        assert!(!is_sizes_line(bad, name), "{bad:?}");
    }
}

// --- the exit codes ----------------------------------------------------------

#[test]
fn optimizes_a_clip() {
    let dir = TempDir::new().unwrap();
    let input = clips::make(dir.path(), high_bitrate("a.mp4"));
    let output = run(dir.path(), &["a.mp4"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let result = dir.path().join("optimized-a.mp4");
    assert!(result.is_file());
    assert_eq!(listing(dir.path()), ["a.mp4", "optimized-a.mp4"]);

    // One line on stdout, of the documented shape, with the real sizes.
    let text = stdout(&output);
    assert_eq!(text.lines().count(), 1, "{text:?}");
    let line = text.trim_end_matches('\n');
    assert!(is_sizes_line(line, "optimized-a.mp4"), "{line:?}");
    let (before, after) = (size(&input), size(&result));
    assert!(after < before, "the clip is far smaller at CRF 28");
    let percent = ((1.0 - after as f64 / before as f64) * 100.0).round();
    assert_eq!(
        line,
        format!(
            "optimized-a.mp4: {} MB -> {} MB ({percent}% smaller)",
            megabytes(before),
            megabytes(after)
        )
    );
    // Standard error is not a terminal here: no progress, no noise.
    assert_eq!(stderr(&output), "");
}

#[test]
fn keeps_the_original_and_exits_0_when_the_result_is_not_smaller() {
    let dir = TempDir::new().unwrap();
    // A coarse source: CRF 20 makes it bigger.
    let input = clips::make(
        dir.path(),
        Clip {
            name: "a.mov".into(),
            seconds: 3.0,
            crf: Some(40),
            ..Clip::default()
        },
    );
    let original = fs::read(&input).unwrap();
    let output = run(dir.path(), &["--crf", "20", "a.mov"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "optimized-a.mov: original kept: result was not smaller\n"
    );
    assert_eq!(
        fs::read(dir.path().join("optimized-a.mov")).unwrap(),
        original
    );
    assert_eq!(listing(dir.path()), ["a.mov", "optimized-a.mov"]);
}

#[test]
fn refuses_a_text_file() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("notes.txt"), "this is not a video\n").unwrap();
    let output = run(dir.path(), &["notes.txt"]);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let error = stderr(&output);
    assert!(error.contains("could not be read"), "{error}");
    assert_ne!(error.trim(), "");
    assert_eq!(stdout(&output), "");
    assert_eq!(listing(dir.path()), ["notes.txt"]);
}

#[test]
fn refuses_a_file_that_is_not_there() {
    let dir = TempDir::new().unwrap();
    let output = run(dir.path(), &["missing.mp4"]);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stderr(&output).contains("this is not a file"));
    assert_eq!(stdout(&output), "");
    assert_eq!(listing(dir.path()), Vec::<String>::new());
}

#[test]
fn exits_1_and_leaves_nothing_when_ffmpeg_is_not_found() {
    let dir = TempDir::new().unwrap();
    clips::make(dir.path(), small("a.mp4"));
    let before = listing(dir.path());
    // The one run that sets the search path: an empty PATH and no tools
    // folder leave only the folder beside the binary, which has none.
    let output = command(dir.path())
        .env("PATH", "")
        .env_remove("VIDEO_OPTIMIZER_TOOLS_DIR")
        .arg("a.mp4")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stderr(&output).contains("ffmpeg and ffprobe were not found"));
    assert_eq!(stdout(&output), "");
    assert_eq!(listing(dir.path()), before);
}

#[cfg(unix)]
#[test]
fn exits_1_with_ffmpegs_reason_and_leaves_nothing_when_the_encode_fails() {
    let dir = TempDir::new().unwrap();
    clips::make(dir.path(), small("a.mp4"));
    let before = listing(dir.path());

    // A folder with the real ffprobe and an "ffmpeg" that fails.
    let scripts = TempDir::new().unwrap();
    let fake = fake_ffmpeg::script_tools(scripts.path(), "echo boom >&2\nexit 1");
    let real = Tools::locate().expect("ffmpeg and ffprobe are needed for these tests");
    let tools_dir = TempDir::new().unwrap();
    std::os::unix::fs::symlink(&fake.ffmpeg, tools_dir.path().join("ffmpeg")).unwrap();
    std::os::unix::fs::symlink(&real.ffprobe, tools_dir.path().join("ffprobe")).unwrap();

    let output = command(dir.path())
        .env("VIDEO_OPTIMIZER_TOOLS_DIR", tools_dir.path())
        .env("PATH", "")
        .arg("a.mp4")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("ffmpeg failed: boom"),
        "{}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "");
    assert_eq!(listing(dir.path()), before);
}

// --- the options -------------------------------------------------------------

#[test]
fn a_dash_name_after_double_dash_is_a_file() {
    let dir = TempDir::new().unwrap();
    clips::make(dir.path(), small("-x.mp4"));
    let output = run(dir.path(), &["--", "-x.mp4"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(dir.path().join("optimized--x.mp4").is_file());
    let text = stdout(&output);
    assert!(
        is_sizes_line(text.trim_end(), "optimized--x.mp4")
            || text == "optimized--x.mp4: original kept: result was not smaller\n",
        "{text:?}"
    );
}

#[test]
fn a_dash_name_without_double_dash_is_an_unknown_option() {
    let dir = TempDir::new().unwrap();
    clips::make(dir.path(), small("-x.mp4"));
    assert_usage_error(dir.path(), &["-x.mp4"], "-x.mp4");
}

#[test]
fn rejects_crf_19_37_28_5_minus_1_and_text() {
    let dir = TempDir::new().unwrap();
    clips::make(dir.path(), small("a.mp4"));
    for crf in ["19", "37", "28.5", "-1", "0", "51", "abc", ""] {
        assert_usage_error(
            dir.path(),
            &["--crf", crf, "a.mp4"],
            "CRF must be a whole number from 20 to 36",
        );
    }
}

#[test]
fn crf_20_and_36_are_accepted_and_change_the_size() {
    let dir = TempDir::new().unwrap();
    clips::make(dir.path(), high_bitrate("a.mp4"));
    let low = TempDir::new().unwrap();
    let high = TempDir::new().unwrap();
    let low_text = low.path().to_str().unwrap();
    let high_text = high.path().to_str().unwrap();
    let a = run(dir.path(), &["--crf", "20", "--out", low_text, "a.mp4"]);
    let b = run(dir.path(), &["--crf", "36", "--out", high_text, "a.mp4"]);

    assert_eq!(a.status.code(), Some(0), "{}", stderr(&a));
    assert_eq!(b.status.code(), Some(0), "{}", stderr(&b));
    let (low_size, high_size) = (
        size(&low.path().join("optimized-a.mp4")),
        size(&high.path().join("optimized-a.mp4")),
    );
    assert!(
        high_size < low_size,
        "CRF 36 ({high_size}) is smaller than CRF 20 ({low_size})"
    );
    // `--out` put the results in the folders, and none next to the original.
    assert_eq!(listing(dir.path()), ["a.mp4"]);
}

#[test]
fn no_audio_removes_the_audio_and_the_default_keeps_it() {
    let dir = TempDir::new().unwrap();
    clips::make(
        dir.path(),
        Clip {
            audio: true,
            ..high_bitrate("a.mp4")
        },
    );
    let without = TempDir::new().unwrap();
    let with = TempDir::new().unwrap();
    let without_text = without.path().to_str().unwrap();
    let with_text = with.path().to_str().unwrap();
    let a = run(dir.path(), &["--no-audio", "--out", without_text, "a.mp4"]);
    let b = run(dir.path(), &["--out", with_text, "a.mp4"]);
    assert_eq!(a.status.code(), Some(0), "{}", stderr(&a));
    assert_eq!(b.status.code(), Some(0), "{}", stderr(&b));

    let tools = Tools::locate().unwrap();
    let removed = probe(&tools, &without.path().join("optimized-a.mp4")).unwrap();
    let kept = probe(&tools, &with.path().join("optimized-a.mp4")).unwrap();
    assert!(removed.audio.is_none());
    assert!(kept.audio.is_some());
}

#[test]
fn a_taken_name_gets_a_number_and_the_taken_file_is_not_touched() {
    let dir = TempDir::new().unwrap();
    clips::make(dir.path(), high_bitrate("a.mp4"));
    fs::write(dir.path().join("optimized-a.mp4"), b"known").unwrap();
    let output = run(dir.path(), &["a.mp4"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        fs::read(dir.path().join("optimized-a.mp4")).unwrap(),
        b"known"
    );
    assert!(dir.path().join("optimized-a-1.mp4").is_file());
    assert!(stdout(&output).starts_with("optimized-a-1.mp4: "));
}

// --- usage errors ------------------------------------------------------------

#[test]
fn refuses_two_files() {
    let dir = TempDir::new().unwrap();
    clips::make(dir.path(), small("a.mp4"));
    clips::make(dir.path(), small("b.mp4"));
    assert_usage_error(dir.path(), &["a.mp4", "b.mp4"], "One video at a time");
    assert_usage_error(dir.path(), &["--", "a.mp4", "b.mp4"], "One video at a time");
}

#[test]
fn refuses_no_file() {
    let dir = TempDir::new().unwrap();
    clips::make(dir.path(), small("a.mp4"));
    assert_usage_error(dir.path(), &[], "no video");
    assert_usage_error(dir.path(), &["--no-audio"], "no video");
}

#[test]
fn refuses_a_missing_out_folder() {
    let dir = TempDir::new().unwrap();
    clips::make(dir.path(), small("a.mp4"));
    assert_usage_error(dir.path(), &["--out", "nope", "a.mp4"], "nope");
    assert!(!dir.path().join("nope").exists());
}

#[test]
fn refuses_an_unknown_option_and_an_option_without_its_value() {
    let dir = TempDir::new().unwrap();
    clips::make(dir.path(), small("a.mp4"));
    assert_usage_error(dir.path(), &["--bogus", "a.mp4"], "--bogus");
    assert_usage_error(dir.path(), &["a.mp4", "--crf"], "--crf");
    assert_usage_error(dir.path(), &["a.mp4", "--out"], "--out");
}
