//! The before clip, the sample, PSNR and the result figures. The text and
//! number functions run on plain values. The clips are made and measured by
//! the real ffmpeg and the real `probe`. A shell script stands in for ffmpeg
//! only where a test needs a failure, a partial output file, a cancel at an
//! exact moment or a log of the argument lists (Unix only).

#[path = "common/clips.rs"]
mod clips;
#[cfg(unix)]
#[path = "common/fake_ffmpeg.rs"]
mod fake_ffmpeg;

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use video_optimizer_core::args::SAMPLE_SECONDS;
use video_optimizer_core::probe::{VideoInfo, probe};
use video_optimizer_core::run::{CancelFlag, RunError};
use video_optimizer_core::sample::{
    Figures, SampleResult, figures, make_before, make_sample, parse_psnr, span,
};
use video_optimizer_core::settings::{Audio, Crf, Settings};
use video_optimizer_core::tools::{Tools, command, file_arg};

use clips::{Clip, Codec};
#[cfg(unix)]
use fake_ffmpeg::script_tools;

fn tools() -> Tools {
    Tools::locate().expect("ffmpeg and ffprobe are needed for these tests")
}

/// The real `probe` of a clip.
fn probed(path: &Path) -> VideoInfo {
    probe(&tools(), path).expect("the clip probes")
}

/// Make `clip` in `dir` and probe it.
fn made(dir: &Path, clip: Clip) -> VideoInfo {
    probed(&clips::make(dir, clip))
}

fn clip(name: &str, seconds: f64) -> Clip {
    Clip {
        name: name.into(),
        seconds,
        ..Clip::default()
    }
}

fn settings(crf: i64, audio: Audio) -> Settings {
    Settings {
        crf: Crf::new(crf).unwrap(),
        audio,
    }
}

fn duration_of(path: &Path) -> f64 {
    probed(path).duration.expect("the file has a duration")
}

/// A `VideoInfo` that was never a file: only `figures` and the fake ffmpeg
/// read it.
fn on_paper(bytes: u64, duration: Option<f64>) -> VideoInfo {
    VideoInfo {
        path: PathBuf::from("/videos/clip.mp4"),
        name: "clip.mp4".into(),
        bytes,
        stream_index: 0,
        width: 1920,
        height: 1080,
        fps: Some(25.0),
        duration,
        video_codec: "h264".into(),
        pix_fmt: "yuv420p".into(),
        audio: None,
    }
}

// ---- parse_psnr ----

#[test]
fn parse_psnr_reads_the_average() {
    let stderr =
        "[Parsed_psnr_0 @ 0x55d0c1] PSNR y:43.1 u:47.0 v:47.5 average:42.41 min:39.0 max:46.2\n";
    assert_eq!(parse_psnr(stderr), Some(42.41));
}

#[test]
fn parse_psnr_reads_inf() {
    let stderr = "[Parsed_psnr_0 @ 0x1] PSNR y:inf u:inf v:inf average:inf min:inf max:inf\n";
    assert_eq!(parse_psnr(stderr), Some(f64::INFINITY));
}

#[test]
fn parse_psnr_reads_the_last_psnr_line_among_other_output() {
    let stderr = "ffmpeg version 9.1\n\
        [Parsed_psnr_0 @ 0x1] PSNR y:10.0 u:10.0 v:10.0 average:10.00 min:1 max:2\n\
        frame=  100 fps=0.0 q=-0.0 Lsize=N/A time=00:00:10.00\n\
        [Parsed_psnr_0 @ 0x1] PSNR y:40.0 u:40.0 v:40.0 average:39.50 min:1 max:2\n\
        video:0KiB audio:0KiB\n";
    assert_eq!(parse_psnr(stderr), Some(39.5));
}

#[test]
fn parse_psnr_without_a_psnr_line_is_none() {
    assert_eq!(parse_psnr(""), None);
    assert_eq!(parse_psnr("frame=1 average:42.41\n"), None);
    assert_eq!(parse_psnr("Error opening input file\n"), None);
}

#[test]
fn parse_psnr_rejects_an_average_that_is_not_a_number() {
    for bad in ["average:nan", "average:", "average:abc", "average:-"] {
        let stderr = format!("[Parsed_psnr_0 @ 0x1] PSNR y:1 {bad} min:1\n");
        assert_eq!(parse_psnr(&stderr), None, "{bad}");
    }
}

