//! Saving the whole video. The clips are made, saved and measured by the real
//! ffmpeg and the real `probe`. A shell script stands in for ffmpeg only where
//! a test needs an exact exit code, a partial output file, a progress line at
//! an exact value or an output that is not there (Unix only).
//!
//! Every test that can leave a file behind compares the folder's listing
//! before and after.

#[path = "common/clips.rs"]
mod clips;
#[cfg(unix)]
#[path = "common/fake_ffmpeg.rs"]
mod fake_ffmpeg;

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use video_optimizer_core::probe::{VideoInfo, probe};
use video_optimizer_core::run::{CancelFlag, RunError};
use video_optimizer_core::save::{SaveError, SaveOutcome, candidate_name, save};
use video_optimizer_core::settings::{Audio, Crf, Settings};
use video_optimizer_core::tools::Tools;

use clips::{Clip, Codec};
#[cfg(unix)]
use fake_ffmpeg::script_tools;

fn tools() -> Tools {
    Tools::locate().expect("ffmpeg and ffprobe are needed for these tests")
}

/// The real `probe` of a clip.
fn probed(path: &Path) -> VideoInfo {
    probe(&tools(), path).expect("the file probes")
}

/// Make `clip` in `dir` and probe it.
fn made(dir: &Path, clip: Clip) -> VideoInfo {
    probed(&clips::make(dir, clip))
}

fn settings(crf: i64, audio: Audio) -> Settings {
    Settings {
        crf: Crf::new(crf).unwrap(),
        audio,
    }
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

/// A short clip, for the tests that look at the picture or the streams.
fn short(name: &str) -> Clip {
    Clip {
        name: name.into(),
        seconds: 1.0,
        ..Clip::default()
    }
}

/// The names in `dir`, hidden ones too, in order.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Save `info` at CRF 28 with audio kept, with the real ffmpeg.
fn save_default(info: &VideoInfo, out_dir: Option<&Path>) -> Result<SaveOutcome, SaveError> {
    save_with(info, Settings::default(), out_dir)
}

fn save_with(
    info: &VideoInfo,
    settings: Settings,
    out_dir: Option<&Path>,
) -> Result<SaveOutcome, SaveError> {
    save(
        &tools(),
        info,
        settings,
        out_dir,
        &CancelFlag::new(),
        &mut |_| {},
    )
}

/// A `VideoInfo` made by hand, for the tests that never run the real ffmpeg.
fn on_paper(path: &Path, bytes: u64, duration: Option<f64>) -> VideoInfo {
    VideoInfo {
        path: path.to_path_buf(),
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        bytes,
        stream_index: 0,
        width: 640,
        height: 360,
        fps: Some(10.0),
        duration,
        video_codec: "h264".into(),
        pix_fmt: "yuv420p".into(),
        audio: None,
    }
}

/// The top-level boxes of an MP4 file: each one's four-letter type and where
/// it starts. Reads the 64-bit size of a box whose size field is 1, and stops
/// at a box that reaches to the end of the file (size 0) or one that is
/// broken.
fn top_level_boxes(bytes: &[u8]) -> Vec<([u8; 4], usize)> {
    let mut found = Vec::new();
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as u64;
        found.push((bytes[at + 4..at + 8].try_into().unwrap(), at));
        let size = match size {
            0 => break,
            1 if at + 16 <= bytes.len() => {
                u64::from_be_bytes(bytes[at + 8..at + 16].try_into().unwrap())
            }
            1 => break,
            n => n,
        };
        if size < 8 {
            break;
        }
        at += size as usize;
    }
    found
}

/// Where the first box of type `kind` starts.
fn box_start(boxes: &[([u8; 4], usize)], kind: &[u8; 4]) -> Option<usize> {
    boxes.iter().find(|(k, _)| k == kind).map(|(_, at)| *at)
}

/// A box of `size` bytes (header and zeros).
fn fake_box(kind: &[u8; 4], size: u32) -> Vec<u8> {
    let mut bytes = size.to_be_bytes().to_vec();
    bytes.extend_from_slice(kind);
    bytes.resize(size as usize, 0);
    bytes
}

