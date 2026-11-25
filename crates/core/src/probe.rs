//! Reading what is in the input video, with ffprobe, before anything is
//! encoded: its size as it is shown, frame rate, length, codec and audio.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::tools::{Tools, absolute, command, file_arg};

/// The largest input accepted: 10 GiB.
pub const MAX_INPUT_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// The most characters of ffprobe's message that are kept.
const MAX_REASON_CHARS: usize = 300;

/// What ffprobe found in the input video.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    /// The file, as an absolute path.
    pub path: PathBuf,
    /// The file's name, without its folder.
    pub name: String,
    /// The size of the file.
    pub bytes: u64,
    /// The `index` of the video stream that was chosen.
    pub stream_index: u32,
    /// The size as it is shown: width and height are swapped when the file
    /// says the picture is turned a quarter turn.
    pub width: u32,
    pub height: u32,
    pub fps: Option<f64>,
    /// Seconds.
    pub duration: Option<f64>,
    pub video_codec: String,
    pub pix_fmt: String,
    pub audio: Option<AudioStream>,
}

/// The first audio stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioStream {
    pub codec: String,
    /// Bits per second, when the file says.
    pub bitrate: Option<u64>,
}

impl VideoInfo {
    /// Whether the window cannot play this file as it is. It plays 8-bit
    /// H.264 (`yuv420p`, or `yuvj420p` for full range); anything else needs a
    /// converted copy to be shown.
    pub fn needs_display_copy(&self) -> bool {
        let plays =
            self.video_codec == "h264" && matches!(self.pix_fmt.as_str(), "yuv420p" | "yuvj420p");
        !plays
    }
}

/// Why a file was refused. The message is shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProbeError {
    #[error("this is not a file")]
    NotAFile,
    #[error("this file is larger than 10 GiB")]
    TooLarge { bytes: u64 },
    #[error("this file has no video")]
    NoVideo,
    /// ffprobe could not read it. Carries ffprobe's last message.
    #[error("this file could not be read: {0}")]
    Unreadable(String),
}

// The parts of ffprobe's JSON that are read. Everything else is ignored.

#[derive(Deserialize)]
struct Output {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<Format>,
}

#[derive(Deserialize)]
struct Format {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct Stream {
    index: u32,
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    avg_frame_rate: Option<String>,
    bit_rate: Option<String>,
    #[serde(default)]
    disposition: Disposition,
    #[serde(default)]
    tags: Tags,
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[derive(Deserialize, Default)]
struct Disposition {
    #[serde(default)]
    attached_pic: u8,
}

#[derive(Deserialize, Default)]
struct Tags {
    rotate: Option<String>,
}

#[derive(Deserialize)]
struct SideData {
    /// Present on the display matrix, in degrees.
    rotation: Option<f64>,
}

impl Stream {
    fn is(&self, codec_type: &str) -> bool {
        self.codec_type.as_deref() == Some(codec_type)
    }

