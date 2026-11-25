//! What the window receives: the event names and their JSON payloads.
//!
//! Every payload is plain data: a file by its name only, never its path, and
//! a failure by its reason. The window composes every text it shows from
//! these.

use std::path::Path;

use serde::Serialize;
use video_optimizer_core::probe::{ProbeError, VideoInfo};
use video_optimizer_core::run::RunError;
use video_optimizer_core::sample::Figures;
use video_optimizer_core::save::{SaveError, SaveOutcome};
use video_optimizer_core::settings::Crf;

/// The audio stream of a loaded video.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudioInfo {
    pub codec: String,
    /// Bits per second; `null` when the file does not say.
    pub bitrate: Option<u64>,
}

/// A video was loaded; `video` is its id, which every later command and
/// event about it carries.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoLoaded {
    pub video: u64,
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// `null` when unknown.
    pub fps: Option<f64>,
    /// Seconds; `null` when unknown.
    pub duration: Option<f64>,
    pub bytes: u64,
    /// `null` for a video without sound.
    pub audio: Option<AudioInfo>,
    /// The "before" is a converted copy, as the window cannot play the
    /// original.
    pub display_copy: bool,
}

/// The `video` payload for the video `id` that `info` describes.
pub fn video_loaded(id: u64, info: &VideoInfo) -> VideoLoaded {
    VideoLoaded {
        video: id,
        name: info.name.clone(),
        width: info.width,
        height: info.height,
        fps: info.fps,
        duration: info.duration,
        bytes: info.bytes,
        audio: info.audio.as_ref().map(|audio| AudioInfo {
            codec: audio.codec.clone(),
            bitrate: audio.bitrate,
        }),
        display_copy: info.needs_display_copy(),
    }
}

/// The before clip of `video` can be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BeforeReady {
    pub video: u64,
}

/// Why a drop or a choice loaded nothing, or that the before clip failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Notice {
    /// More than one file was dropped.
    SeveralFiles,
    /// A save is running.
    Saving,
    /// The file is not a video that can be read.
    Refused { name: String, reason: String },
    /// The file is larger than 10 GiB.
    TooLarge { name: String },
    /// The before clip could not be made.
    Failed { reason: String },
}

/// The notice for the file at `path` that `error` refused. An unreadable
/// file carries ffprobe's own message, the other refusals the core's
/// sentence.
pub fn refusal(path: &Path, error: &ProbeError) -> Notice {
    let name = file_name_of(path);
    match error {
        ProbeError::TooLarge { .. } => Notice::TooLarge { name },
        ProbeError::Unreadable(reason) => Notice::Refused {
            name,
            reason: reason.clone(),
        },
        ProbeError::NotAFile | ProbeError::NoVideo => Notice::Refused {
            name,
            reason: error.to_string(),
        },
    }
}

/// The last component of `path`, lossily decoded; empty if it has none (a
/// root such as `/`). Never the path.
pub fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A finished sample and its figures. Every key is always there; a value
/// that is not known is `null`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub video: u64,
    pub request: u64,
    pub crf: u8,
    pub original_bytes: Option<u64>,
    pub sample_bytes: u64,
    pub smaller_by: Option<i32>,
    pub whole_bytes: Option<u64>,
    /// dB; `null` when it could not be measured, and for identical pictures.
    pub psnr: Option<f64>,
    /// The sample's pictures are identical to the before clip's (an infinite
    /// PSNR, which JSON cannot hold).
    pub identical: bool,
}

/// The `sample` payload for request `request` of video `video`, made at
/// `crf`, with its `figures` and its `psnr` as the core measured it.
pub fn sample(video: u64, request: u64, crf: Crf, figures: &Figures, psnr: Option<f64>) -> Sample {
    Sample {
        video,
        request,
        crf: crf.get(),
        original_bytes: figures.original_bytes,
        sample_bytes: figures.sample_bytes,
        smaller_by: figures.smaller_by,
        whole_bytes: figures.whole_bytes,
        psnr: psnr.filter(|value| value.is_finite()),
        identical: psnr.is_some_and(f64::is_infinite),
    }
}

