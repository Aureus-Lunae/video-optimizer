//! The command line: which video, at what quality, and where it goes.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use video_optimizer_core::settings::{Audio, Crf, Settings, SettingsError};

/// What the user asked for.
#[derive(Debug, PartialEq, Eq)]
pub struct Args {
    /// The one video.
    pub file: PathBuf,
    /// CRF 28 and keep the audio unless `--crf` or `--no-audio` said otherwise.
    pub settings: Settings,
    /// `--out <folder>`: an existing folder for the result, instead of the
    /// original's own.
    pub out: Option<PathBuf>,
}

/// Why the arguments do not make sense. The caller prints the message and
/// [`USAGE`], and exits with 2.
#[derive(Debug, PartialEq, Eq)]
pub enum UsageError {
    NoFile,
    TwoFiles,
    /// An argument that starts with `-` and is no option.
    UnknownOption(OsString),
    /// `--crf` or `--out` was the last argument.
    MissingValue(&'static str),
    /// An option given twice, also with the same value.
    Repeated(&'static str),
    /// The value of `--crf`, refused by the same check the window uses.
    Crf(SettingsError),
    /// The value of `--out` is not an existing folder.
    OutNotFolder(PathBuf),
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UsageError::NoFile => write!(f, "no video was given"),
            UsageError::TwoFiles => write!(f, "One video at a time"),
            UsageError::UnknownOption(option) => {
                write!(f, "unknown option {}", option.to_string_lossy())
            }
            UsageError::MissingValue(option) => write!(f, "{option} needs a value"),
            UsageError::Repeated(option) => write!(f, "{option} was given twice"),
            UsageError::Crf(error) => write!(f, "{error}"),
            UsageError::OutNotFolder(path) => {
                write!(f, "{} is not an existing folder", path.display())
            }
        }
    }
}

/// The usage text, printed to stderr after the reason of a usage error.
pub const USAGE: &str =
    "usage: video-optimizer [--crf <20-36>] [--no-audio] [--out <folder>] [--] <file>";

/// Read the arguments after the program name.
///
/// `--crf` and `--out` take the next argument as their value, whatever it
/// looks like; the CRF goes through [`Crf::parse`], the one check every entry
/// point ends in. `--out` must name an existing folder. `--` ends option
/// parsing, so a file named `-x.mp4` can be given after it. Arguments stay
/// `OsString`: nothing is turned into text except the CRF, which a non-UTF-8
/// value cannot be.
///
/// A usage error is: no file or a second one, an unknown option (also the
/// `--crf=30` spelling, and a file name that starts with `-` and is not after
/// `--`), an option without its value or given twice, a CRF that is not a whole
/// number from 20 to 36, or an `--out` that is no folder.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Args, UsageError> {
    let mut args = args.into_iter();
    let mut file: Option<PathBuf> = None;
    let mut crf: Option<Crf> = None;
    let mut remove_audio = false;
    let mut out: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        if arg == "--" {
            for name in args.by_ref() {
                add_file(&mut file, name)?;
            }
        } else if arg == "--crf" {
            let value = args.next().ok_or(UsageError::MissingValue("--crf"))?;
            // A value that is not text cannot be a number: the same error.
            let value =
                Crf::parse(value.to_str().unwrap_or("\u{fffd}")).map_err(UsageError::Crf)?;
            if crf.replace(value).is_some() {
                return Err(UsageError::Repeated("--crf"));
            }
        } else if arg == "--no-audio" {
            if std::mem::replace(&mut remove_audio, true) {
                return Err(UsageError::Repeated("--no-audio"));
            }
        } else if arg == "--out" {
            let value = PathBuf::from(args.next().ok_or(UsageError::MissingValue("--out"))?);
            if !value.is_dir() {
                return Err(UsageError::OutNotFolder(value));
            }
            if out.replace(value).is_some() {
                return Err(UsageError::Repeated("--out"));
            }
        } else if arg != "-" && arg.as_encoded_bytes().starts_with(b"-") {
            return Err(UsageError::UnknownOption(arg));
        } else {
            add_file(&mut file, arg)?;
        }
    }
    Ok(Args {
        file: file.ok_or(UsageError::NoFile)?,
        settings: Settings {
            crf: crf.unwrap_or_default(),
            audio: if remove_audio {
                Audio::Remove
            } else {
                Audio::Keep
            },
        },
        out,
    })
}