#[test]
fn parse_psnr_does_not_fall_back_to_an_earlier_line() {
    // The last PSNR line is the one that counts, even when it has no figure.
    let stderr = "[Parsed_psnr_0 @ 0x1] PSNR y:40 average:40.0 min:1\nPSNR failed\n";
    assert_eq!(parse_psnr(stderr), None);
}

// ---- span ----

#[test]
fn the_span_is_the_duration_up_to_ten_seconds() {
    assert_eq!(span(&on_paper(1, Some(36.0))), Some(10.0));
    assert_eq!(span(&on_paper(1, Some(12.0))), Some(SAMPLE_SECONDS));
    assert_eq!(span(&on_paper(1, Some(10.0))), Some(10.0));
}

#[test]
fn a_short_clip_has_a_span_of_its_whole_length() {
    assert_eq!(span(&on_paper(1, Some(4.0))), Some(4.0));
    assert_eq!(span(&on_paper(1, Some(0.5))), Some(0.5));
}

#[test]
fn without_a_usable_duration_there_is_no_span() {
    assert_eq!(span(&on_paper(1, None)), None);
    for bad in [0.0, -3.0, f64::NAN, f64::INFINITY] {
        assert_eq!(span(&on_paper(1, Some(bad))), None, "{bad}");
    }
}

// ---- figures ----

#[test]
fn figures_match_the_mock_up() {
    let result = figures(&on_paper(96_900_000, Some(36.0)), 11_300_000);
    assert_eq!(
        result,
        Figures {
            original_bytes: Some(26_916_667),
            sample_bytes: 11_300_000,
            smaller_by: Some(58),
            whole_bytes: Some(40_680_000),
        }
    );
}

#[test]
fn figures_without_duration_are_unknown() {
    let result = figures(&on_paper(96_900_000, None), 11_300_000);
    assert_eq!(
        result,
        Figures {
            original_bytes: None,
            sample_bytes: 11_300_000,
            smaller_by: None,
            whole_bytes: None,
        }
    );
}

#[test]
fn a_short_clip_uses_its_whole_length() {
    // 4 s: the sample covers the whole clip, so "original" is the input and
    // "whole file" is the sample.
    let result = figures(&on_paper(8_000_000, Some(4.0)), 2_000_000);
    assert_eq!(result.original_bytes, Some(8_000_000));
    assert_eq!(result.whole_bytes, Some(2_000_000));
    assert_eq!(result.whole_bytes, Some(result.sample_bytes));
    assert_eq!(result.smaller_by, Some(75));
    // The negative: a clip longer than the sample does not.
    let longer = figures(&on_paper(8_000_000, Some(40.0)), 2_000_000);
    assert_eq!(longer.original_bytes, Some(2_000_000));
    assert_eq!(longer.whole_bytes, Some(8_000_000));
    assert_eq!(longer.smaller_by, Some(0));
}

#[test]
fn a_bigger_sample_gives_a_negative_saving() {
    let result = figures(&on_paper(10_000_000, Some(10.0)), 12_500_000);
    assert_eq!(result.original_bytes, Some(10_000_000));
    assert_eq!(result.smaller_by, Some(-25));
    // The negative: a smaller sample does not.
    let smaller = figures(&on_paper(10_000_000, Some(10.0)), 7_500_000);
    assert_eq!(smaller.smaller_by, Some(25));
}

#[test]
fn the_saving_is_rounded_to_a_whole_percent() {
    // 4 949 of 10 000 is 50.51 % smaller, and 4 951 is 50.49 % smaller.
    let up = figures(&on_paper(10_000, Some(10.0)), 4_949);
    assert_eq!(up.smaller_by, Some(51));
    let down = figures(&on_paper(10_000, Some(10.0)), 4_951);
    assert_eq!(down.smaller_by, Some(50));
}

#[test]
fn figures_of_an_empty_input_have_no_saving() {
    // Nothing to divide by, so the saving is unknown instead of infinite.
    let result = figures(&on_paper(0, Some(10.0)), 5_000);
    assert_eq!(result.original_bytes, Some(0));
    assert_eq!(result.smaller_by, None);
    assert_eq!(result.whole_bytes, Some(5_000));
}