// ---- Names ----

#[test]
fn names_the_first_output() {
    assert_eq!(
        candidate_name(OsStr::new("a"), "mp4", 0),
        OsStr::new("optimized-a.mp4")
    );
    assert_eq!(
        candidate_name(OsStr::new("a"), "mp4", 2),
        OsStr::new("optimized-a-2.mp4")
    );
}

#[test]
fn a_number_is_only_added_from_one_on() {
    // The negative of the first test: n = 0 carries no `-0`.
    assert_ne!(
        candidate_name(OsStr::new("a"), "mp4", 0),
        OsStr::new("optimized-a-0.mp4")
    );
    assert_eq!(
        candidate_name(OsStr::new("a"), "mp4", 1),
        OsStr::new("optimized-a-1.mp4")
    );
}

#[test]
fn keeps_a_name_with_spaces_and_non_ascii() {
    assert_eq!(
        candidate_name(OsStr::new("Showreel V1 é"), "mov", 0),
        OsStr::new("optimized-Showreel V1 é.mov")
    );
    assert_eq!(
        candidate_name(OsStr::new("Showreel V1 é"), "mov", 3),
        OsStr::new("optimized-Showreel V1 é-3.mov")
    );
}

#[cfg(unix)]
#[test]
fn keeps_a_name_that_is_not_utf8() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let stem = OsStr::from_bytes(b"a\xff");
    let name = candidate_name(stem, "mp4", 1);
    assert_eq!(name.into_vec(), b"optimized-a\xff-1.mp4");
}

// ---- The box reader the faststart test relies on ----

#[test]
fn the_box_reader_finds_moov_after_mdat_in_a_file_without_faststart() {
    let mut bytes = fake_box(b"ftyp", 16);
    bytes.extend(fake_box(b"mdat", 40));
    bytes.extend(fake_box(b"moov", 24));
    let boxes = top_level_boxes(&bytes);
    let moov = box_start(&boxes, b"moov").expect("moov is found");
    let mdat = box_start(&boxes, b"mdat").expect("mdat is found");
    assert_eq!((mdat, moov), (16, 56));
    assert!(moov > mdat);
}

#[test]
fn the_box_reader_steps_over_a_box_with_a_64_bit_size() {
    let mut bytes = fake_box(b"ftyp", 16);
    // mdat with a 64-bit size of 32: size field 1, type, then the real size.
    let mut big = 1u32.to_be_bytes().to_vec();
    big.extend_from_slice(b"mdat");
    big.extend_from_slice(&32u64.to_be_bytes());
    big.resize(32, 0);
    bytes.extend(big);
    bytes.extend(fake_box(b"moov", 24));
    let boxes = top_level_boxes(&bytes);
    assert_eq!(box_start(&boxes, b"mdat"), Some(16));
    assert_eq!(box_start(&boxes, b"moov"), Some(48));
}

// ---- A real save ----

#[test]
fn writes_optimized_next_to_the_original() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mp4"));
    let outcome = save_default(&info, None).unwrap();

    assert!(!outcome.kept_original);
    assert_eq!(outcome.path, dir.path().join("optimized-a.mp4"));
    assert_eq!(outcome.name, "optimized-a.mp4");
    let size = fs::metadata(&outcome.path).unwrap().len();
    assert_eq!(outcome.bytes, size);
    assert!(
        size < info.bytes,
        "{size} is not smaller than {}",
        info.bytes
    );
    let out = probed(&outcome.path);
    assert_eq!(out.video_codec, "h264");
    // Nothing else: no `.part` file is left.
    assert_eq!(listing(dir.path()), ["a.mp4", "optimized-a.mp4"]);
}

#[test]
fn the_output_has_moov_before_mdat() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mp4"));
    let outcome = save_default(&info, None).unwrap();

    let boxes = top_level_boxes(&fs::read(&outcome.path).unwrap());
    let moov = box_start(&boxes, b"moov").expect("the file has a moov box");
    let mdat = box_start(&boxes, b"mdat").expect("the file has an mdat box");
    assert!(moov < mdat, "moov at {moov} is after mdat at {mdat}");
}

