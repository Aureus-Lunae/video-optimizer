//! Probing a video. The first half reads committed ffprobe output through
//! `parse_probe`; the second half runs `probe` for real, on clips made by the
//! real ffmpeg.

#[path = "common/clips.rs"]
mod clips;

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tempfile::TempDir;
use video_optimizer_core::probe::{
    AudioStream, MAX_INPUT_BYTES, ProbeError, VideoInfo, parse_probe, probe,
};
use video_optimizer_core::tools::{Tools, absolute};

use clips::{Clip, Codec};

// ---- parse_probe, from fixtures ----
//
// The fixtures in tests/fixtures/probe come from ffprobe 8.1 (see the README
// there).

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/probe")
        .join(format!("{name}.json"));
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// `parse_probe` on `value`, for a file `/videos/clip.mp4` of 1234 bytes.
fn parse(value: &Value) -> Result<VideoInfo, ProbeError> {
    parse_probe(&value.to_string(), PathBuf::from("/videos/clip.mp4"), 1234)
}

/// Replace the field at `pointer` (a JSON pointer such as `/streams/0/width`).
fn set(value: &mut Value, pointer: &str, new: Value) {
    *value
        .pointer_mut(pointer)
        .unwrap_or_else(|| panic!("the fixture has no {pointer}")) = new;
}

/// Remove the field `key` of the object at `pointer`.
fn remove(value: &mut Value, pointer: &str, key: &str) {
    let removed = value
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .and_then(|object| object.remove(key));
    assert!(removed.is_some(), "the fixture has no {key} in {pointer}");
}

#[test]
fn reads_size_fps_duration_and_audio() {
    let info = parse(&fixture("full")).unwrap();
    assert_eq!((info.width, info.height), (1328, 747));
    assert_eq!(info.fps, Some(24.0));
    let duration = info.duration.expect("the file has a duration");
    assert!((duration - 36.04).abs() < 0.01, "{duration}");
    assert_eq!(
        info.audio,
        Some(AudioStream {
            codec: "aac".into(),
            bitrate: Some(317_000)
        })
    );
    assert_eq!(info.path, PathBuf::from("/videos/clip.mp4"));
    assert_eq!(info.name, "clip.mp4");
    assert_eq!(info.bytes, 1234);
    assert_eq!(info.stream_index, 0);
    assert_eq!(info.video_codec, "h264");
    assert_eq!(info.pix_fmt, "yuv420p");
}

#[test]
fn reports_no_audio_when_there_is_no_audio_stream() {
    let info = parse(&fixture("no_audio")).unwrap();
    assert_eq!(info.audio, None);
    assert_eq!((info.width, info.height), (1328, 747));
}

#[test]
fn takes_the_first_audio_stream_and_a_missing_bitrate_is_none() {
    let mut value = fixture("full");
    remove(&mut value, "/streams/1", "bit_rate");
    let second_audio = json!({
        "index": 2, "codec_type": "audio", "codec_name": "mp3",
        "bit_rate": "99000", "disposition": {"attached_pic": 0}
    });
    value["streams"].as_array_mut().unwrap().push(second_audio);
    let audio = parse(&value).unwrap().audio.expect("there is audio");
    assert_eq!(audio.codec, "aac");
    assert_eq!(audio.bitrate, None);
    // A bit rate that is not a number is no bit rate either.
    value["streams"][1]["bit_rate"] = json!("N/A");
    assert_eq!(parse(&value).unwrap().audio.unwrap().bitrate, None);
}

#[test]
fn swaps_width_and_height_for_a_90_degree_rotation() {
    let value = fixture("rotated");
    assert_eq!(value["streams"][0]["width"], 1920, "stored size");
    let info = parse(&value).unwrap();
    assert_eq!((info.width, info.height), (1080, 1920));
    for degrees in [json!(90), json!(270), json!(-270), json!(-90.0)] {
        let mut value = fixture("rotated");
        set(
            &mut value,
            "/streams/0/side_data_list/0/rotation",
            degrees.clone(),
        );
        let info = parse(&value).unwrap();
        assert_eq!((info.width, info.height), (1080, 1920), "{degrees}");
    }
}

