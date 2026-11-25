//! The 10-second comparison: the "before" clip, the sample at the chosen CRF,
//! its PSNR against the before clip, and the figures the result column shows.
//!
//! Every ffmpeg call is built in [`crate::args`] and run by
//! [`crate::run::run_ffmpeg`]. A file this module started and did not finish
//! is removed before the error is returned, so a cancelled or failed run
//! leaves nothing behind.

use std::fs;
use std::path::{Path, PathBuf};

use crate::args::{
    SAMPLE_SECONDS, Span, before_args, display_copy_args, encode_args, even_size, psnr_args,
};
use crate::probe::VideoInfo;
use crate::run::{CancelFlag, RunError, run_ffmpeg};
use crate::settings::Settings;
use crate::tools::{PathError, Tools};

/// The "before" clip's file name in the work folder.
const BEFORE_FILE: &str = "before.mp4";

/// The padded PSNR reference's file name in the work folder.
const PSNR_REFERENCE_FILE: &str = "psnr-ref.mp4";

/// How many seconds the sample and the "before" clip cover: [`SAMPLE_SECONDS`],
/// or the whole clip when it is shorter. `None` when the duration is unknown.
pub fn span(info: &VideoInfo) -> Option<f64> {
    usable_duration(info).map(|duration| duration.min(SAMPLE_SECONDS))
}

/// The duration, when it is a positive number of seconds.
fn usable_duration(info: &VideoInfo) -> Option<f64> {
    info.duration.filter(|d| d.is_finite() && *d > 0.0)
}

/// A path the arguments refused is a failed run that never started ffmpeg.
pub(crate) fn refused(error: PathError) -> RunError {
    RunError::Failed {
        code: None,
        reason: error.to_string(),
    }
}