#[test]
fn the_original_is_byte_identical_afterwards() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mp4"));
    let before = fs::read(&info.path).unwrap();
    save_default(&info, None).unwrap();
    assert_eq!(fs::read(&info.path).unwrap(), before);
}

#[test]
fn a_taken_name_gets_a_number() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mp4"));
    let taken = dir.path().join("optimized-a.mp4");
    let taken_1 = dir.path().join("optimized-a-1.mp4");
    fs::write(&taken, b"first, known bytes").unwrap();
    fs::write(&taken_1, b"second, known bytes").unwrap();

    let outcome = save_default(&info, None).unwrap();

    assert_eq!(outcome.path, dir.path().join("optimized-a-2.mp4"));
    assert_eq!(fs::read(&taken).unwrap(), b"first, known bytes");
    assert_eq!(fs::read(&taken_1).unwrap(), b"second, known bytes");
    assert_eq!(
        listing(dir.path()),
        [
            "a.mp4",
            "optimized-a-1.mp4",
            "optimized-a-2.mp4",
            "optimized-a.mp4"
        ]
    );
}

#[test]
fn the_first_free_name_is_used_when_nothing_is_taken() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mp4"));
    let outcome = save_default(&info, None).unwrap();
    assert_eq!(outcome.path, dir.path().join("optimized-a.mp4"));
    assert!(!dir.path().join("optimized-a-1.mp4").exists());
}

#[test]
fn a_free_number_below_a_taken_one_is_used() {
    // Only `optimized-a-1.mp4` is taken: the plain name is the first free one.
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mp4"));
    fs::write(dir.path().join("optimized-a-1.mp4"), b"known").unwrap();
    let outcome = save_default(&info, None).unwrap();
    assert_eq!(outcome.path, dir.path().join("optimized-a.mp4"));
}

#[test]
fn writes_into_the_chosen_folder() {
    let dir = TempDir::new().unwrap();
    let chosen = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mp4"));
    let outcome = save_default(&info, Some(chosen.path())).unwrap();
    assert_eq!(outcome.path, chosen.path().join("optimized-a.mp4"));
    assert_eq!(listing(chosen.path()), ["optimized-a.mp4"]);
}

#[test]
fn the_originals_folder_gets_no_new_file_when_a_folder_is_chosen() {
    let dir = TempDir::new().unwrap();
    let chosen = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mp4"));
    save_default(&info, Some(chosen.path())).unwrap();
    assert_eq!(listing(dir.path()), ["a.mp4"]);
}

#[test]
fn a_name_is_taken_in_the_chosen_folder_not_in_the_originals() {
    // `optimized-a.mp4` exists next to the original but not in the chosen
    // folder, so the chosen folder gets the plain name.
    let dir = TempDir::new().unwrap();
    let chosen = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mp4"));
    fs::write(dir.path().join("optimized-a.mp4"), b"known").unwrap();
    let outcome = save_default(&info, Some(chosen.path())).unwrap();
    assert_eq!(outcome.path, chosen.path().join("optimized-a.mp4"));
}

#[test]
fn keeps_the_original_when_the_result_is_not_smaller() {
    let dir = TempDir::new().unwrap();
    // A coarse source: CRF 20 makes it bigger.
    let info = made(
        dir.path(),
        Clip {
            name: "a.mov".into(),
            seconds: 3.0,
            crf: Some(40),
            ..Clip::default()
        },
    );
    let original = fs::read(&info.path).unwrap();
    let outcome = save_with(&info, settings(20, Audio::Keep), None).unwrap();

    assert!(outcome.kept_original);
    assert_eq!(outcome.path, dir.path().join("optimized-a.mov"));
    assert_eq!(outcome.name, "optimized-a.mov");
    assert_eq!(fs::read(&outcome.path).unwrap(), original);
    assert_eq!(outcome.bytes, original.len() as u64);
    assert_eq!(listing(dir.path()), ["a.mov", "optimized-a.mov"]);
}

