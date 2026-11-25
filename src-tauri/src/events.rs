//! What the window receives: the event names and their JSON payloads.
//!
//! Every payload is plain data: a file by its name only, never its path, and
//! a failure by its reason, with every folder the backend knows of taken out
//! ([`without_folders`], [`io_reason`]). The window composes every text it
//! shows from these.

use std::io;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

use serde::Serialize;
use video_optimizer_core::probe::{ProbeError, VideoInfo};
use video_optimizer_core::run::RunError;
use video_optimizer_core::sample::Figures;
use video_optimizer_core::save::{SaveError, SaveOutcome};
use video_optimizer_core::settings::Crf;
use video_optimizer_core::tools::absolute;

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
/// file carries ffprobe's own message, without the file's folder or any of
/// `folders` (the core names ffprobe by its path when it cannot start it),
/// and without the `<name>: ` ffprobe puts first. The other refusals carry
/// the core's sentence.
pub fn refusal(path: &Path, error: &ProbeError, folders: &[PathBuf]) -> Notice {
    let name = file_name_of(path);
    match error {
        ProbeError::TooLarge { .. } => Notice::TooLarge { name },
        ProbeError::Unreadable(reason) => {
            // ffprobe was given the absolute path, as `file:<path>`.
            let own_folder = absolute(path)
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf));
            let folders: Vec<PathBuf> = folders.iter().cloned().chain(own_folder).collect();
            let reason = without_folders(reason, &folders);
            let reason = match reason.strip_prefix(&format!("{name}: ")) {
                Some(rest) => rest.to_owned(),
                None => reason,
            };
            Notice::Refused { name, reason }
        }
        ProbeError::NotAFile | ProbeError::NoVideo => Notice::Refused {
            name,
            reason: error.to_string(),
        },
    }
}

/// `text` with each of `folders` taken out, so the window hears file names
/// and never where they are. A folder goes together with the separator after
/// it, and with a `file:` before it (how ffmpeg and ffprobe name their
/// files). The longest folder goes first, so a folder inside another one
/// leaves nothing behind. The Unix root folder (`/`) is not taken out, as
/// that would take every separator: without its trailing separator it is
/// empty and skipped. A Windows drive root (`C:\`) is taken out, as `C:` with
/// the separator after it.
pub fn without_folders(text: &str, folders: &[PathBuf]) -> String {
    let mut folders: Vec<String> = folders
        .iter()
        .map(|folder| {
            let folder = folder.to_string_lossy();
            folder.trim_end_matches(MAIN_SEPARATOR).to_owned()
        })
        .filter(|folder| !folder.is_empty())
        .collect();
    folders.sort_by_key(|folder| std::cmp::Reverse(folder.len()));
    let mut text = text.to_owned();
    for folder in folders {
        let inside = format!("{folder}{MAIN_SEPARATOR}");
        text = text
            .replace(&format!("file:{inside}"), "")
            .replace(&inside, "");
    }
    text
}