#[test]
fn keeps_the_size_for_rotation_0_and_180() {
    for degrees in [json!(0), json!(180), json!(-180), json!(360), json!(45)] {
        let mut value = fixture("rotated");
        set(
            &mut value,
            "/streams/0/side_data_list/0/rotation",
            degrees.clone(),
        );
        let info = parse(&value).unwrap();
        assert_eq!((info.width, info.height), (1920, 1080), "{degrees}");
    }
}

#[test]
fn reads_a_rotation_from_the_rotate_tag_when_there_is_no_display_matrix() {
    let mut value = fixture("rotated");
    remove(&mut value, "/streams/0", "side_data_list");
    value["streams"][0]["tags"] = json!({"rotate": "90"});
    let info = parse(&value).unwrap();
    assert_eq!((info.width, info.height), (1080, 1920));

    value["streams"][0]["tags"] = json!({"rotate": "180"});
    let info = parse(&value).unwrap();
    assert_eq!((info.width, info.height), (1920, 1080));

    // No rotation anywhere: the stored size.
    remove(&mut value, "/streams/0", "tags");
    let info = parse(&value).unwrap();
    assert_eq!((info.width, info.height), (1920, 1080));
}

#[test]
fn the_display_matrix_wins_over_the_rotate_tag() {
    let mut value = fixture("rotated");
    set(&mut value, "/streams/0/side_data_list/0/rotation", json!(0));
    value["streams"][0]["tags"] = json!({"rotate": "90"});
    let info = parse(&value).unwrap();
    assert_eq!((info.width, info.height), (1920, 1080));
}

#[test]
fn skips_cover_art_and_picks_the_real_video() {
    let value = fixture("cover_first");
    assert_eq!(value["streams"][0]["codec_name"], "mjpeg");
    assert_eq!(value["streams"][0]["disposition"]["attached_pic"], 1);
    let info = parse(&value).unwrap();
    assert_eq!(info.stream_index, 1);
    assert_eq!(info.video_codec, "h264");
    assert_eq!((info.width, info.height), (320, 180));
}

#[test]
fn takes_the_first_video_when_there_are_two() {
    let mut value = fixture("full");
    let second_video = json!({
        "index": 2, "codec_type": "video", "codec_name": "hevc", "width": 640,
        "height": 360, "pix_fmt": "yuv420p", "disposition": {"attached_pic": 0}
    });
    value["streams"].as_array_mut().unwrap().push(second_video);
    let info = parse(&value).unwrap();
    assert_eq!(info.stream_index, 0);
    assert_eq!(info.video_codec, "h264");
}

#[test]
fn refuses_a_file_whose_only_video_is_cover_art() {
    let value = fixture("cover_only");
    assert_eq!(value["streams"][1]["codec_type"], "video");
    assert_eq!(value["streams"][1]["disposition"]["attached_pic"], 1);
    assert_eq!(parse(&value), Err(ProbeError::NoVideo));
}

#[test]
fn refuses_output_with_no_streams() {
    assert_eq!(parse(&json!({"streams": []})), Err(ProbeError::NoVideo));
    assert_eq!(parse(&json!({})), Err(ProbeError::NoVideo));
}

#[test]
fn refuses_a_video_stream_without_a_size() {
    for pointer in ["width", "height"] {
        let mut value = fixture("full");
        remove(&mut value, "/streams/0", pointer);
        let result = parse(&value);
        assert!(
            matches!(result, Err(ProbeError::Unreadable(_))),
            "{result:?}"
        );
    }
    let mut value = fixture("full");
    set(&mut value, "/streams/0/width", json!(0));
    let result = parse(&value);
    assert!(
        matches!(result, Err(ProbeError::Unreadable(_))),
        "{result:?}"
    );
}

#[test]
fn refuses_output_that_is_not_json() {
    for text in ["", "not json", "{\"streams\": 5}"] {
        let result = parse_probe(text, PathBuf::from("/v/a.mp4"), 1);
        let Err(ProbeError::Unreadable(reason)) = result else {
            panic!("{text:?} gave {result:?}");
        };
        assert!(!reason.is_empty());
    }
}

#[test]
fn missing_or_zero_duration_is_none() {
    let mut value = fixture("full");
    for text in ["N/A", "0.000000", "-1.5", "nan", "inf", "soon"] {
        set(&mut value, "/format/duration", json!(text));
        assert_eq!(parse(&value).unwrap().duration, None, "{text}");
    }
    remove(&mut value, "/format", "duration");
    assert_eq!(parse(&value).unwrap().duration, None, "absent");
    remove(&mut value, "", "format");
    assert_eq!(parse(&value).unwrap().duration, None, "no format at all");
}