#[test]
fn writes_the_encode_when_it_is_smaller() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mov"));
    let outcome = save_with(&info, settings(28, Audio::Keep), None).unwrap();
    assert!(!outcome.kept_original);
    // The output is an MP4 whatever the input was.
    assert_eq!(outcome.path, dir.path().join("optimized-a.mp4"));
    assert!(outcome.bytes < info.bytes);
}

#[test]
fn keeps_the_size() {
    let dir = TempDir::new().unwrap();
    let info = made(
        dir.path(),
        Clip {
            width: 1280,
            height: 720,
            ..short("big.mp4")
        },
    );
    assert_eq!((info.width, info.height), (1280, 720));
    let out = probed(&save_default(&info, None).unwrap().path);
    assert_eq!((out.width, out.height), (1280, 720));
}

#[test]
fn pads_an_odd_source() {
    let dir = TempDir::new().unwrap();
    let info = made(
        dir.path(),
        Clip {
            width: 1281,
            height: 721,
            codec: Codec::Mpeg4,
            ..short("odd.mov")
        },
    );
    assert_eq!((info.width, info.height), (1281, 721));
    let out = probed(&save_default(&info, None).unwrap().path);
    assert_eq!((out.width, out.height), (1282, 722));
}

#[test]
fn keeps_a_rotated_clip_upright() {
    let dir = TempDir::new().unwrap();
    let info = made(
        dir.path(),
        Clip {
            rotate: Some(90),
            ..short("turned.mp4")
        },
    );
    // Stored 640x360, shown 360x640.
    assert_eq!((info.width, info.height), (360, 640));
    let out = probed(&save_default(&info, None).unwrap().path);
    assert_eq!((out.width, out.height), (360, 640));
}

#[test]
fn remove_audio_gives_no_audio_stream() {
    let dir = TempDir::new().unwrap();
    let info = made(
        dir.path(),
        Clip {
            audio: true,
            ..short("a.mp4")
        },
    );
    assert!(info.audio.is_some());
    let out = probed(
        &save_with(&info, settings(28, Audio::Remove), None)
            .unwrap()
            .path,
    );
    assert!(out.audio.is_none());
}

#[test]
fn keep_audio_gives_an_aac_stream() {
    let dir = TempDir::new().unwrap();
    let info = made(
        dir.path(),
        Clip {
            audio: true,
            ..short("a.mp4")
        },
    );
    let out = probed(
        &save_with(&info, settings(28, Audio::Keep), None)
            .unwrap()
            .path,
    );
    assert_eq!(out.audio.expect("an audio stream").codec, "aac");
}

#[test]
fn keep_audio_on_a_silent_source_still_saves() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), short("a.mp4"));
    assert!(info.audio.is_none());
    let out = probed(
        &save_with(&info, settings(28, Audio::Keep), None)
            .unwrap()
            .path,
    );
    assert!(out.audio.is_none());
}

#[test]
fn a_name_starting_with_a_dash_is_saved() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("-y.mp4"));
    let outcome = save_default(&info, None).unwrap();
    assert_eq!(outcome.path, dir.path().join("optimized--y.mp4"));
    assert_eq!(listing(dir.path()), ["-y.mp4", "optimized--y.mp4"]);
}

// ---- Cancel ----

#[test]
fn cancel_deletes_the_partial_output() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mp4"));
    let before = listing(dir.path());
    let cancel = CancelFlag::new();
    let mut calls = 0;
    let result = save(
        &tools(),
        &info,
        Settings::default(),
        None,
        &cancel,
        &mut |_| {
            calls += 1;
            cancel.cancel();
        },
    );
    assert!(
        matches!(result, Err(SaveError::Run(RunError::Cancelled))),
        "got {result:?}"
    );
    assert!(calls >= 1, "the cancel came from a progress report");
    assert_eq!(listing(dir.path()), before);
}