/// A sample request ended without a sample.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SampleFailed {
    pub video: u64,
    pub request: u64,
    pub reason: String,
}

/// How far the save is: a percent, or `null` when it cannot be known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SaveProgress {
    pub percent: Option<u8>,
}

/// The save finished and wrote `name`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveDone {
    pub name: String,
    pub bytes: u64,
    pub input_bytes: u64,
    pub kept_original: bool,
}

/// The save failed; nothing was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SaveFailed {
    pub reason: String,
}

/// The save was cancelled; nothing was written. Serialised as `{}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SaveCancelled {}

/// The event a finished save sends: `save-done`, `save-cancelled` or
/// `save-failed`. `input_bytes` is the original's size.
pub fn save_finished(input_bytes: u64, result: Result<SaveOutcome, SaveError>) -> Event {
    match result {
        Ok(outcome) => Event::SaveDone(SaveDone {
            name: outcome.name,
            bytes: outcome.bytes,
            input_bytes,
            kept_original: outcome.kept_original,
        }),
        Err(SaveError::Run(RunError::Cancelled)) => Event::SaveCancelled(SaveCancelled {}),
        Err(error) => Event::SaveFailed(SaveFailed {
            reason: error.to_string(),
        }),
    }
}

/// One event for the window: its name ([`Event::name`]) and its payload (the
/// `Serialize` output).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Event {
    Video(VideoLoaded),
    BeforeReady(BeforeReady),
    Notice(Notice),
    Sample(Sample),
    SampleFailed(SampleFailed),
    SaveProgress(SaveProgress),
    SaveDone(SaveDone),
    SaveFailed(SaveFailed),
    SaveCancelled(SaveCancelled),
}

