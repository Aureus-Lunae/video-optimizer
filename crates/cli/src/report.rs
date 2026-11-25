//! What the command line prints: the result line on stdout and the progress
//! on stderr.

use std::io::Write;

/// The one line printed when a save finished.
///
/// With `kept_original` the original's bytes were copied, so there are no
/// sizes to compare. Otherwise it is `<name>: <in> MB -> <out> MB (<n>%
/// smaller)`: decimal megabytes (1 MB is 1,000,000 bytes) with one decimal,
/// and the saving rounded to a whole percent. `input_bytes` is not 0 then: a
/// result is only kept when it is smaller than the original.
pub fn output_line(name: &str, input_bytes: u64, output_bytes: u64, kept_original: bool) -> String {
    if kept_original {
        return format!("{name}: original kept: result was not smaller");
    }
    let smaller = ((1.0 - output_bytes as f64 / input_bytes as f64) * 100.0).round();
    format!(
        "{name}: {} MB -> {} MB ({smaller}% smaller)",
        megabytes(input_bytes),
        megabytes(output_bytes)
    )
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_000_000.0)
}

/// The progress shown while ffmpeg runs: `\r42%` on the same line of stderr,
/// and only when stderr is a terminal (`enabled`), so a pipe or a log file
/// holds none.
pub struct Progress<W: Write> {
    out: W,
    enabled: bool,
    shown: bool,
}

impl<W: Write> Progress<W> {
    pub fn new(out: W, enabled: bool) -> Progress<W> {
        Progress {
            out,
            enabled,
            shown: false,
        }
    }

    /// Show `percent`; nothing when it is unknown. A progress that cannot be
    /// written (stderr closed) is dropped: it never stops the encode.
    pub fn report(&mut self, percent: Option<u8>) {
        if let (true, Some(percent)) = (self.enabled, percent) {
            let _ = write!(self.out, "\r{percent}%");
            let _ = self.out.flush();
            self.shown = true;
        }
    }

    /// End the progress line, if one was shown, so the next text starts on a
    /// line of its own.
    pub fn finish(&mut self) {
        if self.shown {
            let _ = writeln!(self.out);
            self.shown = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_line_has_decimal_megabytes_and_a_rounded_percentage() {
        assert_eq!(
            output_line("optimized-a.mp4", 68_300_000, 5_840_000, false),
            "optimized-a.mp4: 68.3 MB -> 5.8 MB (91% smaller)"
        );
    }

    #[test]
    fn the_percentage_rounds_to_the_nearest_whole_number() {
        // 495 of 1000 is a saving of 50.5 %, which rounds up; 504 and 506 are 49.6 % and 49.4 %.
        assert!(output_line("o.mp4", 1000, 495, false).ends_with("(51% smaller)"));
        assert!(output_line("o.mp4", 1000, 504, false).ends_with("(50% smaller)"));
        assert!(output_line("o.mp4", 1000, 506, false).ends_with("(49% smaller)"));
        // A result a hair smaller is 0 % smaller, not a negative or a crash.
        assert!(output_line("o.mp4", 1000, 999, false).ends_with("(0% smaller)"));
    }

    #[test]
    fn a_kept_original_says_so_and_shows_no_sizes() {
        assert_eq!(
            output_line("optimized-a.mov", 1000, 1000, true),
            "optimized-a.mov: original kept: result was not smaller"
        );
    }

    #[test]
    fn progress_writes_a_carriage_return_and_the_percent_on_a_terminal() {
        let mut out = Vec::new();
        let mut progress = Progress::new(&mut out, true);
        progress.report(Some(42));
        progress.report(Some(100));
        progress.finish();
        assert_eq!(String::from_utf8(out).unwrap(), "\r42%\r100%\n");
    }

    #[test]
    fn progress_writes_nothing_when_stderr_is_not_a_terminal() {
        let mut out = Vec::new();
        let mut progress = Progress::new(&mut out, false);
        progress.report(Some(42));
        progress.finish();
        assert!(out.is_empty());
    }

    #[test]
    fn progress_with_an_unknown_percent_writes_nothing_and_finish_adds_no_line() {
        let mut out = Vec::new();
        let mut progress = Progress::new(&mut out, true);
        progress.report(None);
        progress.finish();
        assert!(out.is_empty());
    }
}