#[test]
fn a_finished_save_is_not_deleted_by_a_later_cancel() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mp4"));
    let cancel = CancelFlag::new();
    let outcome = save(
        &tools(),
        &info,
        Settings::default(),
        None,
        &cancel,
        &mut |_| {},
    )
    .unwrap();
    cancel.cancel();
    assert!(outcome.path.is_file());
    assert_eq!(listing(dir.path()), ["a.mp4", "optimized-a.mp4"]);
}

// ---- Failures ----

#[cfg(unix)]
const WRITES_PARTIAL: &str = "last=; for a; do last=$a; done; printf partial > \"${last#file:}\"";

/// A folder for the output and a folder for the fake ffmpeg script, so the
/// output folder's listing holds only what a save leaves.
#[cfg(unix)]
fn fake_setup(body: &str) -> (TempDir, TempDir, Tools) {
    let scripts = TempDir::new().unwrap();
    let out = TempDir::new().unwrap();
    let tools = script_tools(scripts.path(), body);
    (scripts, out, tools)
}

#[cfg(unix)]
fn save_fake(
    tools: &Tools,
    info: &VideoInfo,
    out: &Path,
    on_progress: &mut dyn FnMut(Option<u8>),
) -> Result<SaveOutcome, SaveError> {
    save(
        tools,
        info,
        Settings::default(),
        Some(out),
        &CancelFlag::new(),
        on_progress,
    )
}