#[test]
fn a_real_duration_is_kept() {
    let mut value = fixture("full");
    set(&mut value, "/format/duration", json!("12.500000"));
    assert_eq!(parse(&value).unwrap().duration, Some(12.5));
}

#[test]
fn zero_denominator_frame_rate_is_none() {
    let mut value = fixture("full");
    for text in ["0/0", "24/0", "0/1", "-24/1", "abc", "24", "24/x", ""] {
        set(&mut value, "/streams/0/avg_frame_rate", json!(text));
        assert_eq!(parse(&value).unwrap().fps, None, "{text:?}");
    }
    remove(&mut value, "/streams/0", "avg_frame_rate");
    assert_eq!(parse(&value).unwrap().fps, None, "absent");
}

#[test]
fn ntsc_frame_rate_is_29_97() {
    let mut value = fixture("full");
    set(&mut value, "/streams/0/avg_frame_rate", json!("30000/1001"));
    let fps = parse(&value).unwrap().fps.expect("a frame rate");
    assert!((fps - 29.97).abs() < 0.001, "{fps}");
}

#[test]
fn needs_display_copy_for_prores_hevc_and_10_bit_h264() {
    for (codec, pix_fmt) in [
        ("prores", "yuv422p10le"),
        ("hevc", "yuv420p"),
        ("h264", "yuv420p10le"),
        ("h264", "yuv444p"),
        ("mpeg4", "yuv420p"),
        ("h264", ""),
    ] {
        let mut value = fixture("full");
        set(&mut value, "/streams/0/codec_name", json!(codec));
        set(&mut value, "/streams/0/pix_fmt", json!(pix_fmt));
        let info = parse(&value).unwrap();
        assert!(info.needs_display_copy(), "{codec} {pix_fmt}");
    }
}

#[test]
fn plays_8_bit_h264_as_it_is() {
    for pix_fmt in ["yuv420p", "yuvj420p"] {
        let mut value = fixture("full");
        set(&mut value, "/streams/0/pix_fmt", json!(pix_fmt));
        let info = parse(&value).unwrap();
        assert!(!info.needs_display_copy(), "h264 {pix_fmt}");
    }
}

#[test]
fn a_probe_error_says_what_is_wrong() {
    assert_eq!(ProbeError::NotAFile.to_string(), "this is not a file");
    assert_eq!(
        ProbeError::TooLarge {
            bytes: MAX_INPUT_BYTES + 1
        }
        .to_string(),
        "this file is larger than 10 GiB"
    );
    assert_eq!(ProbeError::NoVideo.to_string(), "this file has no video");
    assert_eq!(
        ProbeError::Unreadable("Invalid data".into()).to_string(),
        "this file could not be read: Invalid data"
    );
}

// ---- probe, for real ----

fn tools() -> Tools {
    Tools::locate().expect("ffmpeg and ffprobe are needed for these tests")
}

fn clip(name: &str) -> Clip {
    Clip {
        name: name.into(),
        ..Clip::default()
    }
}

/// Tools whose programs do not exist: starting one fails, which is a
/// different error from any a real ffprobe gives.
fn missing_tools(dir: &Path) -> Tools {
    Tools {
        ffmpeg: dir.join("no-such-ffmpeg"),
        ffprobe: dir.join("no-such-ffprobe"),
    }
}

#[test]
fn probes_a_generated_clip() {
    let dir = TempDir::new().unwrap();
    let path = clips::make(
        dir.path(),
        Clip {
            audio: true,
            ..clip("a.mp4")
        },
    );
    let info = probe(&tools(), &path).unwrap();
    assert_eq!((info.width, info.height), (640, 360));
    assert_eq!(info.fps, Some(10.0));
    let duration = info.duration.expect("a duration");
    assert!((duration - 2.0).abs() < 0.2, "{duration}");
    assert_eq!(info.video_codec, "h264");
    assert_eq!(info.pix_fmt, "yuv420p");
    assert_eq!(info.audio.as_ref().expect("audio").codec, "aac");
    assert_eq!(info.stream_index, 0);
    assert_eq!(info.name, "a.mp4");
    assert_eq!(info.path, path);
    assert_eq!(info.bytes, fs::metadata(&path).unwrap().len());
    assert!(!info.needs_display_copy());
}

