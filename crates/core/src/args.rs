//! Every argument list ffmpeg is started with, as plain functions: the full
//! encode, the "before" clip of the comparison, and the PSNR measurement.
//!
//! Nothing here starts a process. A file reaches ffmpeg only through
//! [`file_arg`], as one argument that starts with `file:`, never as a part of
//! a longer string and never as something that could be read as an option.
//! The output keeps the input's size: there is no scale filter, ever. The
//! only filter is `pad`, which adds one pixel to an odd side.

use std::ffi::OsString;
use std::path::Path;

use crate::probe::VideoInfo;
use crate::settings::{Audio, Settings};
use crate::tools::{PathError, file_arg};

/// The length of the sample and of the "before" clip, in seconds.
pub const SAMPLE_SECONDS: f64 = 10.0;

/// The CRF of the converted copy of an original the window cannot play: close
/// to lossless, so the comparison shows only the sample's own loss.
const DISPLAY_COPY_CRF: &str = "10";

/// How much of the input an encode covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    /// The whole input: the Save.
    Whole,
    /// The first [`SAMPLE_SECONDS`] seconds (the whole clip when shorter): the
    /// sample.
    Sample,
}

/// `width` and `height`, each raised by one when odd. H.264 in `yuv420p`
/// needs even sides, and a one pixel border is added, never a scale.
pub fn even_size(width: u32, height: u32) -> (u32, u32) {
    (
        width.saturating_add(width % 2),
        height.saturating_add(height % 2),
    )
}

/// Appends fixed text arguments.
fn push(args: &mut Vec<OsString>, items: &[&str]) {
    args.extend(items.iter().map(OsString::from));
}

/// `-vf pad=<W>:<H>` when the displayed size has an odd side. ffmpeg turns a
/// rotated picture itself, so the padding is on the displayed size, which is
/// what [`VideoInfo`] holds.
fn push_pad(args: &mut Vec<OsString>, info: &VideoInfo) {
    let (width, height) = even_size(info.width, info.height);
    if (width, height) != (info.width, info.height) {
        push(args, &["-vf", &format!("pad={width}:{height}")]);
    }
}

/// The start every ffmpeg list that writes a file shares: no banner, no
/// stdin, overwrite. Every output is a temporary file this app just made, so
/// `-y` replaces nothing of the user's.
fn start(args: &mut Vec<OsString>, input: &OsString) {
    push(args, &["-hide_banner", "-nostdin", "-y", "-i"]);
    args.push(input.clone());
}

/// The arguments for encoding `info` into `output`: H.264 at the CRF of
/// `settings`, over the whole input or only the sample. Progress is written to
/// standard output (`-progress pipe:1`).
///
/// Refuses a relative input or output path.
pub fn encode_args(
    info: &VideoInfo,
    settings: Settings,
    output: &Path,
    span: Span,
) -> Result<Vec<OsString>, PathError> {
    let input = file_arg(&info.path)?;
    let output = file_arg(output)?;
    let mut args = Vec::new();
    start(&mut args, &input);
    if span == Span::Sample {
        push(&mut args, &["-t", &SAMPLE_SECONDS.to_string()]);
    }
    push(&mut args, &["-map", &format!("0:{}", info.stream_index)]);
    if settings.audio == Audio::Keep {
        // The `?` lets a file with no audio through.
        push(&mut args, &["-map", "0:a:0?"]);
    }
    push(
        &mut args,
        &[
            "-c:v",
            "libx264",
            "-preset",
            "slow",
            "-crf",
            &settings.crf.get().to_string(),
            "-pix_fmt",
            "yuv420p",
        ],
    );
    push_pad(&mut args, info);
    match settings.audio {
        Audio::Keep => push(&mut args, &["-c:a", "aac", "-b:a", "128k"]),
        Audio::Remove => push(&mut args, &["-an"]),
    }
    push(
        &mut args,
        &[
            "-movflags",
            "+faststart",
            "-progress",
            "pipe:1",
            "-nostats",
            "-f",
            "mp4",
        ],
    );
    args.push(output);
    Ok(args)
}

