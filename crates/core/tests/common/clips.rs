//! Test clips made with the real ffmpeg: a moving test pattern (`testsrc2`)
//! and a sine tone (`sine`), encoded with the fastest preset.
//!
//! Shared by every test crate that needs a video: each one includes this one
//! file with `#[path = "…/crates/core/tests/common/clips.rs"] mod clips;`.
//! There are no copies.

// Each test crate that includes this file uses only part of it.
#![allow(dead_code)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use video_optimizer_core::tools::{Tools, command, file_arg};

/// The video codec of a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// 8-bit `yuv420p`: plays in the window as it is.
    H264,
    /// 10-bit `yuv422p10le`: needs a display copy. Use a `.mov` name.
    ProRes,
    Mpeg4,
}

/// What to put in a clip. `Clip::default()` is a 2 second, 640x360, 10 frames
/// per second H.264 clip with no audio, no rotation, no cover art and x264's
/// own CRF; give it a `name`.
///
/// An odd `width` or `height` is made as a crop of the next even size, in
/// `yuv444p` (the test pattern and the 4:2:0 formats only have even sizes).
/// Only [`Codec::Mpeg4`] and [`Codec::ProRes`] hold such a size: 8-bit 4:2:0
/// H.264 cannot have one (its crop unit is 2 pixels), so [`make`] panics for
/// an odd [`Codec::H264`] clip.
#[derive(Debug, Clone)]
pub struct Clip {
    /// The file name, made inside the folder given to [`make`].
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub seconds: f64,
    pub fps: u32,
    /// An AAC sine tone.
    pub audio: bool,
    pub codec: Codec,
    /// A display rotation in degrees (`-90`), written as the file's display
    /// matrix: the stored size stays `width` x `height`.
    pub rotate: Option<i32>,
    /// A JPEG picture attached as a second video stream, the way a music file
    /// carries its cover. Only mp4 and mov hold it.
    pub cover_art: bool,
    /// The x264 CRF of an H.264 clip (`0` is lossless, `40` is coarse); `None`
    /// leaves it at x264's own 23. Ignored for the other codecs.
    pub crf: Option<u8>,
}

impl Default for Clip {
    fn default() -> Clip {
        Clip {
            name: String::new(),
            width: 640,
            height: 360,
            seconds: 2.0,
            fps: 10,
            audio: false,
            codec: Codec::H264,
            rotate: None,
            cover_art: false,
            crf: None,
        }
    }
}

/// ffmpeg, found the way the app finds it. Panics when there is none: a test
/// that needs a video never skips silently.
fn tools() -> Tools {
    Tools::locate().expect(
        "ffmpeg and ffprobe are needed for these tests: put them on the PATH or set \
         VIDEO_OPTIMIZER_TOOLS_DIR",
    )
}

/// Run ffmpeg with `args`. Panics with ffmpeg's own message when it fails.
fn run_ffmpeg(args: &[OsString]) {
    let output = command(&tools().ffmpeg)
        .args(["-v", "error", "-y"])
        .args(args)
        .output()
        .expect("ffmpeg could not be started");
    assert!(
        output.status.success(),
        "ffmpeg failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn args<const N: usize>(parts: [&str; N]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

/// `file:` plus the path, as ffmpeg takes it.
fn file(path: &Path) -> OsString {
    file_arg(path).expect("the test folder is absolute")
}

/// Make the clip `spec` in `dir`, and return its path.
pub fn make(dir: &Path, spec: Clip) -> PathBuf {
    let out = dir.join(&spec.name);
    let seconds = spec.seconds;
    let odd = spec.width % 2 == 1 || spec.height % 2 == 1;
    assert!(
        !(odd && spec.codec == Codec::H264),
        "8-bit 4:2:0 H.264 cannot have an odd size"
    );
    let mut a = args(["-f", "lavfi", "-i"]);
    a.push(
        format!(
            "testsrc2=size={}x{}:rate={}:duration={seconds}",
            spec.width + spec.width % 2,
            spec.height + spec.height % 2,
            spec.fps
        )
        .into(),
    );
    // The streams of the output, in the order of its inputs: video, then audio,
    // then the cover.
    let mut maps = vec![args(["-map", "0:v"])];
    let mut next_input = 1;
    if spec.audio {
        a.extend(args(["-f", "lavfi", "-i"]));
        a.push(format!("sine=frequency=440:sample_rate=44100:duration={seconds}").into());
        maps.push(args(["-map", &format!("{next_input}:a")]));
        next_input += 1;
    }
    if spec.cover_art {
        a.extend(args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x64:rate=1:duration=1",
        ]));
        maps.push(args(["-map", &format!("{next_input}:v")]));
    }
    a.extend(maps.into_iter().flatten());
    if odd {
        a.extend(args(["-filter:v:0"]));
        a.push(format!("format=yuv444p,crop={}:{}", spec.width, spec.height).into());
    }
    match spec.codec {
        Codec::H264 => {
            a.extend(args([
                "-c:v:0",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt:v:0",
                "yuv420p",
            ]));
            if let Some(crf) = spec.crf {
                a.extend(args(["-crf:v:0"]));
                a.push(crf.to_string().into());
            }
        }
        Codec::ProRes => a.extend(args(["-c:v:0", "prores_ks"])),
        Codec::Mpeg4 => a.extend(args(["-c:v:0", "mpeg4"])),
    }
    if spec.audio {
        a.extend(args(["-c:a", "aac", "-b:a", "128k"]));
    }
    if spec.cover_art {
        a.extend(args([
            "-c:v:1",
            "mjpeg",
            "-frames:v:1",
            "1",
            "-disposition:v:1",
            "attached_pic",
        ]));
    }

    let Some(degrees) = spec.rotate else {
        a.push(file(&out));
        run_ffmpeg(&a);
        return out;
    };
    // ffmpeg turns a rotation into pixels while it transcodes. Written like
    // this the stored size stays, and only the display matrix is added: encode
    // first, then copy the streams with `-display_rotation`.
    let unrotated = dir.join(format!("unrotated-{}", spec.name));
    a.push(file(&unrotated));
    run_ffmpeg(&a);
    let mut remux = args(["-display_rotation"]);
    remux.push(degrees.to_string().into());
    remux.extend(args(["-i"]));
    remux.push(file(&unrotated));
    remux.extend(args(["-map", "0", "-c", "copy"]));
    remux.push(file(&out));
    run_ffmpeg(&remux);
    fs::remove_file(&unrotated).expect("the unrotated clip is there");
    out
}

/// An audio-only `.m4a` of `seconds` seconds, with a picture attached as its
/// cover when `cover_art` is true: the picture is a video stream that is no
/// video.
pub fn make_audio(dir: &Path, name: &str, seconds: f64, cover_art: bool) -> PathBuf {
    let out = dir.join(name);
    let mut a = args(["-f", "lavfi", "-i"]);
    a.push(format!("sine=frequency=440:sample_rate=44100:duration={seconds}").into());
    if cover_art {
        a.extend(args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x64:rate=1:duration=1",
        ]));
        a.extend(args([
            "-map",
            "0:a",
            "-map",
            "1:v",
            "-c:v",
            "mjpeg",
            "-frames:v",
            "1",
        ]));
        a.extend(args(["-disposition:v", "attached_pic"]));
    }
    a.extend(args(["-c:a", "aac"]));
    a.push(file(&out));
    run_ffmpeg(&a);
    out
}