// ---- make_before ----

/// Whether the real `probe` finds an audio stream in `path`.
fn has_audio(path: &Path) -> bool {
    probed(path).audio.is_some()
}

#[test]
fn before_of_an_h264_clip_is_a_stream_copy() {
    let dir = TempDir::new().unwrap();
    let info = made(
        dir.path(),
        Clip {
            audio: true,
            ..clip("in.mp4", 12.0)
        },
    );
    assert!(info.audio.is_some(), "the source has audio to leave out");
    let before = make_before(&tools(), &info, dir.path(), &CancelFlag::new()).unwrap();
    assert_eq!(before, dir.path().join("before.mp4"));

    let result = probed(&before);
    assert_eq!(result.video_codec, "h264");
    assert!(!has_audio(&before), "the before clip has no audio");
    assert!((duration_of(&before) - 10.0).abs() < 0.15);

    // Within 5% of a plain `-c copy` cut: it is not re-encoded.
    let cut = dir.path().join("cut.mp4");
    let status = command(&tools().ffmpeg)
        .args(["-v", "error", "-y", "-i"])
        .arg(file_arg(&info.path).unwrap())
        .args(["-t", "10", "-map", "0:v", "-an", "-c", "copy", "-f", "mp4"])
        .arg(file_arg(&cut).unwrap())
        .output()
        .unwrap();
    assert!(status.status.success());
    let copied = fs::metadata(&cut).unwrap().len() as f64;
    let before_bytes = fs::metadata(&before).unwrap().len() as f64;
    assert!(
        (before_bytes - copied).abs() <= copied * 0.05,
        "before {before_bytes} bytes, copy {copied} bytes"
    );
}

#[test]
fn before_of_a_prores_clip_is_h264() {
    let dir = TempDir::new().unwrap();
    let info = made(
        dir.path(),
        Clip {
            codec: Codec::ProRes,
            ..clip("in.mov", 3.0)
        },
    );
    assert!(info.needs_display_copy());
    let before = make_before(&tools(), &info, dir.path(), &CancelFlag::new()).unwrap();
    let result = probed(&before);
    assert_eq!(result.video_codec, "h264");
    assert_eq!(result.pix_fmt, "yuv420p");
    assert!(!result.needs_display_copy());
    assert!(!has_audio(&before));
    // The whole 3 s clip, and the size of the original.
    assert!((duration_of(&before) - 3.0).abs() < 0.15);
    assert_eq!((result.width, result.height), (640, 360));
}

#[test]
fn before_of_an_odd_clip_that_needs_conversion_is_padded() {
    let dir = TempDir::new().unwrap();
    let info = made(
        dir.path(),
        Clip {
            width: 1281,
            height: 721,
            codec: Codec::Mpeg4,
            ..clip("odd.mov", 2.0)
        },
    );
    assert_eq!((info.width, info.height), (1281, 721));
    let before = make_before(&tools(), &info, dir.path(), &CancelFlag::new()).unwrap();
    let result = probed(&before);
    assert_eq!((result.width, result.height), (1282, 722));
}

#[test]
fn a_failed_before_leaves_no_file() {
    let dir = TempDir::new().unwrap();
    let mut info = made(dir.path(), clip("in.mp4", 2.0));
    info.path = dir.path().join("gone.mp4");
    let result = make_before(&tools(), &info, dir.path(), &CancelFlag::new());
    assert!(matches!(result, Err(RunError::Failed { .. })), "{result:?}");
    assert!(!dir.path().join("before.mp4").exists());
}

#[test]
fn a_cancelled_before_returns_cancelled_and_leaves_no_file() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), clip("in.mp4", 2.0));
    let cancel = CancelFlag::new();
    cancel.cancel();
    let result = make_before(&tools(), &info, dir.path(), &cancel);
    assert!(matches!(result, Err(RunError::Cancelled)), "{result:?}");
    assert!(!dir.path().join("before.mp4").exists());
}

