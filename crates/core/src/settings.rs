//! The two settings the user chooses: the CRF (quality) and the audio choice.
//!
//! Every way a value can arrive (typed text on the command line, a JSON value
//! from the window, an integer in Rust) ends in the same range check, so no
//! entry point can accept a CRF another one refuses.

use serde_json::Value;

/// Why a setting was refused. The messages are shown to the user as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    /// A whole number, but not from 20 to 36.
    #[error("CRF must be a whole number from 20 to 36")]
    CrfOutOfRange(i64),
    /// Not a whole number at all: text with anything but digits, a fraction, a
    /// string, a boolean, `null`, an array or an object.
    #[error("CRF must be a whole number from 20 to 36")]
    CrfNotWholeNumber,
    /// Audio is `keep` or `remove`, nothing else.
    #[error("Audio must be keep or remove")]
    AudioUnknown,
}

/// The x264 constant rate factor: a whole number from [`Crf::MIN`] to
/// [`Crf::MAX`]. The field is private, so a value outside the range cannot be
/// built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crf(u8);

impl Crf {
    pub const MIN: u8 = 20;
    pub const MAX: u8 = 36;
    pub const DEFAULT: u8 = 28;

    /// The one range check every other constructor ends in.
    pub fn new(value: i64) -> Result<Crf, SettingsError> {
        if (i64::from(Self::MIN)..=i64::from(Self::MAX)).contains(&value) {
            // The range check above keeps the value inside a u8.
            Ok(Crf(value as u8))
        } else {
            Err(SettingsError::CrfOutOfRange(value))
        }
    }

    /// Reads typed text: ASCII digits only, so no sign, space, fraction or
    /// full-width digit. Digits too long for an `i64` are out of range.
    pub fn parse(text: &str) -> Result<Crf, SettingsError> {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(SettingsError::CrfNotWholeNumber);
        }
        match text.parse::<i64>() {
            Ok(value) => Crf::new(value),
            // All digits, so the only way to fail is to be too big.
            Err(_) => Err(SettingsError::CrfOutOfRange(i64::MAX)),
        }
    }

    /// Reads a JSON integer. A string (`"28"`), a float (even a whole one such
    /// as `28.0`), `null`, a boolean, an array or an object is refused.
    pub fn from_json(value: &Value) -> Result<Crf, SettingsError> {
        if value.is_i64() || value.is_u64() {
            // A u64 above i64::MAX has no i64; it is out of range all the same.
            Crf::new(value.as_i64().unwrap_or(i64::MAX))
        } else {
            Err(SettingsError::CrfNotWholeNumber)
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

impl Default for Crf {
    fn default() -> Self {
        Crf(Self::DEFAULT)
    }
}

/// Keep the audio (re-encoded as AAC) or remove it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Audio {
    #[default]
    Keep,
    Remove,
}

impl Audio {
    /// Accepts exactly the JSON strings `"keep"` and `"remove"`.
    pub fn from_json(value: &Value) -> Result<Audio, SettingsError> {
        match value.as_str() {
            Some("keep") => Ok(Audio::Keep),
            Some("remove") => Ok(Audio::Remove),
            _ => Err(SettingsError::AudioUnknown),
        }
    }
}

/// What one encode needs from the user. The window opens with the defaults;
/// nothing is remembered between launches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Settings {
    pub crf: Crf,
    pub audio: Audio,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_the_ends_and_the_default() {
        for v in [20, 28, 36] {
            assert_eq!(Crf::new(v).unwrap().get() as i64, v);
        }
    }

    #[test]
    fn rejects_values_outside_20_to_36() {
        for v in [19, 37, 0, 51, -1, i64::MIN, i64::MAX] {
            assert_eq!(Crf::new(v), Err(SettingsError::CrfOutOfRange(v)));
        }
    }

    #[test]
    fn parses_only_plain_digits() {
        assert_eq!(Crf::parse("28").unwrap().get(), 28);
        for t in ["28.5", "-1", "+28", " 28", "", "２８", "28a", "0x1c"] {
            assert_eq!(Crf::parse(t), Err(SettingsError::CrfNotWholeNumber), "{t}");
        }
    }

    #[test]
    fn parse_refuses_digits_outside_the_range_and_overlong_digits() {
        for t in ["19", "37", "0", "051", "99999999999999999999999"] {
            assert!(Crf::parse(t).is_err(), "{t}");
        }
        assert_eq!(Crf::parse("036").unwrap().get(), 36);
    }

    #[test]
    fn reads_only_a_json_integer() {
        assert_eq!(Crf::from_json(&json!(36)).unwrap().get(), 36);
        assert_eq!(Crf::from_json(&json!(20)).unwrap().get(), 20);
        for v in [
            json!(28.5),
            json!(28.0),
            json!("28"),
            json!(null),
            json!([28]),
            json!(f64::NAN),
            json!(true),
            json!({"crf": 28}),
        ] {
            assert_eq!(
                Crf::from_json(&v),
                Err(SettingsError::CrfNotWholeNumber),
                "{v}"
            );
        }
        for v in [json!(37), json!(19), json!(-1), json!(u64::MAX)] {
            assert!(
                matches!(Crf::from_json(&v), Err(SettingsError::CrfOutOfRange(_))),
                "{v}"
            );
        }
    }

    #[test]
    fn audio_reads_keep_and_remove_only() {
        assert_eq!(Audio::from_json(&json!("remove")).unwrap(), Audio::Remove);
        assert_eq!(Audio::from_json(&json!("keep")).unwrap(), Audio::Keep);
        for v in [
            json!("Keep"),
            json!(""),
            json!(1),
            json!(["keep"]),
            json!(null),
        ] {
            assert_eq!(
                Audio::from_json(&v),
                Err(SettingsError::AudioUnknown),
                "{v}"
            );
        }
    }

    #[test]
    fn defaults_are_crf_28_and_keep() {
        let s = Settings::default();
        assert_eq!(s.crf.get(), 28);
        assert_eq!(s.audio, Audio::Keep);
        assert_eq!(Crf::default().get(), Crf::DEFAULT);
        assert_eq!((Crf::MIN, Crf::MAX, Crf::DEFAULT), (20, 36, 28));
    }

    #[test]
    fn error_messages_state_the_range() {
        let want = "CRF must be a whole number from 20 to 36";
        assert_eq!(SettingsError::CrfOutOfRange(19).to_string(), want);
        assert_eq!(SettingsError::CrfNotWholeNumber.to_string(), want);
        assert_eq!(
            SettingsError::AudioUnknown.to_string(),
            "Audio must be keep or remove"
        );
    }
}