    /// How far the picture is turned, in degrees: the display matrix, else the
    /// old `rotate` tag.
    fn rotation(&self) -> f64 {
        self.side_data_list
            .iter()
            .find_map(|data| data.rotation)
            .or_else(|| self.tags.rotate.as_deref()?.trim().parse().ok())
            .unwrap_or(0.0)
    }
}

/// Whether a rotation of `degrees` turns the picture a quarter turn, so that
/// it is shown with its width and height swapped.
fn is_quarter_turn(degrees: f64) -> bool {
    // `as` saturates, so NaN and infinities land on a value that is no turn.
    matches!((degrees.round() as i64).rem_euclid(360), 90 | 270)
}

/// A frame rate written `n/d`. `None` when `d` is 0, or the value is not a
/// positive number.
fn frame_rate(text: Option<&str>) -> Option<f64> {
    let (n, d) = text?.split_once('/')?;
    let fps = n.parse::<f64>().ok()? / d.parse::<f64>().ok()?;
    (fps.is_finite() && fps > 0.0).then_some(fps)
}

/// Seconds, from the text ffprobe gives (`"36.041667"`). `None` when it is
/// absent (`"N/A"`), not a number, or not above 0.
fn seconds(text: Option<&str>) -> Option<f64> {
    let value = text?.trim().parse::<f64>().ok()?;
    (value.is_finite() && value > 0.0).then_some(value)
}

/// Read ffprobe's JSON for the file at `path`, which is `bytes` bytes long.
/// The video is the first video stream that is not a cover picture.
pub fn parse_probe(json: &str, path: PathBuf, bytes: u64) -> Result<VideoInfo, ProbeError> {
    let output: Output = serde_json::from_str(json).map_err(|error| {
        ProbeError::Unreadable(format!("ffprobe's answer was not understood: {error}"))
    })?;
    let video = output
        .streams
        .iter()
        .find(|stream| stream.is("video") && stream.disposition.attached_pic != 1)
        .ok_or(ProbeError::NoVideo)?;
    let (Some(width), Some(height)) = (video.width, video.height) else {
        return Err(ProbeError::Unreadable("the video has no size".into()));
    };
    if width == 0 || height == 0 {
        return Err(ProbeError::Unreadable("the video has no size".into()));
    }
    let (width, height) = if is_quarter_turn(video.rotation()) {
        (height, width)
    } else {
        (width, height)
    };
    let audio = output
        .streams
        .iter()
        .find(|stream| stream.is("audio"))
        .map(|stream| AudioStream {
            codec: stream.codec_name.clone().unwrap_or_default(),
            bitrate: stream
                .bit_rate
                .as_deref()
                .and_then(|text| text.trim().parse().ok()),
        });
    Ok(VideoInfo {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path,
        bytes,
        stream_index: video.index,
        width,
        height,
        fps: frame_rate(video.avg_frame_rate.as_deref()),
        duration: seconds(
            output
                .format
                .as_ref()
                .and_then(|format| format.duration.as_deref()),
        ),
        video_codec: video.codec_name.clone().unwrap_or_default(),
        pix_fmt: video.pix_fmt.clone().unwrap_or_default(),
        audio,
    })
}

/// The last line with text in ffprobe's stderr, trimmed and cut to
/// [`MAX_REASON_CHARS`] characters. `None` when there is no text.
fn reason(stderr: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| line.chars().take(MAX_REASON_CHARS).collect())
}

/// `path` as an absolute path, and as the argument that names it to ffprobe.
/// `None` for a path with no absolute form (the empty path); `absolute` never
/// gives a relative one, so `file_arg` refuses nothing else.
fn absolute_with_arg(path: &Path) -> Option<(PathBuf, OsString)> {
    let path = absolute(path).ok()?;
    let arg = file_arg(&path).ok()?;
    Some((path, arg))
}

/// Run ffprobe on the file at `path`. The file must be a regular file of at
/// most [`MAX_INPUT_BYTES`]; both are checked before ffprobe is started.
pub fn probe(tools: &Tools, path: &Path) -> Result<VideoInfo, ProbeError> {
    let (path, arg) = absolute_with_arg(path).ok_or(ProbeError::NotAFile)?;
    let metadata = fs::metadata(&path).map_err(|_| ProbeError::NotAFile)?;
    if !metadata.is_file() {
        return Err(ProbeError::NotAFile);
    }
    if metadata.len() > MAX_INPUT_BYTES {
        return Err(ProbeError::TooLarge {
            bytes: metadata.len(),
        });
    }
    // `output()` reads stdout and stderr together, so ffprobe never blocks on
    // a full pipe.
    let output = command(&tools.ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(&arg)
        .output()
        .map_err(|error| {
            ProbeError::Unreadable(format!(
                "{} could not be started: {error}",
                tools.ffprobe.display()
            ))
        })?;
    if !output.status.success() {
        let reason = reason(&output.stderr)
            .unwrap_or_else(|| format!("ffprobe stopped with {}", output.status));
        return Err(ProbeError::Unreadable(reason));
    }
    parse_probe(
        &String::from_utf8_lossy(&output.stdout),
        path,
        metadata.len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_is_the_last_line_with_text_trimmed() {
        assert_eq!(
            reason(b"first\n  the real reason  \n\n").as_deref(),
            Some("the real reason")
        );
        assert_eq!(reason(b"a\r\nb\r\n").as_deref(), Some("b"));
        assert_eq!(reason(b"only").as_deref(), Some("only"));
    }

    #[test]
    fn reason_is_none_when_there_is_no_text() {
        assert_eq!(reason(b""), None);
        assert_eq!(reason(b" \n\t\r\n"), None);
    }

    #[test]
    fn reason_is_cut_at_300_characters_not_bytes() {
        let ascii = "x".repeat(400);
        assert_eq!(reason(ascii.as_bytes()).unwrap().chars().count(), 300);
        let accented = "é".repeat(400);
        let cut = reason(accented.as_bytes()).unwrap();
        assert_eq!(cut, "é".repeat(300));
        // Exactly 300 is kept whole.
        let exact = "y".repeat(300);
        assert_eq!(reason(exact.as_bytes()).unwrap(), exact);
    }

    #[test]
    fn reason_survives_bytes_that_are_not_utf8() {
        let text = reason(&[0xff, b'o', b'k']).unwrap();
        assert!(text.ends_with("ok"), "{text}");
    }
}
