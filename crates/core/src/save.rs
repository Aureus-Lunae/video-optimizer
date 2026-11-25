//! Saving the whole video: encode into a temporary file in the target folder,
//! then move it to the first free `optimized-<stem>[-n].<ext>` name. Nothing
//! that exists is ever overwritten, and a cancelled or failed save leaves the
//! folder as it was.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use tempfile::{Builder, NamedTempFile};

use crate::args::{Span, encode_args};
use crate::probe::VideoInfo;
use crate::progress::percent;
use crate::run::{CancelFlag, RunError, run_ffmpeg};
use crate::sample::refused;
use crate::settings::Settings;
use crate::tools::{Tools, absolute};

/// The highest number tried after the plain name: `optimized-a-9999.mp4`.
pub const MAX_SUFFIX: u32 = 9999;

/// The extension of an encoded output.
const OUTPUT_EXTENSION: &str = "mp4";

/// What a finished save wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveOutcome {
    /// The new file.
    pub path: PathBuf,
    /// The new file's name, without its folder.
    pub name: String,
    /// The new file's size.
    pub bytes: u64,
    /// The encode was not smaller than the original, so the original's bytes
    /// were copied instead.
    pub kept_original: bool,
}

/// Why a save did not finish. Nothing is left in the folder in any case.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    /// ffmpeg did not finish: it was cancelled, failed or could not start.
    #[error(transparent)]
    Run(#[from] RunError),
    /// A file could not be made, read, copied or moved.
    #[error("the file could not be saved: {0}")]
    Io(#[from] io::Error),
    /// `optimized-<stem>.<ext>` and the names with `-1` to [`MAX_SUFFIX`] are
    /// all taken.
    #[error("every output name is taken")]
    NoFreeName,
}

/// `optimized-<stem>.<extension>` for `n == 0`, else
/// `optimized-<stem>-<n>.<extension>`. The stem is kept as it is, so a name
/// with spaces, non-ASCII characters or a leading `-` is not changed.
pub fn candidate_name(stem: &OsStr, extension: &str, n: u32) -> OsString {
    let mut name = OsString::from("optimized-");
    name.push(stem);
    if n > 0 {
        name.push(format!("-{n}"));
    }
    name.push(".");
    name.push(extension);
    name
}

/// Encode all of `info` at `settings` and save it as `optimized-<stem>.mp4`
/// (or the first free numbered name) in `out_dir`, or in the original's folder
/// when that is `None`.
///
/// ffmpeg writes to a hidden `.optimized-*.part` file in that folder, so the
/// final move stays on one file system. When the encode is not smaller than
/// the original, a copy of the original's bytes is saved instead, with the
/// original's own extension (lower case; `mp4` when it has none), and
/// `kept_original` is set. `on_progress` gets the percent of the duration
/// ffmpeg has written, and `100` once ffmpeg has finished; it gets `None`
/// each time when the duration is unknown.
///
/// A cancel, a failure and every error remove the temporary files before
/// returning, so the folder is as it was. A cancel that comes after this
/// returned `Ok` changes nothing.
pub fn save(
    tools: &Tools,
    info: &VideoInfo,
    settings: Settings,
    out_dir: Option<&Path>,
    cancel: &CancelFlag,
    on_progress: &mut dyn FnMut(Option<u8>),
) -> Result<SaveOutcome, SaveError> {
    let folder = match out_dir {
        Some(dir) => absolute(dir)?,
        None => info
            .path
            .parent()
            .ok_or_else(|| no_name("the original has no folder"))?
            .to_path_buf(),
    };
    let stem = info
        .path
        .file_stem()
        .ok_or_else(|| no_name("the original has no file name"))?;

    // Dropping a `NamedTempFile` deletes its file, on every return below.
    let encoded = temp_file_in(&folder)?;
    let args = encode_args(info, settings, encoded.path(), Span::Whole).map_err(refused)?;
    run_ffmpeg(tools, &args, cancel, &mut |out_us| {
        on_progress(percent(out_us, info.duration));
    })?;
    // ffmpeg's last report stops short of the end (93 % for a 3 second clip),
    // and it has finished: say so.
    on_progress(percent(0, info.duration).map(|_| 100));
    let encoded_bytes = fs::metadata(encoded.path())?.len();

    let (file, bytes, extension, kept_original) = if encoded_bytes < info.bytes {
        (encoded, encoded_bytes, OUTPUT_EXTENSION.to_string(), false)
    } else {
        drop(encoded);
        let copy = temp_file_in(&folder)?;
        let bytes = fs::copy(&info.path, copy.path())?;
        (copy, bytes, original_extension(&info.path), true)
    };

    let path = persist_first_free(file, &folder, stem, &extension, MAX_SUFFIX)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(SaveOutcome {
        path,
        name,
        bytes,
        kept_original,
    })
}

/// A path that cannot be saved next to is a problem with the file, not ffmpeg.
fn no_name(reason: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidInput, reason)
}