#[cfg(unix)]
#[test]
fn an_ffmpeg_failure_leaves_nothing() {
    let (_scripts, out, tools) = fake_setup(&format!(
        "{WRITES_PARTIAL}; echo 'muxer failed' >&2; exit 1"
    ));
    let source = TempDir::new().unwrap();
    let original = source.path().join("a.mp4");
    fs::write(&original, vec![0u8; 100]).unwrap();
    let info = on_paper(&original, 100, Some(3.0));
    let before = listing(out.path());

    let result = save_fake(&tools, &info, out.path(), &mut |_| {});

    match result {
        Err(SaveError::Run(RunError::Failed { code, reason })) => {
            assert_eq!(code, Some(1));
            assert_eq!(reason, "muxer failed");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    assert_eq!(listing(out.path()), before);
    assert_eq!(listing(source.path()), ["a.mp4"]);
}

#[cfg(unix)]
#[test]
fn exit_0_returns_done() {
    let (_scripts, out, tools) = fake_setup(WRITES_PARTIAL);
    let source = TempDir::new().unwrap();
    let original = source.path().join("a.mp4");
    fs::write(&original, vec![0u8; 100]).unwrap();
    let info = on_paper(&original, 100, Some(3.0));

    let outcome = save_fake(&tools, &info, out.path(), &mut |_| {}).unwrap();

    assert!(!outcome.kept_original);
    assert_eq!(outcome.bytes, 7);
    assert_eq!(fs::read(&outcome.path).unwrap(), b"partial");
    assert_eq!(listing(out.path()), ["optimized-a.mp4"]);
}

#[test]
fn an_ffmpeg_that_cannot_start_leaves_nothing() {
    let dir = TempDir::new().unwrap();
    let out = TempDir::new().unwrap();
    let tools = Tools {
        ffmpeg: dir.path().join("no-such-ffmpeg"),
        ffprobe: dir.path().join("no-such-ffprobe"),
    };
    let original = dir.path().join("a.mp4");
    fs::write(&original, b"original").unwrap();
    let info = on_paper(&original, 8, Some(3.0));
    let result = save(
        &tools,
        &info,
        Settings::default(),
        Some(out.path()),
        &CancelFlag::new(),
        &mut |_| {},
    );
    assert!(
        matches!(result, Err(SaveError::Run(RunError::Start(_)))),
        "got {result:?}"
    );
    assert!(listing(out.path()).is_empty());
}

#[test]
fn a_relative_original_is_refused_and_leaves_nothing() {
    let out = TempDir::new().unwrap();
    let info = on_paper(Path::new("relative/a.mp4"), 8, Some(3.0));
    let unused = Tools {
        ffmpeg: PathBuf::from("/no/ffmpeg"),
        ffprobe: PathBuf::from("/no/ffprobe"),
    };
    let result = save(
        &unused,
        &info,
        Settings::default(),
        Some(out.path()),
        &CancelFlag::new(),
        &mut |_| {},
    );
    // Refused before ffmpeg is started (a start failure would be `Start`).
    assert!(
        matches!(
            result,
            Err(SaveError::Run(RunError::Failed { code: None, .. }))
        ),
        "got {result:?}"
    );
    assert!(listing(out.path()).is_empty());
}

#[test]
fn an_original_with_no_name_is_an_io_error() {
    // `/x/..` has a folder but no file stem. ffmpeg is never reached: a start
    // failure would be `Run(Start)`.
    let out = TempDir::new().unwrap();
    let info = on_paper(Path::new("/x/.."), 8, Some(3.0));
    let unused = Tools {
        ffmpeg: PathBuf::from("/no/ffmpeg"),
        ffprobe: PathBuf::from("/no/ffprobe"),
    };
    let result = save(
        &unused,
        &info,
        Settings::default(),
        Some(out.path()),
        &CancelFlag::new(),
        &mut |_| {},
    );
    assert!(matches!(result, Err(SaveError::Io(_))), "got {result:?}");
    assert!(listing(out.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn an_unwritable_folder_fails_cleanly() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mp4"));
    let locked = TempDir::new().unwrap();
    fs::set_permissions(locked.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let result = save_default(&info, Some(locked.path()));
    // Unlock first, so a failed assertion still lets the folder be removed.
    fs::set_permissions(locked.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(
            result,
            Err(SaveError::Io(_) | SaveError::Run(RunError::Failed { .. }))
        ),
        "got {result:?}"
    );
    assert!(listing(locked.path()).is_empty());
    assert_eq!(listing(dir.path()), ["a.mp4"]);
}

#[cfg(unix)]
#[test]
fn an_encode_that_leaves_no_file_is_an_io_error() {
    // ffmpeg says it succeeded but the output is gone.
    let (_scripts, out, tools) = fake_setup("last=; for a; do last=$a; done; rm \"${last#file:}\"");
    let source = TempDir::new().unwrap();
    let original = source.path().join("a.mp4");
    fs::write(&original, vec![0u8; 100]).unwrap();
    let info = on_paper(&original, 100, Some(3.0));

    let result = save_fake(&tools, &info, out.path(), &mut |_| {});

    assert!(matches!(result, Err(SaveError::Io(_))), "got {result:?}");
    assert!(listing(out.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn a_failed_copy_of_the_original_leaves_nothing() {
    // The encode (7 bytes) is not smaller than the original (7 bytes), so the
    // original is copied; the original is not there to be read.
    let (_scripts, out, tools) = fake_setup(WRITES_PARTIAL);
    let source = TempDir::new().unwrap();
    let missing = source.path().join("gone.mp4");
    let info = on_paper(&missing, 7, Some(3.0));

    let result = save_fake(&tools, &info, out.path(), &mut |_| {});

    assert!(matches!(result, Err(SaveError::Io(_))), "got {result:?}");
    assert!(listing(out.path()).is_empty());
}

// ---- Which of the two is saved ----

/// Save a fake encode of 7 bytes of an original of `original_bytes` bytes
/// whose file holds `ORIGINL` (7 bytes).
#[cfg(unix)]
fn save_fake_seven(name: &str, original_bytes: u64) -> (TempDir, SaveOutcome) {
    let (_scripts, out, tools) = fake_setup(WRITES_PARTIAL);
    let source = TempDir::new().unwrap();
    let original = source.path().join(name);
    fs::write(&original, b"ORIGINL").unwrap();
    let info = on_paper(&original, original_bytes, Some(3.0));
    let outcome = save_fake(&tools, &info, out.path(), &mut |_| {}).unwrap();
    (out, outcome)
}

#[cfg(unix)]
#[test]
fn a_result_of_the_same_size_counts_as_not_smaller() {
    let (_out, outcome) = save_fake_seven("a.mp4", 7);
    assert!(outcome.kept_original);
    assert_eq!(fs::read(&outcome.path).unwrap(), b"ORIGINL");
}

#[cfg(unix)]
#[test]
fn a_result_one_byte_smaller_is_written() {
    let (_out, outcome) = save_fake_seven("a.mp4", 8);
    assert!(!outcome.kept_original);
    assert_eq!(fs::read(&outcome.path).unwrap(), b"partial");
}

#[cfg(unix)]
#[test]
fn the_kept_original_gets_its_extension_in_lower_case() {
    let (out, outcome) = save_fake_seven("Clip.MOV", 7);
    assert!(outcome.kept_original);
    assert_eq!(outcome.name, "optimized-Clip.mov");
    assert_eq!(listing(out.path()), ["optimized-Clip.mov"]);
}

#[cfg(unix)]
#[test]
fn the_kept_original_without_an_extension_is_an_mp4() {
    let (_out, outcome) = save_fake_seven("clip", 7);
    assert!(outcome.kept_original);
    assert_eq!(outcome.name, "optimized-clip.mp4");
}

#[cfg(unix)]
#[test]
fn an_encode_keeps_the_mp4_extension_whatever_the_original_was() {
    // The negative of the two tests above.
    let (_out, outcome) = save_fake_seven("Clip.MOV", 8);
    assert!(!outcome.kept_original);
    assert_eq!(outcome.name, "optimized-Clip.mp4");
}

// ---- Progress ----

#[test]
fn progress_runs_to_100() {
    let dir = TempDir::new().unwrap();
    let info = made(dir.path(), high_bitrate("a.mp4"));
    let mut seen = Vec::new();
    save(
        &tools(),
        &info,
        Settings::default(),
        None,
        &CancelFlag::new(),
        &mut |p| seen.push(p),
    )
    .unwrap();
    assert!(!seen.is_empty(), "no progress was reported");
    assert_eq!(seen.last(), Some(&Some(100)));
}

#[cfg(unix)]
const REPORTS_TWO_POINTS: &str =
    "echo out_time_us=1000000; echo out_time_us=2000000; echo progress=end;";

#[cfg(unix)]
#[test]
fn progress_is_the_percent_of_the_duration() {
    let (_scripts, out, tools) = fake_setup(&format!("{REPORTS_TWO_POINTS} {WRITES_PARTIAL}"));
    let source = TempDir::new().unwrap();
    let original = source.path().join("a.mp4");
    fs::write(&original, vec![0u8; 100]).unwrap();
    let info = on_paper(&original, 100, Some(4.0));
    let mut seen = Vec::new();
    save_fake(&tools, &info, out.path(), &mut |p| seen.push(p)).unwrap();
    // The two reports, then 100 when ffmpeg has finished.
    assert_eq!(seen, [Some(25), Some(50), Some(100)]);
}

#[cfg(unix)]
#[test]
fn a_failed_save_does_not_report_100() {
    let (_scripts, out, tools) = fake_setup(&format!(
        "echo out_time_us=1000000; {WRITES_PARTIAL}; exit 1"
    ));
    let source = TempDir::new().unwrap();
    let original = source.path().join("a.mp4");
    fs::write(&original, vec![0u8; 100]).unwrap();
    let info = on_paper(&original, 100, Some(4.0));
    let mut seen = Vec::new();
    let result = save_fake(&tools, &info, out.path(), &mut |p| seen.push(p));
    assert!(matches!(result, Err(SaveError::Run(_))), "got {result:?}");
    assert_eq!(seen, [Some(25)]);
}

#[cfg(unix)]
#[test]
fn progress_without_a_duration_is_none() {
    let (_scripts, out, tools) = fake_setup(&format!("{REPORTS_TWO_POINTS} {WRITES_PARTIAL}"));
    let source = TempDir::new().unwrap();
    let original = source.path().join("a.mp4");
    fs::write(&original, vec![0u8; 100]).unwrap();
    let info = on_paper(&original, 100, None);
    let mut seen = Vec::new();
    save_fake(&tools, &info, out.path(), &mut |p| seen.push(p)).unwrap();
    // Two reports and the finish, none of them a number.
    assert_eq!(seen, [None, None, None]);
}
