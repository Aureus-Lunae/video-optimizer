//! The `media` scheme: the current video's before clip and its newest sample,
//! served to the window's `<video>` elements with HTTP Range support.
//!
//! The window builds `media://localhost/3-before` (`http://media.localhost/…`
//! on Windows) with `convertFileSrc`. The key is the URI path exactly as it
//! arrived; it names a video and a clip, never a path, so the window cannot
//! reach any other file. Everything that is not the current video's before
//! clip or its newest sample is an empty `404`.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use tauri::http::header::{HeaderName, HeaderValue, RANGE};
use tauri::http::{Request, Response, StatusCode};

use crate::session::Shared;

/// The most bytes one `206` answer holds. A media element asks again for the
/// rest, so `bytes=0-` on a large file costs one chunk, not the whole file.
pub const MAX_CHUNK: u64 = 4 * 1024 * 1024;

/// What a request names: a video's before clip, or one request's sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKey {
    Before { video: u64 },
    Sample { video: u64, request: u64 },
}

/// The key named by `uri_path`, exactly `^/(\d{1,10})-before$` or
/// `^/(\d{1,10})-sample-(\d{1,10})$` with ASCII digits only. The path is taken
/// as received: a percent-encoded path (`/%33-before`) is not a key.
pub fn parse_key(uri_path: &str) -> Option<MediaKey> {
    let rest = uri_path.strip_prefix('/')?;
    if let Some(video) = rest.strip_suffix("-before") {
        return Some(MediaKey::Before {
            video: key_number(video)?,
        });
    }
    let (video, request) = rest.split_once("-sample-")?;
    Some(MediaKey::Sample {
        video: key_number(video)?,
        request: key_number(request)?,
    })
}

/// The longest number in a key.
const MAX_KEY_DIGITS: usize = 10;

/// One to [`MAX_KEY_DIGITS`] ASCII digits as a number.
fn key_number(text: &str) -> Option<u64> {
    if text.len() <= MAX_KEY_DIGITS {
        number(text)
    } else {
        None
    }
}

/// ASCII digits only (no sign, no space) as a number; `None` when empty, not
/// digits, or too large.
fn number(text: &str) -> Option<u64> {
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        text.parse().ok()
    } else {
        None
    }
}

/// How a `Range` header is answered for a file of a given length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeAnswer {
    /// The whole file: no header, or one this does not understand.
    Full,
    /// Bytes `start` to `end`, both included, at most [`MAX_CHUNK`] of them.
    Partial { start: u64, end: u64 },
    /// The range starts at or after the end of the file.
    Unsatisfiable,
}

/// Answer `header` for a file of `len` bytes. One range is understood
/// (`bytes=a-b`, `bytes=a-`, `bytes=-n`); several ranges or a malformed header
/// answer [`RangeAnswer::Full`].
pub fn parse_range(header: Option<&str>, len: u64) -> RangeAnswer {
    let Some(spec) = header.and_then(|header| header.strip_prefix("bytes=")) else {
        return RangeAnswer::Full;
    };
    if spec.contains(',') {
        return RangeAnswer::Full;
    }
    let Some((first, last)) = spec.split_once('-') else {
        return RangeAnswer::Full;
    };
    match (first.is_empty(), last.is_empty()) {
        (true, true) => RangeAnswer::Full,
        // The last `n` bytes.
        (true, false) => match number(last) {
            None => RangeAnswer::Full,
            Some(0) => RangeAnswer::Unsatisfiable,
            Some(_) if len == 0 => RangeAnswer::Unsatisfiable,
            Some(n) => capped(len.saturating_sub(n), len - 1),
        },
        // From `start` to the end of the file.
        (false, true) => match number(first) {
            None => RangeAnswer::Full,
            Some(start) if start >= len => RangeAnswer::Unsatisfiable,
            Some(start) => capped(start, len - 1),
        },
        (false, false) => match (number(first), number(last)) {
            (Some(start), Some(end)) if end >= start => {
                if start >= len {
                    RangeAnswer::Unsatisfiable
                } else {
                    capped(start, end.min(len - 1))
                }
            }
            _ => RangeAnswer::Full,
        },
    }
}

/// Bytes `start` to `end` (the last byte wanted, already inside the file),
/// cut to [`MAX_CHUNK`] bytes.
fn capped(start: u64, end: u64) -> RangeAnswer {
    RangeAnswer::Partial {
        start,
        end: end.min(start.saturating_add(MAX_CHUNK - 1)),
    }
}

/// Where the current video's files are. [`Shared`] is the real one; tests may
/// give another.
pub trait Files {
    /// The before clip of `video`, while it is the current one and the clip
    /// exists.
    fn before_path(&self, video: u64) -> Option<PathBuf>;
    /// The sample of `request` of `video`, while it is the newest finished one
    /// of the current video.
    fn sample_path(&self, video: u64, request: u64) -> Option<PathBuf>;
}