/// Takes `name` as the file, unless there already is one.
fn add_file(file: &mut Option<PathBuf>, name: OsString) -> Result<(), UsageError> {
    match file.replace(PathBuf::from(name)) {
        None => Ok(()),
        Some(_) => Err(UsageError::TwoFiles),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use tempfile::TempDir;
    use video_optimizer_core::settings::{Audio, Crf, SettingsError};

    fn parse_args(args: &[&str]) -> Result<Args, UsageError> {
        parse(args.iter().map(OsString::from))
    }

    fn crf_of(args: &[&str]) -> u8 {
        parse_args(args).unwrap().settings.crf.get()
    }

    #[test]
    fn accepts_crf_20_28_36() {
        for crf in [20, 28, 36] {
            let text = crf.to_string();
            assert_eq!(crf_of(&["--crf", &text, "a.mp4"]), crf);
        }
        // The value is taken as the CRF, wherever the option stands.
        assert_eq!(crf_of(&["a.mp4", "--crf", "33"]), 33);
    }

    #[test]
    fn rejects_crf_19_37_28_5_minus_1_and_text() {
        let out_of_range = |value| Err(UsageError::Crf(SettingsError::CrfOutOfRange(value)));
        assert_eq!(parse_args(&["--crf", "19", "a.mp4"]), out_of_range(19));
        assert_eq!(parse_args(&["--crf", "37", "a.mp4"]), out_of_range(37));
        assert_eq!(parse_args(&["--crf", "0", "a.mp4"]), out_of_range(0));
        assert_eq!(parse_args(&["--crf", "51", "a.mp4"]), out_of_range(51));
        for text in ["28.5", "-1", "abc", "", " 28", "+28", "2 8", "0x1c"] {
            assert_eq!(
                parse_args(&["--crf", text, "a.mp4"]),
                Err(UsageError::Crf(SettingsError::CrfNotWholeNumber)),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_crf_value_that_looks_like_an_option_is_still_the_value() {
        assert_eq!(
            parse_args(&["--crf", "--no-audio", "a.mp4"]),
            Err(UsageError::Crf(SettingsError::CrfNotWholeNumber))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_crf_value_that_is_not_valid_utf8_is_refused() {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(vec![b'2', 0xff]);
        let args = ["--crf".into(), bad, "a.mp4".into()];
        assert_eq!(
            parse(args),
            Err(UsageError::Crf(SettingsError::CrfNotWholeNumber))
        );
        // The valid spelling parses, so the byte decides.
        assert_eq!(crf_of(&["--crf", "22", "a.mp4"]), 22);
    }

    #[test]
    fn defaults_to_crf_28_and_keep() {
        let args = parse_args(&["a.mp4"]).unwrap();
        assert_eq!(args.file, PathBuf::from("a.mp4"));
        assert_eq!(args.settings.crf, Crf::new(28).unwrap());
        assert_eq!(args.settings.audio, Audio::Keep);
        assert_eq!(args.out, None);
    }

    #[test]
    fn no_audio_sets_remove() {
        let args = parse_args(&["--no-audio", "a.mp4"]).unwrap();
        assert_eq!(args.settings.audio, Audio::Remove);
        // Where it stands does not matter, and it leaves the CRF alone.
        let args = parse_args(&["a.mp4", "--crf", "30", "--no-audio"]).unwrap();
        assert_eq!(args.settings.audio, Audio::Remove);
        assert_eq!(args.settings.crf.get(), 30);
    }

    #[test]
    fn refuses_no_file() {
        assert_eq!(parse_args(&[]), Err(UsageError::NoFile));
        assert_eq!(parse_args(&["--no-audio"]), Err(UsageError::NoFile));
        assert_eq!(parse_args(&["--crf", "30"]), Err(UsageError::NoFile));
        assert_eq!(parse_args(&["--"]), Err(UsageError::NoFile));
    }

    #[test]
    fn refuses_two_files() {
        assert_eq!(parse_args(&["a.mp4", "b.mp4"]), Err(UsageError::TwoFiles));
        assert_eq!(
            parse_args(&["a.mp4", "--no-audio", "b.mp4"]),
            Err(UsageError::TwoFiles)
        );
        // Also after `--`.
        assert_eq!(
            parse_args(&["--", "a.mp4", "b.mp4"]),
            Err(UsageError::TwoFiles)
        );
        assert_eq!(
            parse_args(&["a.mp4", "--", "b.mp4"]),
            Err(UsageError::TwoFiles)
        );
    }

    #[test]
    fn refuses_a_missing_out_folder() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nope");
        let missing_text = missing.to_str().unwrap();
        assert_eq!(
            parse_args(&["--out", missing_text, "a.mp4"]),
            Err(UsageError::OutNotFolder(missing.clone()))
        );
        // A file is no folder either.
        let file = dir.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(
            parse_args(&["--out", file.to_str().unwrap(), "a.mp4"]),
            Err(UsageError::OutNotFolder(file))
        );
        // It was not created by being named.
        assert!(!missing.exists());
    }

    #[test]
    fn accepts_an_existing_out_folder() {
        let dir = TempDir::new().unwrap();
        let args = parse_args(&["--out", dir.path().to_str().unwrap(), "a.mp4"]).unwrap();
        assert_eq!(args.out, Some(dir.path().to_path_buf()));
    }

    #[test]
    fn a_dash_name_after_double_dash_is_a_file() {
        assert_eq!(
            parse_args(&["--", "-x.mp4"]).unwrap().file,
            PathBuf::from("-x.mp4")
        );
        // Options before it still count; after it nothing is an option.
        let args = parse_args(&["--no-audio", "--", "--crf"]).unwrap();
        assert_eq!(args.file, PathBuf::from("--crf"));
        assert_eq!(args.settings.audio, Audio::Remove);
    }

    #[test]
    fn a_dash_name_without_double_dash_is_an_unknown_option() {
        assert_eq!(
            parse_args(&["-x.mp4"]),
            Err(UsageError::UnknownOption(OsString::from("-x.mp4")))
        );
        assert_eq!(
            parse_args(&["a.mp4", "--bogus"]),
            Err(UsageError::UnknownOption(OsString::from("--bogus")))
        );
        // The `=` spelling is not an option.
        assert_eq!(
            parse_args(&["--crf=30", "a.mp4"]),
            Err(UsageError::UnknownOption(OsString::from("--crf=30")))
        );
    }

    #[test]
    fn a_lone_dash_is_a_file_name() {
        assert_eq!(parse_args(&["-"]).unwrap().file, PathBuf::from("-"));
    }

    #[test]
    fn an_option_without_its_value_is_a_usage_error() {
        assert_eq!(
            parse_args(&["a.mp4", "--crf"]),
            Err(UsageError::MissingValue("--crf"))
        );
        assert_eq!(
            parse_args(&["a.mp4", "--out"]),
            Err(UsageError::MissingValue("--out"))
        );
    }

    #[test]
    fn an_option_given_twice_is_a_usage_error() {
        assert_eq!(
            parse_args(&["--crf", "30", "--crf", "30", "a.mp4"]),
            Err(UsageError::Repeated("--crf"))
        );
        assert_eq!(
            parse_args(&["--no-audio", "--no-audio", "a.mp4"]),
            Err(UsageError::Repeated("--no-audio"))
        );
        let dir = TempDir::new().unwrap();
        let out = dir.path().to_str().unwrap();
        assert_eq!(
            parse_args(&["--out", out, "--out", out, "a.mp4"]),
            Err(UsageError::Repeated("--out"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn keeps_a_file_name_that_is_not_valid_utf8() {
        use std::os::unix::ffi::OsStringExt;
        let name = OsString::from_vec(vec![b'a', 0xff, b'.', b'm', b'p', b'4']);
        assert_eq!(
            parse([name.clone()]).map(|args| args.file),
            Ok(PathBuf::from(name))
        );
    }

    #[test]
    fn every_error_has_a_message_and_the_crf_errors_name_the_range() {
        let range = UsageError::Crf(SettingsError::CrfOutOfRange(19)).to_string();
        assert!(range.contains("from 20 to 36"), "{range}");
        for error in [
            UsageError::NoFile,
            UsageError::TwoFiles,
            UsageError::MissingValue("--crf"),
            UsageError::Repeated("--out"),
            UsageError::UnknownOption(OsString::from("-x.mp4")),
            UsageError::OutNotFolder(PathBuf::from("nope")),
        ] {
            assert!(!error.to_string().is_empty(), "{error:?}");
        }
        assert!(
            UsageError::UnknownOption(OsString::from("-x.mp4"))
                .to_string()
                .contains("-x.mp4")
        );
        assert!(
            UsageError::OutNotFolder(PathBuf::from("nope"))
                .to_string()
                .contains("nope")
        );
    }
}