/// A new empty temporary file in `folder`. A leading dot hides it while it is
/// there.
fn temp_file_in(folder: &Path) -> io::Result<NamedTempFile> {
    Builder::new()
        .prefix(".optimized-")
        .suffix(".part")
        .tempfile_in(folder)
}

/// The extension of `path` in lower case, `mp4` when it has none.
fn original_extension(path: &Path) -> String {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .filter(|extension| !extension.is_empty())
        .unwrap_or_else(|| OUTPUT_EXTENSION.to_string())
}

/// Move `file` to the first free `candidate_name(stem, extension, n)` in
/// `folder`, for `n` from 0 to `max_suffix`, and return where it went.
///
/// A taken name (`AlreadyExists`) moves on to the next number; the move never
/// replaces a file. Any other error stops. When every name is taken the result
/// is [`SaveError::NoFreeName`]. On every error `file` is dropped, which
/// deletes it.
fn persist_first_free(
    mut file: NamedTempFile,
    folder: &Path,
    stem: &OsStr,
    extension: &str,
    max_suffix: u32,
) -> Result<PathBuf, SaveError> {
    for n in 0..=max_suffix {
        let target = folder.join(candidate_name(stem, extension, n));
        match file.persist_noclobber(&target) {
            Ok(_) => return Ok(target),
            Err(failed) if failed.error.kind() == ErrorKind::AlreadyExists => file = failed.file,
            Err(failed) => return Err(SaveError::Io(failed.error)),
        }
    }
    Err(SaveError::NoFreeName)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn temp_in(dir: &Path, content: &[u8]) -> NamedTempFile {
        let file = Builder::new()
            .prefix(".optimized-")
            .suffix(".part")
            .tempfile_in(dir)
            .unwrap();
        fs::write(file.path(), content).unwrap();
        file
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Make `optimized-a.mp4` and `optimized-a-<n>.mp4` for each `n`.
    fn take(dir: &Path, numbers: &[u32]) {
        for n in numbers {
            fs::write(
                dir.join(candidate_name(OsStr::new("a"), "mp4", *n)),
                format!("taken {n}"),
            )
            .unwrap();
        }
    }

    #[test]
    fn every_name_up_to_the_limit_taken_is_no_free_name() {
        let dir = TempDir::new().unwrap();
        take(dir.path(), &[0, 1, 2]);
        let file = temp_in(dir.path(), b"new");
        let result = persist_first_free(file, dir.path(), OsStr::new("a"), "mp4", 2);
        assert!(matches!(result, Err(SaveError::NoFreeName)), "{result:?}");
        // The new file is gone; the taken ones are untouched.
        assert_eq!(
            listing(dir.path()),
            ["optimized-a-1.mp4", "optimized-a-2.mp4", "optimized-a.mp4"]
        );
        assert_eq!(
            fs::read(dir.path().join("optimized-a-2.mp4")).unwrap(),
            b"taken 2"
        );
    }

    #[test]
    fn the_limit_itself_is_a_free_name() {
        // The negative of the test above: one name fewer taken.
        let dir = TempDir::new().unwrap();
        take(dir.path(), &[0, 1]);
        let file = temp_in(dir.path(), b"new");
        let path = persist_first_free(file, dir.path(), OsStr::new("a"), "mp4", 2).unwrap();
        assert_eq!(path, dir.path().join("optimized-a-2.mp4"));
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(
            fs::read(dir.path().join("optimized-a-1.mp4")).unwrap(),
            b"taken 1"
        );
    }

    #[test]
    fn a_move_error_that_is_not_a_taken_name_stops_and_leaves_nothing() {
        let dir = TempDir::new().unwrap();
        let file = temp_in(dir.path(), b"new");
        let missing = dir.path().join("no-such-folder");
        let result = persist_first_free(file, &missing, OsStr::new("a"), "mp4", 2);
        match result {
            Err(SaveError::Io(error)) => {
                assert_eq!(error.kind(), std::io::ErrorKind::NotFound)
            }
            other => panic!("expected Io, got {other:?}"),
        }
        assert!(listing(dir.path()).is_empty());
    }

    #[test]
    fn the_largest_suffix_is_9999() {
        assert_eq!(MAX_SUFFIX, 9999);
    }
}