#[test]
fn a_relative_work_folder_is_refused_before_ffmpeg_starts() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), clip("in.mp4", 2.0));
    // A tool that cannot start: only a refusal before the start gives `Failed`
    // (a start would give `Start`).
    let nowhere = Tools {
        ffmpeg: dir.path().join("no-ffmpeg"),
        ffprobe: dir.path().join("no-ffprobe"),
    };
    let result = make_before(
        &nowhere,
        &info,
        Path::new("relative-dir"),
        &CancelFlag::new(),
    );
    assert!(matches!(result, Err(RunError::Failed { .. })), "{result:?}");
    let sample = make_sample(
        &nowhere,
        &info,
        Settings::default(),
        &dir.path().join("before.mp4"),
        Path::new("relative-dir"),
        "sample",
        &CancelFlag::new(),
    );
    assert!(matches!(sample, Err(RunError::Failed { .. })), "{sample:?}");
}

// ---- make_sample, real ffmpeg ----

/// `make_sample` for `info` with its before clip made next to it.
fn sample_of(
    dir: &Path,
    info: &VideoInfo,
    settings: Settings,
    name: &str,
) -> Result<SampleResult, RunError> {
    let before = dir.join("before.mp4");
    if !before.exists() {
        make_before(&tools(), info, dir, &CancelFlag::new()).unwrap();
    }
    make_sample(
        &tools(),
        info,
        settings,
        &before,
        dir,
        name,
        &CancelFlag::new(),
    )
}

#[test]
fn sample_of_a_12_s_clip_is_10_s() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), clip("in.mp4", 12.0));
    let result = sample_of(dir.path(), &info, Settings::default(), "sample").unwrap();
    assert_eq!(result.path, dir.path().join("sample.mp4"));
    assert!((duration_of(&result.path) - 10.0).abs() < 0.1);
    assert_eq!(result.bytes, fs::metadata(&result.path).unwrap().len());
    assert!(result.bytes > 0);
}

#[test]
fn sample_of_a_4_s_clip_is_4_s() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), clip("in.mp4", 4.0));
    let result = sample_of(dir.path(), &info, Settings::default(), "sample").unwrap();
    assert!((duration_of(&result.path) - 4.0).abs() < 0.1);
}

#[test]
fn the_sample_has_audio_only_when_audio_is_kept() {
    let dir = TempDir::new().unwrap();
    let info = made(
        dir.path(),
        Clip {
            audio: true,
            ..clip("in.mp4", 2.0)
        },
    );
    let kept = sample_of(dir.path(), &info, settings(28, Audio::Keep), "kept").unwrap();
    assert!(has_audio(&kept.path));
    let removed = sample_of(dir.path(), &info, settings(28, Audio::Remove), "removed").unwrap();
    assert!(!has_audio(&removed.path));
}

#[test]
fn sample_reports_psnr_above_30_db_at_crf_28() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), clip("in.mp4", 3.0));
    let result = sample_of(dir.path(), &info, settings(28, Audio::Keep), "sample").unwrap();
    let psnr = result.psnr.expect("PSNR was measured");
    assert!(psnr > 30.0 && psnr.is_finite(), "{psnr}");
}

#[test]
fn psnr_falls_as_crf_rises() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), clip("in.mp4", 3.0));
    let sharp = sample_of(dir.path(), &info, settings(20, Audio::Keep), "crf20").unwrap();
    let rough = sample_of(dir.path(), &info, settings(36, Audio::Keep), "crf36").unwrap();
    let (sharp, rough) = (sharp.psnr.unwrap(), rough.psnr.unwrap());
    assert!(rough < sharp, "CRF 36 gave {rough}, CRF 20 gave {sharp}");
}

#[test]
fn a_higher_crf_gives_a_smaller_sample() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), clip("in.mp4", 3.0));
    let sharp = sample_of(dir.path(), &info, settings(20, Audio::Keep), "crf20").unwrap();
    let rough = sample_of(dir.path(), &info, settings(36, Audio::Keep), "crf36").unwrap();
    assert!(rough.bytes < sharp.bytes);
}

#[test]
fn sample_of_an_odd_clip_is_compared_against_the_padded_reference() {
    let dir = TempDir::new().unwrap();
    let info = made(
        dir.path(),
        Clip {
            width: 1281,
            height: 721,
            codec: Codec::Mpeg4,
            ..clip("odd.mov", 2.0)
        },
    );
    assert_eq!((info.width, info.height), (1281, 721));
    let result = sample_of(dir.path(), &info, Settings::default(), "sample").unwrap();
    // The sample is padded to 1282x722, and so is the before clip it is
    // compared with: a size mismatch would have made PSNR fail.
    let sample = probed(&result.path);
    assert_eq!((sample.width, sample.height), (1282, 722));
    let psnr = result.psnr.expect("PSNR was measured");
    assert!(psnr.is_finite() && psnr > 20.0, "{psnr}");
    // A before clip that is already padded needs no second reference.
    assert!(!dir.path().join("psnr-ref.mp4").exists());
}