/// Remove a file that is not wanted: one this module was making, or (in the
/// app) a sample that a newer one replaced. A file that is not there is the
/// normal case (ffmpeg was stopped before it created it), and one that cannot
/// be removed leaves nothing better to do than go on: the caller reports its
/// own outcome.
pub fn discard(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Make the "before" clip, `before.mp4` in `dir` (the session's work folder),
/// and return its path. A cancelled or failed run leaves no file.
pub fn make_before(
    tools: &Tools,
    info: &VideoInfo,
    dir: &Path,
    cancel: &CancelFlag,
) -> Result<PathBuf, RunError> {
    let output = dir.join(BEFORE_FILE);
    let args = before_args(info, &output).map_err(refused)?;
    match run_ffmpeg(tools, &args, cancel, &mut |_| {}) {
        Ok(_) => Ok(output),
        Err(error) => {
            discard(&output);
            Err(error)
        }
    }
}

/// A finished sample.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleResult {
    pub path: PathBuf,
    pub bytes: u64,
    /// The average PSNR in dB against the "before" clip, `Some(INFINITY)` for
    /// identical pictures; `None` when it could not be measured.
    pub psnr: Option<f64>,
}

/// Encode the sample of `info` at `settings` into `dir/<name>.mp4` (`name` is
/// a file name without extension), and measure its PSNR against `before`.
///
/// A PSNR that cannot be measured gives `psnr: None` and still a sample. A
/// cancel, and any failure of the encode itself, return the error and leave no
/// sample file. When the cancel arrives during the PSNR run the finished sample
/// is removed too: the caller asked for no result.
pub fn make_sample(
    tools: &Tools,
    info: &VideoInfo,
    settings: Settings,
    before: &Path,
    dir: &Path,
    name: &str,
    cancel: &CancelFlag,
) -> Result<SampleResult, RunError> {
    let path = dir.join(format!("{name}.mp4"));
    let args = encode_args(info, settings, &path, Span::Sample).map_err(refused)?;
    let finished = run_ffmpeg(tools, &args, cancel, &mut |_| {}).and_then(|_| {
        // ffmpeg said it succeeded; a file that is not there is still a failure.
        fs::metadata(&path)
            .map(|meta| meta.len())
            .map_err(|error| RunError::Failed {
                code: None,
                reason: format!("the sample file is missing: {error}"),
            })
    });
    let bytes = match finished {
        Ok(bytes) => bytes,
        Err(error) => {
            discard(&path);
            return Err(error);
        }
    };
    match measure_psnr(tools, info, before, &path, dir, cancel) {
        Ok(psnr) => Ok(SampleResult { path, bytes, psnr }),
        Err(error) => {
            discard(&path);
            Err(error)
        }
    }
}

/// The PSNR of `sample` against `before`, or against the padded reference when
/// the "before" clip is a stream copy of an odd source (its size would not
/// match the padded sample). `Err` only for a cancel; every other failure is
/// `Ok(None)`.
fn measure_psnr(
    tools: &Tools,
    info: &VideoInfo,
    before: &Path,
    sample: &Path,
    dir: &Path,
    cancel: &CancelFlag,
) -> Result<Option<f64>, RunError> {
    let reference = match psnr_reference(tools, info, before, dir, cancel) {
        Ok(reference) => reference,
        Err(RunError::Cancelled) => return Err(RunError::Cancelled),
        Err(_) => return Ok(None),
    };
    let Ok(args) = psnr_args(sample, &reference) else {
        return Ok(None);
    };
    match run_ffmpeg(tools, &args, cancel, &mut |_| {}) {
        Ok(stderr) => Ok(parse_psnr(&stderr)),
        Err(RunError::Cancelled) => Err(RunError::Cancelled),
        Err(_) => Ok(None),
    }
}

/// The clip a sample is compared with: `before` itself, unless it is a stream
/// copy of an odd-sized source. The sample is padded to even sizes and PSNR
/// needs equal sizes, and a stream copy cannot be padded, so a padded copy of
/// the same seconds is made once in `dir` and kept for the later samples. A
/// run that is cancelled or fails deletes its copy (unless the delete itself
/// fails), and a copy that exists is used as it is. It is not checked for
/// being whole: while a run is still writing it (an older, cancelled sample's
/// run that has not stopped yet), a newer sample would read a partial copy.
/// That gives a wrong or unknown PSNR, never a wrong saved file.
fn psnr_reference(
    tools: &Tools,
    info: &VideoInfo,
    before: &Path,
    dir: &Path,
    cancel: &CancelFlag,
) -> Result<PathBuf, RunError> {
    let is_odd = even_size(info.width, info.height) != (info.width, info.height);
    // A converted before clip was padded when it was made.
    if info.needs_display_copy() || !is_odd {
        return Ok(before.to_path_buf());
    }
    let reference = dir.join(PSNR_REFERENCE_FILE);
    if reference.is_file() {
        return Ok(reference);
    }
    let args = display_copy_args(info, &reference).map_err(refused)?;
    match run_ffmpeg(tools, &args, cancel, &mut |_| {}) {
        Ok(_) => Ok(reference),
        Err(error) => {
            discard(&reference);
            Err(error)
        }
    }
}

/// The `average:` value of the last line of ffmpeg's output that contains
/// `PSNR` (`average:inf` is `Some(INFINITY)` for identical pictures). `None`
/// when there is no such line, or its average is not a number.
pub fn parse_psnr(stderr: &str) -> Option<f64> {
    let line = stderr.lines().rfind(|line| line.contains("PSNR"))?;
    let (_, after) = line.split_once("average:")?;
    let value: f64 = after.split_whitespace().next()?.parse().ok()?;
    // Rust also reads `nan` as a number.
    (!value.is_nan()).then_some(value)
}

/// The numbers of the result column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Figures {
    /// The original's share for the seconds the sample covers, in bytes.
    pub original_bytes: Option<u64>,
    pub sample_bytes: u64,
    /// How much smaller the sample is than that share, in whole percent;
    /// negative when it is bigger.
    pub smaller_by: Option<i32>,
    /// What the whole file would weigh at this sample's rate, in bytes.
    pub whole_bytes: Option<u64>,
}

/// The figures for a sample of `sample_bytes` of `info`.
///
/// "Original" is the input's bytes for the sample's seconds, `bytes × span ÷
/// duration`, and "whole file" is `sample_bytes × duration ÷ span`. Each is
/// `None` without a usable duration. The saving is also `None` when the
/// original share is empty, as there is nothing to compare with.
pub fn figures(info: &VideoInfo, sample_bytes: u64) -> Figures {
    let Some(duration) = usable_duration(info) else {
        return Figures {
            original_bytes: None,
            sample_bytes,
            smaller_by: None,
            whole_bytes: None,
        };
    };
    let span = duration.min(SAMPLE_SECONDS);
    let original_bytes = (info.bytes as f64 * span / duration).round() as u64;
    let smaller_by = (original_bytes > 0)
        .then(|| ((1.0 - sample_bytes as f64 / original_bytes as f64) * 100.0).round() as i32);
    let whole_bytes = (sample_bytes as f64 * duration / span).round() as u64;
    Figures {
        original_bytes: Some(original_bytes),
        sample_bytes,
        smaller_by,
        whole_bytes: Some(whole_bytes),
    }
}