impl Files for Shared {
    fn before_path(&self, video: u64) -> Option<PathBuf> {
        Shared::before_path(self, video)
    }

    fn sample_path(&self, video: u64, request: u64) -> Option<PathBuf> {
        Shared::sample_path(self, video, request)
    }
}

/// An answer, before it becomes the webview's response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

#[cfg(test)]
impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Every answer carries this: the bytes are never sniffed into another type.
const NOSNIFF: (&str, &str) = ("X-Content-Type-Options", "nosniff");

impl Answer {
    /// This answer as the webview's response. A header the HTTP types refuse
    /// (none of ours) gives an empty `404` instead.
    pub fn into_http(self) -> Response<Vec<u8>> {
        let mut builder = Response::builder().status(self.status);
        for (name, value) in self.headers {
            builder = builder.header(name, value);
        }
        builder.body(self.body).unwrap_or_else(|_| {
            let mut response = Response::new(Vec::new());
            *response.status_mut() = StatusCode::NOT_FOUND;
            response.headers_mut().insert(
                HeaderName::from_static("x-content-type-options"),
                HeaderValue::from_static("nosniff"),
            );
            response
        })
    }
}

/// The empty `404` that every refusal is.
pub fn not_found() -> Answer {
    Answer {
        status: 404,
        headers: vec![(NOSNIFF.0, NOSNIFF.1.to_owned())],
        body: Vec::new(),
    }
}

/// The headers of an answer that carries video bytes.
fn clip_headers(body_len: usize) -> Vec<(&'static str, String)> {
    vec![
        ("Content-Type", "video/mp4".to_owned()),
        ("Accept-Ranges", "bytes".to_owned()),
        ("Content-Length", body_len.to_string()),
        (NOSNIFF.0, NOSNIFF.1.to_owned()),
    ]
}

/// Read bytes `start..=end` of `source`, nothing else. A source that ends
/// early is an error, so a `Content-Range` is never sent for bytes that were
/// not there.
fn read_span<R: Read + Seek>(source: &mut R, start: u64, end: u64) -> io::Result<Vec<u8>> {
    let wanted = end - start + 1;
    source.seek(SeekFrom::Start(start))?;
    let mut body = Vec::new();
    source.take(wanted).read_to_end(&mut body)?;
    if body.len() as u64 == wanted {
        Ok(body)
    } else {
        Err(io::Error::from(io::ErrorKind::UnexpectedEof))
    }
}

/// The file at `path` answered for `range`. An error is a file that is gone,
/// cannot be opened, or cannot be read to the end of what was asked.
fn file_answer(path: &Path, range: Option<&str>) -> io::Result<Answer> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    Ok(match parse_range(range, len) {
        RangeAnswer::Full => {
            let mut body = Vec::new();
            file.read_to_end(&mut body)?;
            Answer {
                status: 200,
                headers: clip_headers(body.len()),
                body,
            }
        }
        RangeAnswer::Partial { start, end } => {
            let body = read_span(&mut file, start, end)?;
            let mut headers = clip_headers(body.len());
            headers.push(("Content-Range", format!("bytes {start}-{end}/{len}")));
            Answer {
                status: 206,
                headers,
                body,
            }
        }
        RangeAnswer::Unsatisfiable => Answer {
            status: 416,
            headers: vec![
                ("Content-Range", format!("bytes */{len}")),
                (NOSNIFF.0, NOSNIFF.1.to_owned()),
            ],
            body: Vec::new(),
        },
    })
}

/// What `uri_path` and the `Range` header ask for, answered from `files`.
/// Anything that is not a key, that `files` does not have, or that cannot be
/// read is an empty `404`.
pub fn answer(files: &dyn Files, uri_path: &str, range: Option<&str>) -> Answer {
    let path = match parse_key(uri_path) {
        Some(MediaKey::Before { video }) => files.before_path(video),
        Some(MediaKey::Sample { video, request }) => files.sample_path(video, request),
        None => None,
    };
    path.and_then(|path| file_answer(&path, range).ok())
        .unwrap_or_else(not_found)
}

/// The webview's request answered from `files`: the URI's path as received
/// (its query does not count) and the `Range` header. A header that is not
/// text is none.
pub fn serve(files: &dyn Files, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let range = request
        .headers()
        .get(RANGE)
        .and_then(|value| value.to_str().ok());
    answer(files, request.uri().path(), range).into_http()
}