/// The arguments for the "before" clip: the first [`SAMPLE_SECONDS`] seconds of
/// the original, with no audio. A source the window plays is copied as it is;
/// anything else becomes a near lossless H.264 copy for display.
///
/// Refuses a relative input or output path.
pub fn before_args(info: &VideoInfo, output: &Path) -> Result<Vec<OsString>, PathError> {
    let input = file_arg(&info.path)?;
    let output = file_arg(output)?;
    let mut args = Vec::new();
    start(&mut args, &input);
    push(
        &mut args,
        &[
            "-t",
            &SAMPLE_SECONDS.to_string(),
            "-map",
            &format!("0:{}", info.stream_index),
            "-an",
        ],
    );
    if info.needs_display_copy() {
        push(
            &mut args,
            &[
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                DISPLAY_COPY_CRF,
                "-pix_fmt",
                "yuv420p",
            ],
        );
        push_pad(&mut args, info);
    } else {
        // A stream copy cannot be filtered, so there is no padding here.
        push(&mut args, &["-c:v", "copy"]);
    }
    push(&mut args, &["-movflags", "+faststart", "-f", "mp4"]);
    args.push(output);
    Ok(args)
}

/// The arguments that measure how much `sample` differs from `before_ref`
/// (PSNR), writing nothing: ffmpeg prints the figures on its standard error.
/// Both inputs must have the same size.
///
/// Refuses a relative path on either side.
pub fn psnr_args(sample: &Path, before_ref: &Path) -> Result<Vec<OsString>, PathError> {
    let sample = file_arg(sample)?;
    let before_ref = file_arg(before_ref)?;
    let mut args = Vec::new();
    push(&mut args, &["-hide_banner", "-nostdin", "-i"]);
    args.push(sample);
    push(&mut args, &["-i"]);
    args.push(before_ref);
    // The final `-` is ffmpeg's null output, not a file.
    push(&mut args, &["-lavfi", "[0:v][1:v]psnr", "-f", "null", "-"]);
    Ok(args)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::probe::VideoInfo;
    use crate::settings::{Audio, Crf, Settings};
    use crate::tools::{PathError, file_arg};

    /// Every option that appears in the argument lists of this module. Only
    /// these may start with a dash.
    const KNOWN_OPTIONS: &[&str] = &[
        "-hide_banner",
        "-nostdin",
        "-y",
        "-i",
        "-t",
        "-map",
        "-c:v",
        "-preset",
        "-crf",
        "-pix_fmt",
        "-vf",
        "-c:a",
        "-b:a",
        "-an",
        "-movflags",
        "-progress",
        "-nostats",
        "-f",
        "-lavfi",
        // ffmpeg's name for the null output, last in the psnr list.
        "-",
    ];

    /// An absolute path on every platform, in a folder with spaces.
    fn path(name: &str) -> PathBuf {
        std::env::temp_dir().join("a folder with spaces").join(name)
    }

    /// A video that the window plays: H.264, 8 bit, `width` by `height` as
    /// displayed, video stream 0.
    fn info(width: u32, height: u32) -> VideoInfo {
        VideoInfo {
            path: path("in.mp4"),
            name: "in.mp4".into(),
            bytes: 1_000_000,
            stream_index: 0,
            width,
            height,
            fps: Some(25.0),
            duration: Some(60.0),
            video_codec: "h264".into(),
            pix_fmt: "yuv420p".into(),
            audio: None,
        }
    }

    /// A video the window cannot play.
    fn prores(width: u32, height: u32) -> VideoInfo {
        VideoInfo {
            video_codec: "prores".into(),
            pix_fmt: "yuv422p10le".into(),
            ..info(width, height)
        }
    }

    fn settings(crf: i64, audio: Audio) -> Settings {
        Settings {
            crf: Crf::new(crf).unwrap(),
            audio,
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn encode(info: &VideoInfo, crf: i64, audio: Audio, span: Span) -> Vec<String> {
        strings(&encode_args(info, settings(crf, audio), &path("out.mp4"), span).unwrap())
    }

    fn before(info: &VideoInfo) -> Vec<String> {
        strings(&before_args(info, &path("before.mp4")).unwrap())
    }

    fn owned(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    /// `file_arg` of a path that is known to be absolute.
    fn file(path: &Path) -> String {
        file_arg(path).unwrap().to_string_lossy().into_owned()
    }

    /// Whether `run` appears in `args` as consecutive arguments, in order.
    fn has_run(args: &[String], run: &[&str]) -> bool {
        args.windows(run.len())
            .any(|window| window.iter().zip(run).all(|(a, b)| a == b))
    }

    fn has(args: &[String], wanted: &str) -> bool {
        args.iter().any(|arg| arg == wanted)
    }

    fn position(args: &[String], wanted: &str) -> usize {
        args.iter()
            .position(|arg| arg == wanted)
            .unwrap_or_else(|| panic!("{wanted} is missing from {args:?}"))
    }

    /// The arguments that would scale the picture: any containing `scale`
    /// (`scale`, `zscale`, `scale_cuda`...), or the `-s` size option.
    fn scaling(args: &[String]) -> Vec<&String> {
        args.iter()
            .filter(|arg| arg.contains("scale") || *arg == "-s")
            .collect()
    }

    /// The arguments that start with a dash and are not a known option.
    fn unknown_dash_args(args: &[String]) -> Vec<&String> {
        args.iter()
            .filter(|arg| arg.starts_with('-') && !KNOWN_OPTIONS.contains(&arg.as_str()))
            .collect()
    }

    // ---- even_size ----

    #[test]
    fn even_size_rounds_odd_sides_up_by_one() {
        assert_eq!(even_size(1281, 721), (1282, 722));
        assert_eq!(even_size(1280, 721), (1280, 722));
        assert_eq!(even_size(1281, 720), (1282, 720));
        assert_eq!(even_size(1, 1), (2, 2));
    }

    #[test]
    fn even_size_leaves_even_sides_alone() {
        assert_eq!(even_size(1920, 1080), (1920, 1080));
        assert_eq!(even_size(2, 2), (2, 2));
    }

    #[test]
    fn even_size_does_not_overflow_at_the_largest_size() {
        // No panic. A size no video has, so the value itself does not matter.
        assert_eq!(even_size(u32::MAX, u32::MAX), (u32::MAX, u32::MAX));
    }

    // ---- encode: fixed settings and CRF ----

    #[test]
    fn whole_encode_has_the_fixed_settings() {
        let args = encode(&info(1920, 1080), 28, Audio::Keep, Span::Whole);
        assert!(has_run(
            &args,
            &[
                "-c:v", "libx264", "-preset", "slow", "-crf", "28", "-pix_fmt", "yuv420p"
            ]
        ));
        assert!(has_run(&args, &["-movflags", "+faststart"]));
        assert!(has_run(&args, &["-f", "mp4"]));
        assert!(has_run(&args, &["-progress", "pipe:1", "-nostats"]));
        assert_eq!(&args[..4], ["-hide_banner", "-nostdin", "-y", "-i"]);
        assert!(position(&args, "-c:v") < position(&args, "-movflags"));
        assert!(position(&args, "-movflags") < position(&args, "-f"));
    }

    #[test]
    fn encode_uses_the_crf_of_the_settings() {
        for crf in [20, 28, 36] {
            let args = encode(&info(1920, 1080), crf, Audio::Keep, Span::Whole);
            assert!(has_run(&args, &["-crf", &crf.to_string()]), "{crf}");
            assert_eq!(args.iter().filter(|a| *a == "-crf").count(), 1);
        }
        // The negative: CRF 20 does not leave the default 28 in the list.
        let args = encode(&info(1920, 1080), 20, Audio::Keep, Span::Whole);
        assert!(!has_run(&args, &["-crf", "28"]));
    }

    #[test]
    fn the_output_is_the_last_argument_and_appears_once() {
        let args = encode(&info(1920, 1080), 28, Audio::Keep, Span::Whole);
        let out = file(&path("out.mp4"));
        assert_eq!(args.last().unwrap(), &out);
        assert_eq!(args.iter().filter(|a| **a == out).count(), 1);
    }

    // ---- encode: span ----

    #[test]
    fn whole_encode_has_no_time_limit() {
        let args = encode(&info(1920, 1080), 28, Audio::Keep, Span::Whole);
        assert!(!has(&args, "-t"), "{args:?}");
    }

    #[test]
    fn sample_encode_stops_at_10_seconds() {
        let args = encode(&info(1920, 1080), 28, Audio::Keep, Span::Sample);
        assert!(has_run(&args, &["-t", "10"]), "{args:?}");
        assert!(position(&args, "-i") < position(&args, "-t"));
        assert!(position(&args, "-t") < position(&args, "-map"));
        assert_eq!(args.iter().filter(|a| *a == "-t").count(), 1);
    }

    #[test]
    fn the_sample_is_ten_seconds_long() {
        assert_eq!(SAMPLE_SECONDS, 10.0);
    }

    // ---- never scales ----

    #[test]
    fn never_scales() {
        let mut checked = 0;
        for span in [Span::Whole, Span::Sample] {
            for audio in [Audio::Keep, Audio::Remove] {
                for (w, h) in [(1920, 1080), (1281, 721), (721, 1281), (1280, 721), (3, 3)] {
                    let args = encode(&info(w, h), 28, audio, span);
                    assert!(!args.is_empty());
                    assert_eq!(scaling(&args), Vec::<&String>::new(), "{args:?}");
                    checked += 1;
                }
            }
        }
        for video in [
            info(1920, 1080),
            info(1281, 721),
            prores(1920, 1080),
            prores(1281, 721),
            prores(721, 1281),
        ] {
            let args = before(&video);
            assert!(!args.is_empty());
            assert_eq!(scaling(&args), Vec::<&String>::new(), "{args:?}");
            checked += 1;
        }
        let psnr = strings(&psnr_args(&path("s.mp4"), &path("b.mp4")).unwrap());
        assert!(!psnr.is_empty());
        assert_eq!(scaling(&psnr), Vec::<&String>::new(), "{psnr:?}");
        // 20 encode lists and 5 before lists, so no combination was skipped.
        assert_eq!(checked, 25);
    }

    #[test]
    fn the_scaling_check_finds_every_way_to_scale() {
        for fixture in [
            owned(&["-vf", "scale=1280:-2"]),
            owned(&["-vf", "zscale=w=1280"]),
            owned(&["-vf", "pad=1282:722,scale_cuda=1280:720"]),
            owned(&["-s", "1280x720"]),
        ] {
            assert!(!scaling(&fixture).is_empty(), "{fixture:?}");
        }
        assert!(scaling(&owned(&["-vf", "pad=1282:722", "-preset", "slow"])).is_empty());
    }

    // ---- padding ----

    #[test]
    fn pads_1281x721_to_1282x722() {
        let args = encode(&info(1281, 721), 28, Audio::Keep, Span::Whole);
        assert!(has_run(&args, &["-vf", "pad=1282:722"]), "{args:?}");
        assert_eq!(args.iter().filter(|a| *a == "-vf").count(), 1);
    }

    #[test]
    fn pads_only_the_odd_side() {
        let wide = encode(&info(1281, 720), 28, Audio::Keep, Span::Whole);
        assert!(has_run(&wide, &["-vf", "pad=1282:720"]), "{wide:?}");
        let tall = encode(&info(1280, 721), 28, Audio::Keep, Span::Whole);
        assert!(has_run(&tall, &["-vf", "pad=1280:722"]), "{tall:?}");
    }

    #[test]
    fn an_even_source_gets_no_filter() {
        for span in [Span::Whole, Span::Sample] {
            let args = encode(&info(1920, 1080), 28, Audio::Keep, span);
            assert!(!has(&args, "-vf"), "{args:?}");
            assert!(!args.iter().any(|a| a.contains("pad")), "{args:?}");
        }
    }

    #[test]
    fn pads_a_rotated_odd_source_on_its_displayed_size() {
        // Displayed 721 wide and 1281 tall: ffmpeg turns the picture itself,
        // so the padding is on the displayed size, not the stored one.
        let args = encode(&info(721, 1281), 28, Audio::Keep, Span::Whole);
        assert!(has_run(&args, &["-vf", "pad=722:1282"]), "{args:?}");
        assert!(!has(&args, "pad=1282:722"));
    }

    #[test]
    fn the_filter_sits_between_the_video_and_the_audio_options() {
        let args = encode(&info(1281, 721), 28, Audio::Keep, Span::Whole);
        assert!(position(&args, "-pix_fmt") < position(&args, "-vf"));
        assert!(position(&args, "-vf") < position(&args, "-c:a"));
    }

    // ---- audio ----

    #[test]
    fn keep_maps_audio_as_aac_128k() {
        let args = encode(&info(1920, 1080), 28, Audio::Keep, Span::Whole);
        assert!(has_run(&args, &["-map", "0:a:0?"]), "{args:?}");
        assert!(has_run(&args, &["-c:a", "aac", "-b:a", "128k"]), "{args:?}");
        assert!(!has(&args, "-an"));
    }

    #[test]
    fn remove_maps_no_audio() {
        let args = encode(&info(1920, 1080), 28, Audio::Remove, Span::Whole);
        assert!(has(&args, "-an"), "{args:?}");
        assert!(!args.iter().any(|a| a.contains("0:a")), "{args:?}");
        assert!(!has(&args, "-c:a") && !has(&args, "-b:a"), "{args:?}");
    }

    // ---- stream choice ----

    #[test]
    fn maps_the_chosen_video_stream() {
        let mut video = info(1920, 1080);
        video.stream_index = 1;
        let args = encode(&video, 28, Audio::Keep, Span::Whole);
        assert!(has_run(&args, &["-map", "0:1"]), "{args:?}");
        assert!(!has(&args, "0:v:0") && !has(&args, "0:v"), "{args:?}");
        // The same stream is chosen for the before clip, copied or re-encoded.
        assert!(has_run(&before(&video), &["-map", "0:1"]));
        let mut converted = prores(1920, 1080);
        converted.stream_index = 2;
        assert!(has_run(&before(&converted), &["-map", "0:2"]));
    }

    #[test]
    fn never_maps_the_first_video_stream_by_position() {
        for video in [info(1920, 1080), prores(1281, 721)] {
            let mut all = encode(&video, 28, Audio::Keep, Span::Sample);
            all.extend(before(&video));
            assert!(!all.iter().any(|a| a.contains("0:v")), "{all:?}");
        }
    }

    // ---- paths ----

    #[cfg(unix)]
    #[test]
    fn paths_are_one_argument_each_and_never_start_with_a_dash() {
        let dashed = PathBuf::from("/v/-x \"y\".mp4");
        let video = VideoInfo {
            path: dashed.clone(),
            ..info(1920, 1080)
        };
        let converted = VideoInfo {
            path: dashed.clone(),
            ..prores(1281, 721)
        };
        let out = PathBuf::from("/out folder/with spaces/optimized-x.mp4");
        let input_arg = file(&dashed);
        assert_eq!(input_arg, "file:/v/-x \"y\".mp4");
        let output_arg = file(&out);

        let lists = [
            strings(&encode_args(&video, Settings::default(), &out, Span::Whole).unwrap()),
            strings(&encode_args(&video, Settings::default(), &out, Span::Sample).unwrap()),
            strings(&before_args(&video, &out).unwrap()),
            strings(&before_args(&converted, &out).unwrap()),
        ];
        for args in lists {
            assert!(!args.is_empty());
            assert_eq!(
                args.iter().filter(|a| **a == input_arg).count(),
                1,
                "{args:?}"
            );
            assert_eq!(args[position(&args, "-i") + 1], input_arg);
            assert_eq!(unknown_dash_args(&args), Vec::<&String>::new(), "{args:?}");
            // The output is whole, spaces and all, in one argument.
            assert_eq!(args.last().unwrap(), &output_arg);
            // Nothing carries a piece of the input's name apart from `file:`.
            assert!(!has(&args, "-x") && !has(&args, "y\".mp4"), "{args:?}");
        }

        let psnr = strings(&psnr_args(&dashed, &out).unwrap());
        assert_eq!(psnr.iter().filter(|a| **a == input_arg).count(), 1);
        assert_eq!(psnr.iter().filter(|a| **a == output_arg).count(), 1);
        assert_eq!(unknown_dash_args(&psnr), Vec::<&String>::new(), "{psnr:?}");
        assert!(has_run(&psnr, &["-f", "null", "-"]));
    }

    #[test]
    fn the_dash_check_finds_an_option_that_is_not_in_the_lists() {
        let fixture = owned(&["-i", "file:/a.mp4", "-evil", "-y.mp4", "-"]);
        assert_eq!(unknown_dash_args(&fixture), ["-evil", "-y.mp4"]);
    }

    #[test]
    fn every_option_in_every_list_is_a_known_option() {
        let mut lists = Vec::new();
        for span in [Span::Whole, Span::Sample] {
            for audio in [Audio::Keep, Audio::Remove] {
                lists.push(encode(&info(1281, 721), 28, audio, span));
            }
        }
        lists.push(before(&info(1920, 1080)));
        lists.push(before(&prores(1281, 721)));
        lists.push(strings(&psnr_args(&path("s.mp4"), &path("b.mp4")).unwrap()));
        for args in lists {
            assert!(!args.is_empty());
            assert_eq!(unknown_dash_args(&args), Vec::<&String>::new(), "{args:?}");
        }
    }

    // ---- the refused paths ----

    #[test]
    fn a_relative_input_is_refused() {
        let video = VideoInfo {
            path: PathBuf::from("in.mp4"),
            ..info(1920, 1080)
        };
        let settings = Settings::default();
        let out = path("o.mp4");
        assert_eq!(
            encode_args(&video, settings, &out, Span::Whole),
            Err(PathError::NotAbsolute)
        );
        assert_eq!(
            encode_args(&video, settings, &out, Span::Sample),
            Err(PathError::NotAbsolute)
        );
        assert_eq!(before_args(&video, &out), Err(PathError::NotAbsolute));
    }

    #[test]
    fn a_relative_output_is_refused() {
        let video = info(1920, 1080);
        let out = Path::new("-out.mp4");
        assert_eq!(
            encode_args(&video, Settings::default(), out, Span::Whole),
            Err(PathError::NotAbsolute)
        );
        assert_eq!(before_args(&video, out), Err(PathError::NotAbsolute));
        assert_eq!(
            before_args(&prores(1280, 720), out),
            Err(PathError::NotAbsolute)
        );
    }

    #[test]
    fn a_relative_psnr_input_is_refused_on_either_side() {
        assert_eq!(
            psnr_args(Path::new("s.mp4"), &path("b.mp4")),
            Err(PathError::NotAbsolute)
        );
        assert_eq!(
            psnr_args(&path("s.mp4"), Path::new("-b.mp4")),
            Err(PathError::NotAbsolute)
        );
    }

    // ---- before ----

    #[test]
    fn before_copies_a_playable_source() {
        let args = before(&info(1920, 1080));
        assert!(has_run(&args, &["-c:v", "copy"]), "{args:?}");
        assert!(has(&args, "-an"));
        assert!(has_run(&args, &["-t", "10"]));
        assert!(!has(&args, "libx264") && !has(&args, "-crf"));
        assert!(has_run(&args, &["-movflags", "+faststart", "-f", "mp4"]));
        assert_eq!(&args[..4], ["-hide_banner", "-nostdin", "-y", "-i"]);
    }

    #[test]
    fn before_does_not_filter_a_stream_copy() {
        // A stream copy cannot be filtered, so an odd playable source is
        // copied as it is.
        let args = before(&info(1281, 721));
        assert!(has_run(&args, &["-c:v", "copy"]));
        assert!(!has(&args, "-vf"), "{args:?}");
    }

    #[test]
    fn before_reencodes_prores_at_crf_10() {
        let args = before(&prores(1920, 1080));
        assert!(
            has_run(
                &args,
                &[
                    "-c:v", "libx264", "-preset", "veryfast", "-crf", "10", "-pix_fmt", "yuv420p"
                ]
            ),
            "{args:?}"
        );
        assert!(!has(&args, "copy"), "{args:?}");
        assert!(has(&args, "-an"));
        assert!(has_run(&args, &["-t", "10"]));
        assert!(has_run(&args, &["-movflags", "+faststart", "-f", "mp4"]));
        assert!(!has(&args, "-vf"));
    }

    #[test]
    fn the_display_copy_pads_an_odd_source_on_its_displayed_size() {
        let args = before(&prores(721, 1281));
        assert!(has_run(&args, &["-vf", "pad=722:1282"]), "{args:?}");
        assert!(position(&args, "-pix_fmt") < position(&args, "-vf"));
        assert!(position(&args, "-vf") < position(&args, "-movflags"));
    }

    #[test]
    fn a_ten_bit_h264_source_gets_the_display_copy() {
        let ten_bit = VideoInfo {
            pix_fmt: "yuv420p10le".into(),
            ..info(1920, 1080)
        };
        let args = before(&ten_bit);
        assert!(has_run(&args, &["-crf", "10"]), "{args:?}");
        assert!(!has(&args, "copy"));
    }

    #[test]
    fn before_has_no_audio_and_no_progress_report() {
        for video in [info(1920, 1080), prores(1920, 1080)] {
            let args = before(&video);
            assert!(!args.iter().any(|a| a.contains("0:a")), "{args:?}");
            assert!(!has(&args, "-c:a") && !has(&args, "-progress"));
        }
    }

    // ---- psnr ----

    #[test]
    fn psnr_compares_the_sample_with_the_reference_and_writes_nothing() {
        let args = strings(&psnr_args(&path("sample.mp4"), &path("before.mp4")).unwrap());
        let sample = file(&path("sample.mp4"));
        let reference = file(&path("before.mp4"));
        assert_eq!(
            args,
            [
                "-hide_banner",
                "-nostdin",
                "-i",
                sample.as_str(),
                "-i",
                reference.as_str(),
                "-lavfi",
                "[0:v][1:v]psnr",
                "-f",
                "null",
                "-",
            ]
        );
        // Nothing is overwritten: no `-y`, no file output.
        assert!(!has(&args, "-y"));
    }
}