#[test]
fn a_clip_without_audio_has_no_audio() {
    let dir = TempDir::new().unwrap();
    let path = clips::make(dir.path(), clip("silent.mp4"));
    assert_eq!(probe(&tools(), &path).unwrap().audio, None);
}

#[test]
fn probes_the_displayed_size_of_a_rotated_clip() {
    let dir = TempDir::new().unwrap();
    for (rotate, expected) in [
        (Some(-90), (180, 320)),
        (Some(90), (180, 320)),
        (Some(180), (320, 180)),
        (None, (320, 180)),
    ] {
        let name = format!("r{}.mp4", rotate.unwrap_or(0));
        let path = clips::make(
            dir.path(),
            Clip {
                width: 320,
                height: 180,
                rotate,
                ..clip(&name)
            },
        );
        let info = probe(&tools(), &path).unwrap();
        assert_eq!((info.width, info.height), expected, "rotation {rotate:?}");
    }
}

#[test]
fn flags_prores_and_mpeg4_clips_as_needing_a_display_copy() {
    let dir = TempDir::new().unwrap();
    let prores = clips::make(
        dir.path(),
        Clip {
            codec: Codec::ProRes,
            ..clip("p.mov")
        },
    );
    let info = probe(&tools(), &prores).unwrap();
    assert_eq!(info.video_codec, "prores");
    assert!(info.needs_display_copy());

    let mpeg4 = clips::make(
        dir.path(),
        Clip {
            codec: Codec::Mpeg4,
            ..clip("m.mp4")
        },
    );
    let info = probe(&tools(), &mpeg4).unwrap();
    assert_eq!(info.video_codec, "mpeg4");
    assert!(info.needs_display_copy());
}

#[test]
fn probes_past_a_cover_picture_to_the_video() {
    let dir = TempDir::new().unwrap();
    let path = clips::make(
        dir.path(),
        Clip {
            cover_art: true,
            audio: true,
            ..clip("covered.mp4")
        },
    );
    // The cover is really in the file ("covr" is the mp4 atom that holds it).
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.windows(4).any(|window| window == b"covr"));
    let info = probe(&tools(), &path).unwrap();
    assert_eq!(info.stream_index, 0);
    assert_eq!((info.width, info.height), (640, 360));
    assert_eq!(info.video_codec, "h264");
}

#[test]
fn refuses_a_text_file_renamed_mp4() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("notes.mp4");
    fs::write(&path, "this is not a video\n").unwrap();
    let result = probe(&tools(), &path);
    let Err(ProbeError::Unreadable(reason)) = result else {
        panic!("{result:?}");
    };
    assert!(!reason.is_empty());
    assert!(!reason.contains('\n'));
    assert!(reason.chars().count() <= 300);
}

#[test]
fn refuses_a_folder() {
    let dir = TempDir::new().unwrap();
    assert_eq!(probe(&tools(), dir.path()), Err(ProbeError::NotAFile));
}

#[test]
fn refuses_a_path_that_is_not_there() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("gone.mp4");
    assert_eq!(probe(&tools(), &missing), Err(ProbeError::NotAFile));
    assert_eq!(probe(&tools(), Path::new("")), Err(ProbeError::NotAFile));
}

#[test]
fn refuses_an_audio_only_file() {
    let dir = TempDir::new().unwrap();
    let path = clips::make_audio(dir.path(), "tone.m4a", 1.0, false);
    assert_eq!(probe(&tools(), &path), Err(ProbeError::NoVideo));
}

#[test]
fn refuses_an_audio_file_whose_only_picture_is_its_cover() {
    let dir = TempDir::new().unwrap();
    let path = clips::make_audio(dir.path(), "album.m4a", 1.0, true);
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.windows(4).any(|window| window == b"covr"));
    assert_eq!(probe(&tools(), &path), Err(ProbeError::NoVideo));
}

#[test]
fn probes_a_name_with_spaces_quotes_and_non_ascii() {
    // Windows does not allow a double quote in a file name.
    let name = if cfg!(windows) {
        "it's a clip é.mp4"
    } else {
        "it's a \"clip\" é.mp4"
    };
    let dir = TempDir::new().unwrap();
    let path = clips::make(dir.path(), clip(name));
    let info = probe(&tools(), &path).unwrap();
    assert_eq!(info.name, name);
    assert_eq!((info.width, info.height), (640, 360));
}