impl Event {
    /// The event name the window listens to.
    pub fn name(&self) -> &'static str {
        match self {
            Event::Video(_) => "video",
            Event::BeforeReady(_) => "before-ready",
            Event::Notice(_) => "notice",
            Event::Sample(_) => "sample",
            Event::SampleFailed(_) => "sample-failed",
            Event::SaveProgress(_) => "save-progress",
            Event::SaveDone(_) => "save-done",
            Event::SaveFailed(_) => "save-failed",
            Event::SaveCancelled(_) => "save-cancelled",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::{Value, json};
    use video_optimizer_core::probe::AudioStream;

    use super::*;

    fn to_json(event: &Event) -> Value {
        serde_json::to_value(event).unwrap()
    }

    fn showreel() -> VideoInfo {
        VideoInfo {
            path: PathBuf::from("/videos/Showreel V1.mp4"),
            name: "Showreel V1.mp4".into(),
            bytes: 96_900_000,
            stream_index: 0,
            width: 3840,
            height: 2160,
            fps: Some(24.0),
            duration: Some(36.04),
            video_codec: "h264".into(),
            pix_fmt: "yuv420p".into(),
            audio: Some(AudioStream {
                codec: "aac".into(),
                bitrate: Some(317_000),
            }),
        }
    }

    // ---- video ----

    #[test]
    fn a_video_payload_matches_the_example() {
        let event = Event::Video(video_loaded(3, &showreel()));
        assert_eq!(event.name(), "video");
        assert_eq!(
            to_json(&event),
            json!({"video": 3, "name": "Showreel V1.mp4", "width": 3840, "height": 2160,
                   "fps": 24.0, "duration": 36.04, "bytes": 96900000,
                   "audio": {"codec": "aac", "bitrate": 317000}, "displayCopy": false})
        );
    }

    #[test]
    fn a_video_without_audio_duration_or_fps_has_nulls_and_a_display_copy_when_needed() {
        let mut info = showreel();
        info.audio = None;
        info.duration = None;
        info.fps = None;
        info.video_codec = "prores".into();
        info.pix_fmt = "yuv422p10le".into();
        let value = to_json(&Event::Video(video_loaded(4, &info)));
        let object = value.as_object().expect("an object");
        assert_eq!(object.len(), 9, "{value}");
        assert_eq!(value["video"], 4);
        assert_eq!(value["audio"], Value::Null);
        assert_eq!(value["duration"], Value::Null);
        assert_eq!(value["fps"], Value::Null);
        assert_eq!(value["displayCopy"], true);
    }

    #[test]
    fn an_audio_stream_without_a_bitrate_has_a_null_bitrate() {
        let mut info = showreel();
        info.audio = Some(AudioStream {
            codec: "opus".into(),
            bitrate: None,
        });
        let value = to_json(&Event::Video(video_loaded(1, &info)));
        assert_eq!(value["audio"], json!({"codec": "opus", "bitrate": null}));
    }

    // ---- before-ready and notice ----

    #[test]
    fn before_ready_and_the_notices_have_their_shapes() {
        let before = Event::BeforeReady(BeforeReady { video: 3 });
        assert_eq!(before.name(), "before-ready");
        assert_eq!(to_json(&before), json!({"video": 3}));
        for (notice, wanted) in [
            (Notice::SeveralFiles, json!({"kind": "several-files"})),
            (Notice::Saving, json!({"kind": "saving"})),
            (
                Notice::Refused {
                    name: "notes.mp4".into(),
                    reason: "Invalid data found when processing input".into(),
                },
                json!({"kind": "refused", "name": "notes.mp4",
                       "reason": "Invalid data found when processing input"}),
            ),
            (
                Notice::TooLarge {
                    name: "huge.mov".into(),
                },
                json!({"kind": "too-large", "name": "huge.mov"}),
            ),
            (
                Notice::Failed {
                    reason: "ffmpeg failed: x".into(),
                },
                json!({"kind": "failed", "reason": "ffmpeg failed: x"}),
            ),
        ] {
            let event = Event::Notice(notice);
            assert_eq!(event.name(), "notice");
            assert_eq!(to_json(&event), wanted);
        }
    }

    #[test]
    fn a_refusal_carries_the_file_name_and_ffprobes_own_reason() {
        let path = Path::new("/home/me/notes.mp4");
        let reason = "Invalid data found when processing input";
        assert_eq!(
            refusal(path, &ProbeError::Unreadable(reason.into())),
            Notice::Refused {
                name: "notes.mp4".into(),
                reason: reason.into()
            }
        );
        assert_eq!(
            refusal(path, &ProbeError::NoVideo),
            Notice::Refused {
                name: "notes.mp4".into(),
                reason: "this file has no video".into()
            }
        );
        assert_eq!(
            refusal(Path::new("/home/me/folder"), &ProbeError::NotAFile),
            Notice::Refused {
                name: "folder".into(),
                reason: "this is not a file".into()
            }
        );
    }

    #[test]
    fn a_file_over_the_limit_is_too_large_not_refused() {
        let notice = refusal(
            Path::new("/v/huge.mov"),
            &ProbeError::TooLarge { bytes: 11 << 30 },
        );
        assert_eq!(
            notice,
            Notice::TooLarge {
                name: "huge.mov".into()
            }
        );
    }

    // ---- sample ----

    fn full_figures() -> Figures {
        Figures {
            original_bytes: Some(26_916_667),
            sample_bytes: 11_300_000,
            smaller_by: Some(58),
            whole_bytes: Some(40_680_000),
        }
    }

    #[test]
    fn a_full_payload_matches_the_example() {
        let crf = Crf::new(28).unwrap();
        let event = Event::Sample(sample(3, 7, crf, &full_figures(), Some(42.41)));
        assert_eq!(event.name(), "sample");
        assert_eq!(
            to_json(&event),
            json!({"video": 3, "request": 7, "crf": 28, "originalBytes": 26916667,
                   "sampleBytes": 11300000, "smallerBy": 58, "wholeBytes": 40680000,
                   "psnr": 42.41, "identical": false})
        );
    }

    #[test]
    fn sample_payload_always_has_every_key() {
        let figures = Figures {
            original_bytes: None,
            sample_bytes: 5,
            smaller_by: None,
            whole_bytes: None,
        };
        let crf = Crf::new(20).unwrap();
        let value = to_json(&Event::Sample(sample(1, 2, crf, &figures, None)));
        assert_eq!(
            value,
            json!({"video": 1, "request": 2, "crf": 20, "originalBytes": null,
                   "sampleBytes": 5, "smallerBy": null, "wholeBytes": null,
                   "psnr": null, "identical": false})
        );
    }

    #[test]
    fn an_infinite_psnr_is_sent_as_null_and_identical() {
        let crf = Crf::new(36).unwrap();
        let value = to_json(&Event::Sample(sample(
            1,
            1,
            crf,
            &full_figures(),
            Some(f64::INFINITY),
        )));
        assert_eq!(value["psnr"], Value::Null);
        assert_eq!(value["identical"], true);
        // A finite PSNR is not identical, however high.
        let high = to_json(&Event::Sample(sample(
            1,
            1,
            crf,
            &full_figures(),
            Some(99.0),
        )));
        assert_eq!(high["psnr"], 99.0);
        assert_eq!(high["identical"], false);
    }

    #[test]
    fn a_failed_sample_has_its_shape() {
        let event = Event::SampleFailed(SampleFailed {
            video: 3,
            request: 7,
            reason: "ffmpeg failed: x".into(),
        });
        assert_eq!(event.name(), "sample-failed");
        assert_eq!(
            to_json(&event),
            json!({"video": 3, "request": 7, "reason": "ffmpeg failed: x"})
        );
    }

    // ---- save ----

    #[test]
    fn save_progress_is_a_percent_or_null() {
        let known = Event::SaveProgress(SaveProgress { percent: Some(42) });
        assert_eq!(known.name(), "save-progress");
        assert_eq!(to_json(&known), json!({"percent": 42}));
        let unknown = Event::SaveProgress(SaveProgress { percent: None });
        assert_eq!(to_json(&unknown), json!({"percent": null}));
    }

    #[test]
    fn a_finished_save_is_save_done() {
        let outcome = SaveOutcome {
            path: PathBuf::from("/videos/optimized-Showreel V1.mp4"),
            name: "optimized-Showreel V1.mp4".into(),
            bytes: 40_800_000,
            kept_original: false,
        };
        let event = save_finished(96_900_000, Ok(outcome));
        assert_eq!(event.name(), "save-done");
        assert_eq!(
            to_json(&event),
            json!({"name": "optimized-Showreel V1.mp4", "bytes": 40800000,
                   "inputBytes": 96900000, "keptOriginal": false})
        );
    }

    #[test]
    fn a_cancelled_save_is_save_cancelled_with_an_empty_object() {
        let event = save_finished(1, Err(SaveError::Run(RunError::Cancelled)));
        assert_eq!(event.name(), "save-cancelled");
        assert_eq!(to_json(&event), json!({}));
    }

    #[test]
    fn a_failed_save_is_save_failed_with_the_reason() {
        let failed = save_finished(
            1,
            Err(SaveError::Run(RunError::Failed {
                code: Some(1),
                reason: "No space left on device".into(),
            })),
        );
        assert_eq!(failed.name(), "save-failed");
        assert_eq!(
            to_json(&failed),
            json!({"reason": "ffmpeg failed: No space left on device"})
        );
        let no_name = save_finished(1, Err(SaveError::NoFreeName));
        assert_eq!(
            to_json(&no_name),
            json!({"reason": "every output name is taken"})
        );
    }
}
