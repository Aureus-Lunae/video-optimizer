//! Reading ffmpeg's `-progress pipe:1` output.

/// The key of the progress line that says how far the output has got.
const OUT_TIME_KEY: &str = "out_time_us=";

/// The microseconds of the output written so far, from a line
/// `out_time_us=<digits>`. `None` for `out_time_us=N/A` (ffmpeg's answer
/// before the first frame), a negative or signed number, a number too large
/// for a `u64`, and every other line.
pub fn parse_out_time(line: &str) -> Option<u64> {
    let digits = line
        .trim_end_matches(['\r', '\n'])
        .strip_prefix(OUT_TIME_KEY)?;
    // `u64::from_str` also takes a leading `+`, so the digits are checked first.
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// How far along the output is, in whole percent from 0 to 100, when the whole
/// run is `duration` seconds long. `None` when the duration is unknown, not a
/// finite number or not above zero.
pub fn percent(out_us: u64, duration: Option<f64>) -> Option<u8> {
    let duration = duration.filter(|duration| duration.is_finite() && *duration > 0.0)?;
    let percent = (out_us as f64 / 1e6 / duration * 100.0).round();
    // The clamp keeps a timestamp past the end at 100; `as` turns the checked
    // value into a `u8`.
    Some(percent.clamp(0.0, 100.0) as u8)
}