/// A file error as the window is told it: the system's own sentence ("No
/// space left on device") without its code, or, when the error wraps it (as
/// tempfile does, adding the path), the sentence for its kind. Never the
/// path an error may carry in its text.
pub fn io_reason(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => {
            let sentence = io::Error::from_raw_os_error(code).to_string();
            let code = format!(" (os error {code})");
            sentence.strip_suffix(&code).unwrap_or(&sentence).to_owned()
        }
        None => error.kind().to_string(),
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
///
/// A failure's reason names no folder: ffmpeg's own last line without
/// `ffmpeg failed: ` and without `folders`, a file error as [`io_reason`]
/// gives it, and the core's sentence otherwise.
pub fn save_finished(
    input_bytes: u64,
    result: Result<SaveOutcome, SaveError>,
    folders: &[PathBuf],
) -> Event {
    let error = match result {
        Ok(outcome) => {
            return Event::SaveDone(SaveDone {
                name: outcome.name,
                bytes: outcome.bytes,
                input_bytes,
                kept_original: outcome.kept_original,
            });
        }
        Err(SaveError::Run(RunError::Cancelled)) => {
            return Event::SaveCancelled(SaveCancelled {});
        }
        Err(error) => error,
    };
    let reason = match &error {
        SaveError::Run(RunError::Failed { reason, .. }) if reason.is_empty() => {
            "ffmpeg failed".to_owned()
        }
        SaveError::Run(RunError::Failed { reason, .. }) => without_folders(reason, folders),
        SaveError::Run(RunError::Start(start)) => {
            format!("ffmpeg could not be started: {}", io_reason(start))
        }
        SaveError::Io(io) => io_reason(io),
        SaveError::Run(RunError::Cancelled) | SaveError::NoFreeName => error.to_string(),
    };
    Event::SaveFailed(SaveFailed { reason })
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

    /// `/home/me/My Videos`, in the platform's own form.
    fn my_videos() -> PathBuf {
        Path::new("/home/me").join("My Videos")
    }

    #[test]
    fn a_refusal_carries_the_file_name_and_ffprobes_own_reason() {
        let path = my_videos().join("notes.mp4");
        let reason = "Invalid data found when processing input";
        assert_eq!(
            refusal(&path, &ProbeError::Unreadable(reason.into()), &[]),
            Notice::Refused {
                name: "notes.mp4".into(),
                reason: reason.into()
            }
        );
        assert_eq!(
            refusal(&path, &ProbeError::NoVideo, &[]),
            Notice::Refused {
                name: "notes.mp4".into(),
                reason: "this file has no video".into()
            }
        );
        assert_eq!(
            refusal(&my_videos(), &ProbeError::NotAFile, &[]),
            Notice::Refused {
                name: "My Videos".into(),
                reason: "this is not a file".into()
            }
        );
    }

    #[test]
    fn a_refusal_drops_the_probed_path_that_ffprobe_puts_first() {
        let path = my_videos().join("notes.mp4");
        let wanted = Notice::Refused {
            name: "notes.mp4".into(),
            reason: "Invalid data found when processing input".into(),
        };
        // As ffprobe prints it: the argument it was given, then the reason.
        for given in [
            format!(
                "file:{}: Invalid data found when processing input",
                path.display()
            ),
            format!(
                "{}: Invalid data found when processing input",
                path.display()
            ),
        ] {
            assert_eq!(
                refusal(&path, &ProbeError::Unreadable(given.clone()), &[]),
                wanted,
                "{given}"
            );
        }
    }

    #[test]
    fn a_refusal_names_no_folder_it_is_given() {
        // ffprobe could not be started: core names the program by its path.
        let tools = Path::new("/opt").join("tools");
        let given = format!(
            "{} could not be started: Permission denied (os error 13)",
            tools.join("ffprobe").display()
        );
        let notice = refusal(
            &my_videos().join("a.mp4"),
            &ProbeError::Unreadable(given),
            &[tools],
        );
        assert_eq!(
            notice,
            Notice::Refused {
                name: "a.mp4".into(),
                reason: "ffprobe could not be started: Permission denied (os error 13)".into()
            }
        );
    }

    #[test]
    fn a_reason_without_a_known_folder_is_left_as_it_is() {
        let text = "moov atom not found: 1/2 of the file";
        assert_eq!(without_folders(text, &[my_videos()]), text);
        assert_eq!(without_folders("", &[my_videos()]), "");
    }

    #[test]
    fn without_folders_removes_each_folder_with_or_without_file_prefix() {
        let other = Path::new("/srv").join("out");
        let text = format!(
            "Error opening input file file:{}. Writing {}",
            my_videos().join("a b.mp4").display(),
            other.join(".optimized-x.part").display()
        );
        assert_eq!(
            without_folders(&text, &[my_videos(), other]),
            "Error opening input file a b.mp4. Writing .optimized-x.part"
        );
    }

    #[test]
    fn without_folders_takes_the_longest_folder_first() {
        // The work folder sits inside the work root: removing the root first
        // would leave the work folder's own name and a separator.
        let root = Path::new("/tmp").join("work");
        let inner = root.join("video-optimizer-ab12");
        let text = format!("{}: failed", inner.join("before.mp4").display());
        let cleaned = without_folders(&text, &[root, inner]);
        assert_eq!(cleaned, "before.mp4: failed");
    }

    #[test]
    fn io_reason_of_a_tempfile_error_is_the_kind_sentence_without_its_path() {
        // tempfile wraps the system's error with the path and hides its code;
        // only the kind is left.
        let missing = my_videos().join("nonexistent-folder-for-a-test");
        let error = tempfile::Builder::new().tempdir_in(&missing).unwrap_err();
        assert!(
            error.to_string().contains("nonexistent-folder-for-a-test"),
            "the fixture carries the path: {error}"
        );
        let reason = io_reason(&error);
        assert_eq!(reason, std::io::ErrorKind::NotFound.to_string());
        assert!(!reason.contains("nonexistent"), "{reason}");
    }

    #[cfg(unix)]
    #[test]
    fn io_reason_of_a_system_error_is_its_sentence_without_the_code() {
        let full = std::io::Error::from_raw_os_error(28);
        assert_eq!(full.to_string(), "No space left on device (os error 28)");
        assert_eq!(io_reason(&full), "No space left on device");
    }

    #[test]
    fn io_reason_without_a_system_code_is_the_kind_sentence() {
        let error = std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} has no name", my_videos().display()),
        );
        let reason = io_reason(&error);
        assert_eq!(reason, std::io::ErrorKind::InvalidInput.to_string());
        assert!(!reason.contains("My Videos"), "{reason}");
    }

    #[test]
    fn a_file_over_the_limit_is_too_large_not_refused() {
        let notice = refusal(
            Path::new("/v/huge.mov"),
            &ProbeError::TooLarge { bytes: 11 << 30 },
            &[],
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
        let event = save_finished(96_900_000, Ok(outcome), &[]);
        assert_eq!(event.name(), "save-done");
        assert_eq!(
            to_json(&event),
            json!({"name": "optimized-Showreel V1.mp4", "bytes": 40800000,
                   "inputBytes": 96900000, "keptOriginal": false})
        );
    }

    #[test]
    fn a_cancelled_save_is_save_cancelled_with_an_empty_object() {
        let event = save_finished(1, Err(SaveError::Run(RunError::Cancelled)), &[]);
        assert_eq!(event.name(), "save-cancelled");
        assert_eq!(to_json(&event), json!({}));
    }

    #[test]
    fn a_failed_save_is_save_failed_with_ffmpegs_bare_reason() {
        let failed = save_finished(
            1,
            Err(SaveError::Run(RunError::Failed {
                code: Some(1),
                reason: "No space left on device".into(),
            })),
            &[],
        );
        assert_eq!(failed.name(), "save-failed");
        assert_eq!(
            to_json(&failed),
            json!({"reason": "No space left on device"})
        );
        let no_name = save_finished(1, Err(SaveError::NoFreeName), &[]);
        assert_eq!(
            to_json(&no_name),
            json!({"reason": "every output name is taken"})
        );
    }

    #[test]
    fn a_failed_save_names_no_folder_it_is_given() {
        let out = Path::new("/srv").join("Exports");
        let reason = format!(
            "file:{}: No space left on device",
            out.join(".optimized-ab.part").display()
        );
        let failed = save_finished(
            1,
            Err(SaveError::Run(RunError::Failed {
                code: Some(1),
                reason,
            })),
            &[out],
        );
        assert_eq!(
            to_json(&failed),
            json!({"reason": ".optimized-ab.part: No space left on device"})
        );
    }

    #[test]
    fn a_save_whose_ffmpeg_said_nothing_still_has_a_reason() {
        let failed = save_finished(
            1,
            Err(SaveError::Run(RunError::Failed {
                code: None,
                reason: String::new(),
            })),
            &[],
        );
        assert_eq!(to_json(&failed), json!({"reason": "ffmpeg failed"}));
    }

    #[test]
    fn a_save_that_could_not_start_ffmpeg_says_so_without_a_path() {
        let failed = save_finished(
            1,
            Err(SaveError::Run(RunError::Start(
                std::io::Error::from_raw_os_error(2),
            ))),
            &[],
        );
        let Event::SaveFailed(SaveFailed { reason }) = failed else {
            panic!("expected save-failed")
        };
        let system = io_reason(&std::io::Error::from_raw_os_error(2));
        assert!(
            !system.is_empty() && !system.contains("os error"),
            "{system}"
        );
        assert_eq!(reason, format!("ffmpeg could not be started: {system}"));
    }

    #[test]
    fn a_save_that_could_not_write_sends_no_path() {
        let missing = my_videos().join("nonexistent-export-folder");
        let error = tempfile::Builder::new().tempfile_in(&missing).unwrap_err();
        assert!(error.to_string().contains("nonexistent-export-folder"));
        let failed = save_finished(1, Err(SaveError::Io(error)), &[]);
        assert_eq!(
            to_json(&failed),
            json!({"reason": std::io::ErrorKind::NotFound.to_string()})
        );
    }
}