#[test]
fn probes_a_name_starting_with_a_dash() {
    let dir = TempDir::new().unwrap();
    let path = clips::make(dir.path(), clip("-y.mp4"));
    let info = probe(&tools(), &path).unwrap();
    assert_eq!(info.name, "-y.mp4");
    assert_eq!((info.width, info.height), (640, 360));
}

#[test]
fn probes_a_relative_path_and_reports_it_absolute() {
    // `cargo test` runs in the crate's folder, so a folder made here can be
    // named relatively.
    let dir = TempDir::new_in(".").unwrap();
    clips::make(dir.path(), clip("rel.mp4"));
    let relative = Path::new(dir.path().file_name().unwrap()).join("rel.mp4");
    assert!(relative.is_relative());
    let info = probe(&tools(), &relative).unwrap();
    assert!(info.path.is_absolute(), "{:?}", info.path);
    assert_eq!(info.path, absolute(&relative).unwrap());
    assert_eq!(info.name, "rel.mp4");
}

/// A sparse file of `bytes` bytes: it takes no room on disk.
fn sparse_file(dir: &Path, bytes: u64) -> PathBuf {
    let path = dir.join("big.mp4");
    File::create(&path).unwrap().set_len(bytes).unwrap();
    path
}

#[test]
fn refuses_a_file_over_10_gib_without_starting_ffprobe() {
    let dir = TempDir::new().unwrap();
    let path = sparse_file(dir.path(), MAX_INPUT_BYTES + 1);
    // Starting the program would fail with `Unreadable`, not `TooLarge`.
    let result = probe(&missing_tools(dir.path()), &path);
    assert_eq!(
        result,
        Err(ProbeError::TooLarge {
            bytes: MAX_INPUT_BYTES + 1
        })
    );
}

#[test]
fn accepts_exactly_10_gib_as_far_as_the_size_check() {
    let dir = TempDir::new().unwrap();
    let path = sparse_file(dir.path(), MAX_INPUT_BYTES);
    // Past the size check, the program is started. It is missing, so the
    // answer is `Unreadable`, and the reason says why.
    let result = probe(&missing_tools(dir.path()), &path);
    let Err(ProbeError::Unreadable(reason)) = result else {
        panic!("{result:?}");
    };
    assert!(reason.contains("no-such-ffprobe"), "{reason}");
}

#[test]
fn a_real_ffprobe_reads_a_sparse_file_of_exactly_10_gib_past_the_size_check() {
    let dir = TempDir::new().unwrap();
    let path = sparse_file(dir.path(), MAX_INPUT_BYTES);
    // Zeros are no video: ffprobe answers, and the size check let it start.
    let result = probe(&tools(), &path);
    assert!(
        matches!(result, Err(ProbeError::Unreadable(_))),
        "{result:?}"
    );
}

/// Tools whose "ffprobe" is this program, which ignores ffprobe's arguments
/// and exits with a status, so the exit paths can be reached without a real
/// ffprobe that fails on cue.
#[cfg(unix)]
fn tools_that_run(program: &str) -> Tools {
    Tools {
        ffmpeg: PathBuf::from(program),
        ffprobe: PathBuf::from(program),
    }
}

#[cfg(unix)]
#[test]
fn a_failing_ffprobe_with_no_message_is_still_a_reason() {
    let dir = TempDir::new().unwrap();
    let path = clips::make(dir.path(), clip("a.mp4"));
    let result = probe(&tools_that_run("/usr/bin/false"), &path);
    let Err(ProbeError::Unreadable(reason)) = result else {
        panic!("{result:?}");
    };
    assert!(reason.contains("exit"), "{reason}");
}

#[cfg(unix)]
#[test]
fn an_ffprobe_that_prints_nothing_is_unreadable() {
    let dir = TempDir::new().unwrap();
    let path = clips::make(dir.path(), clip("a.mp4"));
    // Exit status 0 and no JSON: the answer is not taken for a video.
    let result = probe(&tools_that_run("/usr/bin/true"), &path);
    assert!(
        matches!(result, Err(ProbeError::Unreadable(_))),
        "{result:?}"
    );
}