#[test]
fn sample_of_an_even_clip_makes_no_reference() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), clip("in.mp4", 2.0));
    sample_of(dir.path(), &info, Settings::default(), "sample").unwrap();
    assert!(!dir.path().join("psnr-ref.mp4").exists());
}

#[test]
fn a_failed_encode_with_the_real_ffmpeg_leaves_no_sample_file() {
    let dir = TempDir::new().unwrap();
    let mut info = made(dir.path(), clip("in.mp4", 2.0));
    let before = dir.path().join("before.mp4");
    make_before(&tools(), &info, dir.path(), &CancelFlag::new()).unwrap();
    info.path = dir.path().join("gone.mp4");
    let result = make_sample(
        &tools(),
        &info,
        Settings::default(),
        &before,
        dir.path(),
        "sample",
        &CancelFlag::new(),
    );
    assert!(matches!(result, Err(RunError::Failed { .. })), "{result:?}");
    assert!(!dir.path().join("sample.mp4").exists());
}

#[test]
fn a_cancelled_sample_returns_cancelled_and_leaves_no_sample_file() {
    let dir = TempDir::new().unwrap();
    // Long enough at -preset slow that the cancel comes while ffmpeg runs.
    let info = made(
        dir.path(),
        Clip {
            width: 1920,
            height: 1080,
            ..clip("in.mp4", 12.0)
        },
    );
    let before = dir.path().join("before.mp4");
    make_before(&tools(), &info, dir.path(), &CancelFlag::new()).unwrap();
    let sample = dir.path().join("sample.mp4");
    let cancel = CancelFlag::new();

    let (result, saw_file) = thread::scope(|scope| {
        let watcher = scope.spawn(|| {
            let seen = wait_for(&sample, Duration::from_secs(30));
            cancel.cancel();
            seen
        });
        let result = make_sample(
            &tools(),
            &info,
            Settings::default(),
            &before,
            dir.path(),
            "sample",
            &cancel,
        );
        (result, watcher.join().unwrap())
    });

    // The file was there while ffmpeg wrote it, so its absence now is the
    // cleanup, not a cancel that came too early.
    assert!(saw_file, "ffmpeg never created the sample file");
    assert!(matches!(result, Err(RunError::Cancelled)), "{result:?}");
    assert!(!sample.exists());
}

/// Wait up to `limit` for `path` to exist; whether it did.
fn wait_for(path: &Path, limit: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    false
}

// ---- make_sample, a shell script standing in for ffmpeg ----

/// A script for ffmpeg that logs each call's arguments to `log`, then does
/// `encode` for every call that writes a file and `psnr` for the PSNR call.
/// `$out` is the file a writing call was asked to make.
#[cfg(unix)]
fn fake(dir: &Path, log: &Path, encode: &str, psnr: &str) -> Tools {
    script_tools(
        dir,
        &format!(
            "echo \"$*\" >> '{log}'\n\
             last=; for a; do last=$a; done\n\
             out=${{last#file:}}\n\
             case \"$*\" in\n\
             *-lavfi*) {psnr} ;;\n\
             *) {encode} ;;\n\
             esac",
            log = log.display(),
        ),
    )
}

#[cfg(unix)]
const WRITE_THE_FILE: &str = "printf sample > \"$out\"";

#[cfg(unix)]
const A_PSNR_LINE: &str =
    "echo '[Parsed_psnr_0 @ 0x1] PSNR y:40 u:40 v:40 average:41.5 min:30 max:50' >&2";

/// A work folder with a fake ffmpeg and a made-up before clip, and the info of
/// a playable clip of `width` x `height`.
#[cfg(unix)]
struct Bench {
    dir: TempDir,
    before: PathBuf,
    log: PathBuf,
    info: VideoInfo,
}