/// Run `work(value)` on a thread that `spawn` starts. If no thread can be
/// started, `otherwise(value)` runs on this thread instead, so `value` (a
/// request's responder) is never lost. Taken from Lunaepix's
/// `on_own_thread_or_else`.
pub fn on_own_thread_or_else<T: Send + 'static>(
    value: T,
    work: impl FnOnce(T) + Send + 'static,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<()>,
    otherwise: impl FnOnce(T),
) {
    // The value waits here until one side takes it: the new thread, or this
    // one when the thread never started.
    let slot = Arc::new(Mutex::new(Some(value)));
    let on_thread = {
        let slot = Arc::clone(&slot);
        move || {
            if let Some(value) = slot.lock().unwrap_or_else(PoisonError::into_inner).take() {
                work(value);
            }
        }
    };
    if let Err(error) = spawn(Box::new(on_thread)) {
        eprintln!("video-optimizer: no thread could be started ({error})");
        if let Some(value) = slot.lock().unwrap_or_else(PoisonError::into_inner).take() {
            otherwise(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    use serde_json::json;
    use tauri::http::header::{CONTENT_RANGE, CONTENT_TYPE, RANGE};
    use tempfile::TempDir;
    use video_optimizer_core::tools::Tools;

    use super::*;
    use crate::events::Event;
    use crate::session::Emit;
    use crate::session::clips::{self, Clip};

    const MIB: u64 = 1024 * 1024;

    // ---- parse_key ----

    #[test]
    fn parses_before_and_sample_keys() {
        assert_eq!(parse_key("/3-before"), Some(MediaKey::Before { video: 3 }));
        assert_eq!(
            parse_key("/3-sample-7"),
            Some(MediaKey::Sample {
                video: 3,
                request: 7
            })
        );
        // Ten digits is the longest; the regex allows leading zeros.
        assert_eq!(
            parse_key("/9999999999-before"),
            Some(MediaKey::Before {
                video: 9_999_999_999
            })
        );
        assert_eq!(
            parse_key("/0000000007-sample-0000000001"),
            Some(MediaKey::Sample {
                video: 7,
                request: 1
            })
        );
    }

    #[test]
    fn rejects_other_paths() {
        for path in [
            "",
            "/",
            "3-before",
            "//3-before",
            "/3-before/",
            "/3-before\n",
            "/3-before?x=1",
            "/3-Before",
            "/03x-before",
            "/%33-before",
            "/3-sample-",
            "/3-sample",
            "/-sample-3",
            "/3-sample--7",
            "/../3-before",
            "/a/../3-before",
            "/3-sample-7-8",
            "/3-sample-7/",
            "/3-sample-before",
            "/3-before-before",
            "/+3-before",
            "/-3-before",
            "/3.0-before",
            "/ 3-before",
            "/before",
            // 11 digits, in each place.
            "/12345678901-before",
            "/12345678901-sample-1",
            "/1-sample-12345678901",
            // Digits that are not ASCII digits.
            "/\u{FF13}-before",
            "/\u{663}-before",
            "/3-sample-\u{FF17}",
        ] {
            assert_eq!(parse_key(path), None, "{path:?}");
        }
    }

    // ---- parse_range ----

    #[test]
    fn range_0_to_99() {
        assert_eq!(
            parse_range(Some("bytes=0-99"), 1000),
            RangeAnswer::Partial { start: 0, end: 99 }
        );
        assert_eq!(
            parse_range(Some("bytes=0-0"), 1000),
            RangeAnswer::Partial { start: 0, end: 0 }
        );
        assert_eq!(
            parse_range(Some("bytes=999-999"), 1000),
            RangeAnswer::Partial {
                start: 999,
                end: 999
            }
        );
    }

    #[test]
    fn range_from_500_on_a_1000_byte_file() {
        assert_eq!(
            parse_range(Some("bytes=500-"), 1000),
            RangeAnswer::Partial {
                start: 500,
                end: 999
            }
        );
    }

    #[test]
    fn suffix_range_last_100() {
        assert_eq!(
            parse_range(Some("bytes=-100"), 1000),
            RangeAnswer::Partial {
                start: 900,
                end: 999
            }
        );
        // A suffix longer than the file is the whole file.
        assert_eq!(
            parse_range(Some("bytes=-5000"), 1000),
            RangeAnswer::Partial { start: 0, end: 999 }
        );
    }

    #[test]
    fn a_range_past_the_end_is_unsatisfiable() {
        for (header, len) in [
            ("bytes=1000-", 1000),
            ("bytes=1000-1005", 1000),
            ("bytes=5000-", 1000),
            ("bytes=0-", 0),
            ("bytes=-5", 0),
            ("bytes=-0", 1000),
        ] {
            assert_eq!(
                parse_range(Some(header), len),
                RangeAnswer::Unsatisfiable,
                "{header} on {len}"
            );
        }
    }

    #[test]
    fn a_range_ending_past_the_end_is_cut_to_the_end() {
        assert_eq!(
            parse_range(Some("bytes=990-2000"), 1000),
            RangeAnswer::Partial {
                start: 990,
                end: 999
            }
        );
    }

    #[test]
    fn a_range_that_still_fits_is_not_unsatisfiable() {
        // The last byte is inside; one more is not.
        assert_ne!(
            parse_range(Some("bytes=999-"), 1000),
            RangeAnswer::Unsatisfiable
        );
        assert_eq!(
            parse_range(Some("bytes=1000-"), 1000),
            RangeAnswer::Unsatisfiable
        );
    }

    #[test]
    fn malformed_or_multiple_ranges_answer_full() {
        for header in [
            "bytes=a-b",
            "bytes=0-1,5-6",
            "items=0-1",
            "bytes=5-1",
            "bytes=",
            "bytes=-",
            "bytes=5",
            "bytes=-5-",
            "bytes=+5-9",
            "bytes= 5-9",
            "bytes=5 -9",
            "bytes=0x5-9",
            "bytes=99999999999999999999-",
            "bytes=0-99999999999999999999",
            "bytes",
            "",
            "0-99",
            "Bytes=0-99",
        ] {
            assert_eq!(
                parse_range(Some(header), 1000),
                RangeAnswer::Full,
                "{header:?}"
            );
        }
    }

    #[test]
    fn no_header_answers_full() {
        assert_eq!(parse_range(None, 1000), RangeAnswer::Full);
        assert_eq!(parse_range(None, 0), RangeAnswer::Full);
    }

    #[test]
    fn an_open_range_is_capped_at_4_mib() {
        assert_eq!(
            parse_range(Some("bytes=0-"), 10 * MIB),
            RangeAnswer::Partial {
                start: 0,
                end: 4_194_303
            }
        );
        // The cap counts from the start of the range.
        assert_eq!(
            parse_range(Some("bytes=1000-"), 10 * MIB),
            RangeAnswer::Partial {
                start: 1000,
                end: 1000 + MAX_CHUNK - 1
            }
        );
        // A suffix and a closed range over the cap are cut as well.
        assert_eq!(
            parse_range(Some("bytes=-5000000"), 10 * MIB),
            RangeAnswer::Partial {
                start: 10 * MIB - 5_000_000,
                end: 10 * MIB - 5_000_000 + MAX_CHUNK - 1
            }
        );
        assert_eq!(
            parse_range(Some("bytes=0-9999999"), 10 * MIB),
            RangeAnswer::Partial {
                start: 0,
                end: MAX_CHUNK - 1
            }
        );
        // A range near the end is cut by the file, not by the cap.
        assert_eq!(
            parse_range(Some("bytes=10485000-"), 10 * MIB),
            RangeAnswer::Partial {
                start: 10_485_000,
                end: 10 * MIB - 1
            }
        );
    }

    #[test]
    fn a_closed_range_under_the_cap_is_not_cut() {
        assert_eq!(
            parse_range(Some("bytes=100-4194403"), 10 * MIB),
            RangeAnswer::Partial {
                start: 100,
                end: 100 + MAX_CHUNK - 1
            }
        );
        assert_eq!(
            parse_range(Some("bytes=100-4194402"), 10 * MIB),
            RangeAnswer::Partial {
                start: 100,
                end: 4_194_402
            }
        );
        assert_eq!(
            parse_range(Some("bytes=0-1048575"), 10 * MIB),
            RangeAnswer::Partial {
                start: 0,
                end: MIB - 1
            }
        );
    }

    // ---- read_span ----

    /// A source whose read fails.
    struct Broken;

    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("the disk went away"))
        }
    }

    impl Seek for Broken {
        fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
            Ok(0)
        }
    }

    /// A source whose seek fails.
    struct Unseekable;

    impl Read for Unseekable {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    impl Seek for Unseekable {
        fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
            Err(io::Error::other("cannot seek"))
        }
    }

    fn pattern(len: u64) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn read_span_returns_exactly_the_requested_bytes() {
        let data = pattern(1000);
        let mut source = io::Cursor::new(data.clone());
        assert_eq!(read_span(&mut source, 10, 19).unwrap(), data[10..=19]);
        assert_eq!(read_span(&mut source, 0, 0).unwrap(), data[0..=0]);
        assert_eq!(read_span(&mut source, 990, 999).unwrap(), data[990..=999]);
    }

    #[test]
    fn read_span_reads_no_more_than_the_span() {
        /// Counts the bytes handed out.
        struct Counting {
            inner: io::Cursor<Vec<u8>>,
            read: u64,
        }
        impl Read for Counting {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = self.inner.read(buf)?;
                self.read += n as u64;
                Ok(n)
            }
        }
        impl Seek for Counting {
            fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
                self.inner.seek(to)
            }
        }
        let mut source = Counting {
            inner: io::Cursor::new(pattern(100_000)),
            read: 0,
        };
        assert_eq!(read_span(&mut source, 5000, 5099).unwrap().len(), 100);
        assert_eq!(source.read, 100, "only the 100 requested bytes were read");
    }

    #[test]
    fn read_span_fails_when_the_source_ends_early() {
        let mut source = io::Cursor::new(pattern(100));
        let error = read_span(&mut source, 90, 109).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        // Starting past the end gives nothing, which is also an error.
        assert!(read_span(&mut source, 200, 209).is_err());
    }

    #[test]
    fn read_span_fails_when_the_read_or_the_seek_fails() {
        assert!(read_span(&mut Broken, 0, 9).is_err());
        assert!(read_span(&mut Unseekable, 0, 9).is_err());
    }

    // ---- answer, with a double for the lookup ----

    /// A lookup that names the files it is given, and records what it was asked.
    struct Fake {
        before: Option<PathBuf>,
        sample: Option<PathBuf>,
        asked: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(before: Option<PathBuf>, sample: Option<PathBuf>) -> Fake {
            Fake {
                before,
                sample,
                asked: RefCell::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<String> {
            self.asked.borrow().clone()
        }
    }

    impl Files for Fake {
        fn before_path(&self, video: u64) -> Option<PathBuf> {
            self.asked.borrow_mut().push(format!("before {video}"));
            self.before.clone()
        }

        fn sample_path(&self, video: u64, request: u64) -> Option<PathBuf> {
            self.asked
                .borrow_mut()
                .push(format!("sample {video} {request}"));
            self.sample.clone()
        }
    }

    fn file_with(dir: &Path, name: &str, len: u64) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, pattern(len)).unwrap();
        path
    }

    fn assert_not_found(answer: &Answer) {
        assert_eq!(answer.status, 404, "{answer:?}");
        assert!(answer.body.is_empty());
        assert_eq!(answer.header("content-type"), None);
        assert_eq!(answer.header("content-range"), None);
        assert_eq!(answer.header("x-content-type-options"), Some("nosniff"));
    }

    #[test]
    fn the_whole_file_answers_200_with_accept_ranges() {
        let dir = TempDir::new().unwrap();
        let path = file_with(dir.path(), "before.mp4", 5000);
        let fake = Fake::new(Some(path), None);
        let answer = answer(&fake, "/3-before", None);
        assert_eq!(answer.status, 200);
        assert_eq!(answer.body, pattern(5000));
        assert_eq!(answer.header("accept-ranges"), Some("bytes"));
        assert_eq!(answer.header("content-type"), Some("video/mp4"));
        assert_eq!(answer.header("content-length"), Some("5000"));
        assert_eq!(answer.header("content-range"), None);
        assert_eq!(fake.asked(), ["before 3"]);
    }

    #[test]
    fn a_malformed_range_answers_the_whole_file() {
        let dir = TempDir::new().unwrap();
        let path = file_with(dir.path(), "before.mp4", 300);
        let fake = Fake::new(Some(path), None);
        for header in ["bytes=a-b", "bytes=0-1,5-6", "bytes=5-1"] {
            let answer = answer(&fake, "/3-before", Some(header));
            assert_eq!(answer.status, 200, "{header}");
            assert_eq!(answer.body.len(), 300);
        }
    }

    #[test]
    fn a_range_answers_206_with_only_those_bytes_and_a_content_range() {
        let dir = TempDir::new().unwrap();
        let path = file_with(dir.path(), "sample.mp4", 5000);
        let fake = Fake::new(None, Some(path));
        let answer = answer(&fake, "/3-sample-7", Some("bytes=100-199"));
        assert_eq!(answer.status, 206);
        assert_eq!(answer.body, pattern(5000)[100..200]);
        assert_eq!(answer.header("content-range"), Some("bytes 100-199/5000"));
        assert_eq!(answer.header("content-type"), Some("video/mp4"));
        assert_eq!(answer.header("accept-ranges"), Some("bytes"));
        assert_eq!(answer.header("content-length"), Some("100"));
        assert_eq!(fake.asked(), ["sample 3 7"]);
    }

    #[test]
    fn a_suffix_and_an_open_range_answer_the_end_of_the_file() {
        let dir = TempDir::new().unwrap();
        let path = file_with(dir.path(), "before.mp4", 1000);
        let fake = Fake::new(Some(path), None);
        let suffix = answer(&fake, "/1-before", Some("bytes=-100"));
        assert_eq!(suffix.status, 206);
        assert_eq!(suffix.header("content-range"), Some("bytes 900-999/1000"));
        assert_eq!(suffix.body, pattern(1000)[900..]);
        let open = answer(&fake, "/1-before", Some("bytes=500-"));
        assert_eq!(open.header("content-range"), Some("bytes 500-999/1000"));
        assert_eq!(open.body.len(), 500);
    }

    #[test]
    fn an_open_range_on_a_large_file_answers_one_chunk() {
        let dir = TempDir::new().unwrap();
        let path = file_with(dir.path(), "before.mp4", 5 * MIB);
        let fake = Fake::new(Some(path), None);
        let answer = answer(&fake, "/1-before", Some("bytes=0-"));
        assert_eq!(answer.status, 206);
        assert_eq!(answer.body.len() as u64, MAX_CHUNK);
        assert_eq!(
            answer.header("content-range"),
            Some("bytes 0-4194303/5242880")
        );
    }

    #[test]
    fn a_range_past_the_end_answers_416_with_the_length() {
        let dir = TempDir::new().unwrap();
        let path = file_with(dir.path(), "before.mp4", 1000);
        let fake = Fake::new(Some(path), None);
        let answer = answer(&fake, "/1-before", Some("bytes=1000-"));
        assert_eq!(answer.status, 416);
        assert_eq!(answer.header("content-range"), Some("bytes */1000"));
        assert!(answer.body.is_empty());
        assert_eq!(answer.header("content-type"), None);
    }

    #[test]
    fn a_key_that_does_not_parse_asks_the_lookup_nothing() {
        let dir = TempDir::new().unwrap();
        let path = file_with(dir.path(), "x.mp4", 10);
        let fake = Fake::new(Some(path.clone()), Some(path));
        for uri_path in [
            "/3-before/",
            "/%33-before",
            "/../3-before",
            "/3-sample-7-8",
            "/12345678901-before",
            "/etc/passwd",
            "",
        ] {
            assert_not_found(&answer(&fake, uri_path, None));
        }
        assert!(fake.asked().is_empty(), "{:?}", fake.asked());
    }

    #[test]
    fn a_key_the_lookup_does_not_know_answers_404_even_with_a_range() {
        let fake = Fake::new(None, None);
        for uri_path in ["/9-before", "/9-sample-1"] {
            for range in [None, Some("bytes=0-9"), Some("bytes=999999-")] {
                assert_not_found(&answer(&fake, uri_path, range));
            }
        }
    }

    #[test]
    fn a_file_that_is_gone_answers_404_not_a_panic() {
        let dir = TempDir::new().unwrap();
        let path = file_with(dir.path(), "sample.mp4", 100);
        fs::remove_file(&path).unwrap();
        let fake = Fake::new(Some(path.clone()), Some(path));
        for uri_path in ["/1-before", "/1-sample-1"] {
            for range in [None, Some("bytes=0-9"), Some("bytes=-5")] {
                assert_not_found(&answer(&fake, uri_path, range));
            }
        }
    }

    #[test]
    fn a_file_that_cannot_be_read_answers_404() {
        // A folder opens on Unix and fails on read; on Windows it fails to
        // open. Either way nothing of it is served.
        let dir = TempDir::new().unwrap();
        let fake = Fake::new(Some(dir.path().to_path_buf()), None);
        for range in [None, Some("bytes=0-9")] {
            assert_not_found(&answer(&fake, "/1-before", range));
        }
    }

    #[test]
    fn an_empty_file_answers_200_empty_and_416_for_any_range() {
        let dir = TempDir::new().unwrap();
        let path = file_with(dir.path(), "empty.mp4", 0);
        let fake = Fake::new(Some(path), None);
        let whole = answer(&fake, "/1-before", None);
        assert_eq!((whole.status, whole.body.len()), (200, 0));
        let ranged = answer(&fake, "/1-before", Some("bytes=0-"));
        assert_eq!(ranged.status, 416);
        assert_eq!(ranged.header("content-range"), Some("bytes */0"));
    }

    // ---- into_http and serve ----

    #[test]
    fn an_answer_becomes_a_response_with_its_status_headers_and_body() {
        let response = Answer {
            status: 206,
            headers: vec![("Content-Range", "bytes 0-2/10".to_owned())],
            body: vec![1, 2, 3],
        }
        .into_http();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[CONTENT_RANGE], "bytes 0-2/10");
        assert_eq!(response.body(), &[1, 2, 3]);
    }

    #[test]
    fn an_answer_with_a_header_the_http_types_refuse_is_an_empty_404() {
        let response = Answer {
            status: 200,
            headers: vec![("Content-Type", "video/mp4\n".to_owned())],
            body: vec![1, 2, 3],
        }
        .into_http();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(response.body().is_empty());
        assert!(response.headers().get(CONTENT_TYPE).is_none());
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    }

    fn request(uri: &str, range: Option<&[u8]>) -> Request<Vec<u8>> {
        let mut builder = Request::builder().uri(uri);
        if let Some(range) = range {
            builder = builder.header(RANGE, range);
        }
        builder.body(Vec::new()).unwrap()
    }

    #[test]
    fn serve_reads_the_path_and_the_range_header_of_the_request() {
        let dir = TempDir::new().unwrap();
        let path = file_with(dir.path(), "before.mp4", 1000);
        let fake = Fake::new(Some(path), None);
        for uri in [
            "media://localhost/3-before",
            "http://media.localhost/3-before",
        ] {
            let response = serve(&fake, &request(uri, Some(b"bytes=10-19")));
            assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT, "{uri}");
            assert_eq!(response.body(), &pattern(1000)[10..20]);
            assert_eq!(response.headers()[CONTENT_RANGE], "bytes 10-19/1000");
        }
        // No header: the whole file. A header that is not text: the whole file.
        let whole = serve(&fake, &request("media://localhost/3-before", None));
        assert_eq!(whole.status(), StatusCode::OK);
        let odd = serve(
            &fake,
            &request("media://localhost/3-before", Some(&[0xff, 0xfe])),
        );
        assert_eq!(odd.status(), StatusCode::OK);
        assert_eq!(odd.body().len(), 1000);
    }

    #[test]
    fn serve_does_not_decode_the_path_and_ignores_the_query() {
        let dir = TempDir::new().unwrap();
        let path = file_with(dir.path(), "before.mp4", 100);
        let fake = Fake::new(Some(path), None);
        let encoded = serve(&fake, &request("media://localhost/%33-before", None));
        assert_eq!(encoded.status(), StatusCode::NOT_FOUND);
        assert!(fake.asked().is_empty());
        // The key is the path; a query is not part of it.
        let queried = serve(&fake, &request("media://localhost/3-before?t=1", None));
        assert_eq!(queried.status(), StatusCode::OK);
        assert_eq!(fake.asked(), ["before 3"]);
    }

    // ---- on_own_thread_or_else ----

    #[test]
    fn the_work_runs_on_the_thread_that_started_and_not_the_fallback() {
        let (sender, receiver) = mpsc::channel();
        let fell_back = AtomicBool::new(false);
        let here = std::thread::current().id();
        on_own_thread_or_else(
            sender,
            move |sender| sender.send(std::thread::current().id()).unwrap(),
            |job| std::thread::Builder::new().spawn(job).map(drop),
            |_| fell_back.store(true, Ordering::SeqCst),
        );
        let ran_on = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_ne!(ran_on, here);
        assert!(!fell_back.load(Ordering::SeqCst));
    }

    #[test]
    fn when_no_thread_starts_the_fallback_gets_the_value_and_the_work_does_not_run() {
        let (sender, receiver) = mpsc::channel::<&str>();
        let ran = Arc::new(AtomicBool::new(false));
        let ran_work = Arc::clone(&ran);
        let mut got = None;
        on_own_thread_or_else(
            sender,
            move |_| ran_work.store(true, Ordering::SeqCst),
            |_| Err(io::Error::other("no threads")),
            |sender| {
                sender.send("fallback").unwrap();
                got = Some(());
            },
        );
        assert_eq!(got, Some(()));
        assert_eq!(receiver.try_recv(), Ok("fallback"));
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[test]
    fn a_thread_that_starts_late_does_not_run_the_work_a_second_time() {
        // The spawn "fails" but the job still runs afterwards (as if the
        // thread had started): only one side may take the value.
        let (sender, receiver) = mpsc::channel::<&str>();
        let mut late: Option<Box<dyn FnOnce() + Send>> = None;
        on_own_thread_or_else(
            sender.clone(),
            move |sender| sender.send("work").unwrap(),
            |job| {
                late = Some(job);
                Err(io::Error::other("reported as failed"))
            },
            |sender| sender.send("fallback").unwrap(),
        );
        late.unwrap()();
        assert_eq!(receiver.try_recv(), Ok("fallback"));
        assert!(receiver.try_recv().is_err(), "the work did not also run");
        drop(sender);
    }

    // ---- the real session ----

    /// A real session on the real tools, with its events collected.
    struct Live {
        shared: Arc<Shared>,
        events: Receiver<Event>,
        dir: TempDir,
    }

    impl Live {
        fn new() -> Live {
            let tools = Tools::locate().expect(
                "ffmpeg and ffprobe are needed for these tests: put them on the PATH or set \
                 VIDEO_OPTIMIZER_TOOLS_DIR",
            );
            let dir = TempDir::new().unwrap();
            let work_root = dir.path().join("work");
            fs::create_dir(&work_root).unwrap();
            let (sender, events) = mpsc::channel();
            let emit: Emit = Arc::new(move |event| {
                let _ = sender.send(event);
            });
            Live {
                shared: Shared::new(tools, work_root, emit),
                events,
                dir,
            }
        }

        fn wait(&self, what: &str, matches: impl Fn(&Event) -> bool) -> Event {
            loop {
                let event = self
                    .events
                    .recv_timeout(Duration::from_secs(120))
                    .unwrap_or_else(|_| panic!("no {what} came"));
                if matches(&event) {
                    return event;
                }
            }
        }

        /// Load a generated clip and wait for its before clip; the video id.
        fn load(&self, name: &str) -> u64 {
            let path = clips::make(
                self.dir.path(),
                Clip {
                    name: name.into(),
                    ..Clip::default()
                },
            );
            self.shared.load(&[path]);
            let Event::Video(video) = self.wait("video", |e| matches!(e, Event::Video(_))) else {
                unreachable!()
            };
            self.wait(
                "before-ready",
                |e| matches!(e, Event::BeforeReady(b) if b.video == video.video),
            );
            video.video
        }

        /// Ask for sample `request` of `video` and wait until it exists.
        fn sample(&self, video: u64, request: u64) {
            self.shared
                .make_sample(video, request, &json!(28), &json!("keep"))
                .unwrap();
            self.wait(
                "sample",
                |e| matches!(e, Event::Sample(s) if s.video == video && s.request == request),
            );
        }
    }

    fn get(shared: &Shared, uri_path: &str, range: Option<&str>) -> Answer {
        answer(shared, uri_path, range)
    }

    #[test]
    fn serves_the_real_before_clip_and_sample_from_the_session() {
        let live = Live::new();
        let id = live.load("a.mp4");
        live.sample(id, 1);
        let before = fs::read(live.shared.before_path(id).unwrap()).unwrap();
        let sample = fs::read(live.shared.sample_path(id, 1).unwrap()).unwrap();
        assert!(before.len() > 100 && sample.len() > 100);

        let whole = get(&live.shared, &format!("/{id}-before"), None);
        assert_eq!(whole.status, 200);
        assert_eq!(whole.body, before);

        let head = get(&live.shared, &format!("/{id}-sample-1"), Some("bytes=0-99"));
        assert_eq!(head.status, 206);
        assert_eq!(head.body, sample[..100]);
        assert_eq!(
            head.header("content-range"),
            Some(format!("bytes 0-99/{}", sample.len()).as_str())
        );

        let past = get(
            &live.shared,
            &format!("/{id}-sample-1"),
            Some(&format!("bytes={}-", sample.len())),
        );
        assert_eq!(past.status, 416);
    }

    #[test]
    fn serves_only_the_current_videos_files() {
        let live = Live::new();
        let first = live.load("a.mp4");
        live.sample(first, 1);
        let first_before = format!("/{first}-before");
        let first_sample = format!("/{first}-sample-1");
        assert_eq!(get(&live.shared, &first_before, None).status, 200);
        assert_eq!(
            get(&live.shared, &first_sample, Some("bytes=0-9")).status,
            206
        );

        // The next video replaces the first: its files are no longer served.
        let second = live.load("b.mp4");
        assert_ne!(first, second);
        assert_not_found(&get(&live.shared, &first_before, None));
        assert_not_found(&get(&live.shared, &first_sample, Some("bytes=0-9")));
        assert_eq!(
            get(
                &live.shared,
                &format!("/{second}-before"),
                Some("bytes=0-9")
            )
            .status,
            206
        );

        // Within the video, a newer sample replaces the older one.
        live.sample(second, 1);
        live.sample(second, 2);
        assert_not_found(&get(
            &live.shared,
            &format!("/{second}-sample-1"),
            Some("bytes=0-9"),
        ));
        assert_eq!(
            get(
                &live.shared,
                &format!("/{second}-sample-2"),
                Some("bytes=0-9")
            )
            .status,
            206
        );
        // A request that was never made, and another video's number.
        assert_not_found(&get(&live.shared, &format!("/{second}-sample-3"), None));
        assert_not_found(&get(&live.shared, &format!("/{}-before", second + 1), None));
    }

    #[test]
    fn a_sample_file_removed_after_the_lookup_answers_404_through_the_session() {
        let live = Live::new();
        let id = live.load("a.mp4");
        live.sample(id, 1);
        let path = live.shared.sample_path(id, 1).unwrap();
        fs::remove_file(&path).unwrap();
        // The session still names the file; reading it is what fails.
        assert_eq!(live.shared.sample_path(id, 1), Some(path));
        assert_not_found(&get(
            &live.shared,
            &format!("/{id}-sample-1"),
            Some("bytes=0-9"),
        ));
    }

    #[test]
    fn nothing_is_served_before_a_video_is_loaded() {
        let live = Live::new();
        assert_not_found(&get(&live.shared, "/1-before", None));
        assert_not_found(&get(&live.shared, "/1-sample-1", None));
    }
}