#[cfg(unix)]
impl Bench {
    fn new(width: u32, height: u32) -> Bench {
        let dir = TempDir::new().unwrap();
        let before = dir.path().join("before.mp4");
        fs::write(&before, "before").unwrap();
        let log = dir.path().join("calls.log");
        let info = VideoInfo {
            width,
            height,
            ..on_paper(1_000_000, Some(30.0))
        };
        Bench {
            dir,
            before,
            log,
            info,
        }
    }

    fn sample(&self, tools: &Tools, cancel: &CancelFlag) -> Result<SampleResult, RunError> {
        make_sample(
            tools,
            &self.info,
            Settings::default(),
            &self.before,
            self.dir.path(),
            "sample",
            cancel,
        )
    }

    fn sample_path(&self) -> PathBuf {
        self.dir.path().join("sample.mp4")
    }

    fn reference_path(&self) -> PathBuf {
        self.dir.path().join("psnr-ref.mp4")
    }

    /// The logged argument lists, one string per ffmpeg call.
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

#[cfg(unix)]
#[test]
fn a_failed_encode_removes_the_partial_sample() {
    let bench = Bench::new(1920, 1080);
    let tools = fake(
        bench.dir.path(),
        &bench.log,
        "printf partial > \"$out\"; echo 'x264 exploded' >&2; exit 1",
        A_PSNR_LINE,
    );
    let result = bench.sample(&tools, &CancelFlag::new());
    match result {
        Err(RunError::Failed { reason, .. }) => assert_eq!(reason, "x264 exploded"),
        other => panic!("expected Failed, got {other:?}"),
    }
    assert!(!bench.sample_path().exists());
    // PSNR was never asked for a sample that does not exist.
    assert!(
        bench.calls().iter().all(|call| !call.contains("-lavfi")),
        "{:?}",
        bench.calls()
    );
}

#[cfg(unix)]
#[test]
fn a_successful_fake_encode_keeps_the_sample_and_its_size() {
    // The negative of the two cleanup tests: nothing is removed when nothing
    // fails.
    let bench = Bench::new(1920, 1080);
    let tools = fake(bench.dir.path(), &bench.log, WRITE_THE_FILE, A_PSNR_LINE);
    let result = bench.sample(&tools, &CancelFlag::new()).unwrap();
    assert_eq!(result.path, bench.sample_path());
    assert_eq!(result.bytes, "sample".len() as u64);
    assert_eq!(result.psnr, Some(41.5));
    assert!(bench.sample_path().exists());
}

#[cfg(unix)]
#[test]
fn an_encode_that_exits_cleanly_without_a_file_is_a_failure() {
    let bench = Bench::new(1920, 1080);
    let tools = fake(bench.dir.path(), &bench.log, "true", A_PSNR_LINE);
    let result = bench.sample(&tools, &CancelFlag::new());
    assert!(
        matches!(result, Err(RunError::Failed { code: None, .. })),
        "{result:?}"
    );
    assert!(!bench.sample_path().exists());
}

#[cfg(unix)]
#[test]
fn a_failed_psnr_gives_none_and_keeps_the_sample() {
    let bench = Bench::new(1920, 1080);
    let tools = fake(
        bench.dir.path(),
        &bench.log,
        WRITE_THE_FILE,
        "echo 'Input 0 and 1 are different sizes' >&2; exit 1",
    );
    let result = bench.sample(&tools, &CancelFlag::new()).unwrap();
    assert_eq!(result.psnr, None);
    assert_eq!(result.bytes, "sample".len() as u64);
    assert!(bench.sample_path().exists());
}

#[cfg(unix)]
#[test]
fn a_psnr_run_that_prints_no_figure_gives_none() {
    let bench = Bench::new(1920, 1080);
    let tools = fake(bench.dir.path(), &bench.log, WRITE_THE_FILE, "true");
    let result = bench.sample(&tools, &CancelFlag::new()).unwrap();
    assert_eq!(result.psnr, None);
    assert!(bench.sample_path().exists());
}

#[cfg(unix)]
#[test]
fn a_cancel_during_psnr_returns_cancelled_and_removes_the_sample() {
    let bench = Bench::new(1920, 1080);
    let started = bench.dir.path().join("psnr-started");
    let tools = fake(
        bench.dir.path(),
        &bench.log,
        WRITE_THE_FILE,
        &format!("touch '{}'; exec sleep 30", started.display()),
    );
    let cancel = CancelFlag::new();
    let (result, saw_psnr) = thread::scope(|scope| {
        let watcher = scope.spawn(|| {
            let seen = wait_for(&started, Duration::from_secs(20));
            cancel.cancel();
            seen
        });
        (bench.sample(&tools, &cancel), watcher.join().unwrap())
    });
    // The sample was finished and the PSNR run had begun when the cancel came.
    assert!(saw_psnr);
    assert!(matches!(result, Err(RunError::Cancelled)), "{result:?}");
    assert!(!bench.sample_path().exists());
}

// The reference. An odd-sized H.264 `yuv420p` source cannot be made (its crop
// unit is 2 pixels), so `VideoInfo` says it and the script stands in.

#[cfg(unix)]
#[test]
fn an_odd_playable_source_is_compared_against_a_padded_reference_made_once() {
    let bench = Bench::new(1281, 721);
    assert!(!bench.info.needs_display_copy());
    let tools = fake(bench.dir.path(), &bench.log, WRITE_THE_FILE, A_PSNR_LINE);
    let first = bench.sample(&tools, &CancelFlag::new()).unwrap();
    let second = bench.sample(&tools, &CancelFlag::new()).unwrap();
    assert_eq!(first.psnr, Some(41.5));
    assert_eq!(second.psnr, Some(41.5));

    let calls = bench.calls();
    let reference = bench.reference_path();
    let reference_arg = format!("file:{}", reference.display());
    let before_arg = format!("file:{}", bench.before.display());

    // One call made the reference, with the display copy's arguments: padded,
    // CRF 10, no audio.
    let makers: Vec<&String> = calls
        .iter()
        .filter(|call| call.ends_with(&reference_arg))
        .collect();
    assert_eq!(makers.len(), 1, "{calls:?}");
    assert!(makers[0].contains("-crf 10"), "{}", makers[0]);
    assert!(makers[0].contains("-vf pad=1282:722"), "{}", makers[0]);
    assert!(makers[0].contains(" -an "), "{}", makers[0]);
    assert!(reference.exists());

    // Both PSNR runs compared against the reference, never the odd before clip.
    let psnr_calls: Vec<&String> = calls.iter().filter(|c| c.contains("-lavfi")).collect();
    assert_eq!(psnr_calls.len(), 2, "{calls:?}");
    for call in psnr_calls {
        assert!(call.contains(&format!("-i {reference_arg}")), "{call}");
        assert!(!call.contains(&before_arg), "{call}");
    }
}

#[cfg(unix)]
#[test]
fn an_even_playable_source_is_compared_against_the_before_clip() {
    let bench = Bench::new(1920, 1080);
    let tools = fake(bench.dir.path(), &bench.log, WRITE_THE_FILE, A_PSNR_LINE);
    bench.sample(&tools, &CancelFlag::new()).unwrap();
    let calls = bench.calls();
    let before_arg = format!("file:{}", bench.before.display());
    let psnr_calls: Vec<&String> = calls.iter().filter(|c| c.contains("-lavfi")).collect();
    assert_eq!(psnr_calls.len(), 1, "{calls:?}");
    assert!(psnr_calls[0].contains(&format!("-i {before_arg}")));
    assert!(!bench.reference_path().exists());
    assert_eq!(calls.len(), 2, "an encode and a PSNR run: {calls:?}");
}

#[cfg(unix)]
#[test]
fn a_source_that_needs_a_display_copy_is_compared_against_the_before_clip() {
    // Its before clip was made padded by `before_args`, so it is its own
    // reference even when the source is odd.
    let mut bench = Bench::new(1281, 721);
    bench.info.video_codec = "prores".into();
    bench.info.pix_fmt = "yuv422p10le".into();
    assert!(bench.info.needs_display_copy());
    let tools = fake(bench.dir.path(), &bench.log, WRITE_THE_FILE, A_PSNR_LINE);
    bench.sample(&tools, &CancelFlag::new()).unwrap();
    assert!(!bench.reference_path().exists());
    assert_eq!(bench.calls().len(), 2);
}

#[cfg(unix)]
#[test]
fn a_failed_reference_gives_no_psnr_and_leaves_no_reference_file() {
    let bench = Bench::new(1281, 721);
    let tools = fake(
        bench.dir.path(),
        &bench.log,
        "case \"$out\" in *psnr-ref.mp4) printf partial > \"$out\"; \
         echo 'no reference' >&2; exit 1;; esac; printf sample > \"$out\"",
        A_PSNR_LINE,
    );
    let result = bench.sample(&tools, &CancelFlag::new()).unwrap();
    assert_eq!(result.psnr, None);
    assert!(bench.sample_path().exists());
    assert!(!bench.reference_path().exists());
    // No PSNR run against a reference that does not exist.
    assert!(
        bench.calls().iter().all(|call| !call.contains("-lavfi")),
        "{:?}",
        bench.calls()
    );
    // A later sample tries the reference again rather than trusting a leftover.
    bench.sample(&tools, &CancelFlag::new()).unwrap();
    let tries = bench
        .calls()
        .iter()
        .filter(|call| call.ends_with("psnr-ref.mp4"))
        .count();
    assert_eq!(tries, 2);
}

#[cfg(unix)]
#[test]
fn a_cancel_while_the_reference_is_made_removes_the_reference_and_the_sample() {
    let bench = Bench::new(1281, 721);
    let started = bench.dir.path().join("reference-started");
    let tools = fake(
        bench.dir.path(),
        &bench.log,
        &format!(
            "case \"$out\" in *psnr-ref.mp4) printf partial > \"$out\"; \
             touch '{}'; exec sleep 30;; esac; printf sample > \"$out\"",
            started.display()
        ),
        A_PSNR_LINE,
    );
    let cancel = CancelFlag::new();
    let (result, saw_reference) = thread::scope(|scope| {
        let watcher = scope.spawn(|| {
            let seen = wait_for(&started, Duration::from_secs(20));
            cancel.cancel();
            seen
        });
        (bench.sample(&tools, &cancel), watcher.join().unwrap())
    });
    assert!(saw_reference);
    assert!(matches!(result, Err(RunError::Cancelled)), "{result:?}");
    assert!(!bench.sample_path().exists());
    assert!(!bench.reference_path().exists());
}

// ---- make_before, a shell script standing in for ffmpeg ----

#[cfg(unix)]
#[test]
fn a_failed_before_removes_the_partial_file() {
    let dir = TempDir::new().unwrap();
    let tools = script_tools(
        dir.path(),
        "last=; for a; do last=$a; done; printf partial > \"${last#file:}\"; \
         echo 'muxer failed' >&2; exit 1",
    );
    let info = on_paper(1_000_000, Some(30.0));
    let result = make_before(&tools, &info, dir.path(), &CancelFlag::new());
    match result {
        Err(RunError::Failed { reason, .. }) => assert_eq!(reason, "muxer failed"),
        other => panic!("expected Failed, got {other:?}"),
    }
    assert!(!dir.path().join("before.mp4").exists());
}

#[cfg(unix)]
#[test]
fn a_cancel_while_the_before_clip_is_written_removes_the_file() {
    let dir = TempDir::new().unwrap();
    let tools = script_tools(
        dir.path(),
        "last=; for a; do last=$a; done; printf partial > \"${last#file:}\"; exec sleep 30",
    );
    let info = on_paper(1_000_000, Some(30.0));
    let before = dir.path().join("before.mp4");
    let cancel = CancelFlag::new();
    let (result, saw_file) = thread::scope(|scope| {
        let watcher = scope.spawn(|| {
            let seen = wait_for(&before, Duration::from_secs(20));
            cancel.cancel();
            seen
        });
        (
            make_before(&tools, &info, dir.path(), &cancel),
            watcher.join().unwrap(),
        )
    });
    assert!(saw_file);
    assert!(matches!(result, Err(RunError::Cancelled)), "{result:?}");
    assert!(!before.exists());
}

#[cfg(unix)]
#[test]
fn a_finished_before_keeps_its_file() {
    // The negative of the two cleanup tests above.
    let dir = TempDir::new().unwrap();
    let tools = script_tools(
        dir.path(),
        "last=; for a; do last=$a; done; printf clip > \"${last#file:}\"",
    );
    let info = on_paper(1_000_000, Some(30.0));
    let before = make_before(&tools, &info, dir.path(), &CancelFlag::new()).unwrap();
    assert_eq!(fs::read_to_string(before).unwrap(), "clip");
}
