//! Trade timestamps and calendar dates.

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

use super::FieldError;

/// A UTC execution instant, supplied exactly as `YYYY-MM-DDTHH:MM:SSZ`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TradeTimestamp(DateTime<Utc>);

impl TradeTimestamp {
    pub const FORMAT: &'static str = "%Y-%m-%dT%H:%M:%SZ";

    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        let invalid = |source| FieldError::InvalidTimestamp {
            value: raw.to_owned(),
            source,
        };

        // chrono accepts unpadded fields such as "2026-3-9", so check the exact
        // shape first and let chrono validate the calendar values.
        let shape_ok = raw.len() == 20
            && raw.bytes().enumerate().all(|(i, b)| match i {
                4 | 7 => b == b'-',
                10 => b == b'T',
                13 | 16 => b == b':',
                19 => b == b'Z',
                _ => b.is_ascii_digit(),
            });
        if !shape_ok {
            return Err(invalid(None));
        }

        let naive = NaiveDateTime::parse_from_str(raw, Self::FORMAT)
            .map_err(|source| invalid(Some(source)))?;
        Ok(Self(naive.and_utc()))
    }

    pub fn utc_date(self) -> NaiveDate {
        self.0.date_naive()
    }

    /// Canonical storage form; sorts lexicographically in time order.
    pub fn to_canonical(self) -> String {
        self.0.format(Self::FORMAT).to_string()
    }
}

/// Parses a `YYYY-MM-DD` calendar date.
pub fn parse_date(raw: &str, field: &'static str) -> Result<NaiveDate, FieldError> {
    let invalid = |source| FieldError::InvalidDate {
        field,
        value: raw.to_owned(),
        source,
    };

    let shape_ok = raw.len() == 10
        && raw.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => b == b'-',
            _ => b.is_ascii_digit(),
        });
    if !shape_ok {
        return Err(invalid(None));
    }

    NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|source| invalid(Some(source)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_requires_exact_utc_format() {
        let ts = TradeTimestamp::parse("2026-01-22T08:52:12Z").unwrap();
        assert_eq!(ts.utc_date(), NaiveDate::from_ymd_opt(2026, 1, 22).unwrap());
        assert_eq!(ts.to_canonical(), "2026-01-22T08:52:12Z");

        for bad in [
            "2026-03-13 15:10:35",
            "2026-03-13T15:10:35",
            "2026-03-13T15:10:35+00:00",
            "2026-3-13T15:10:35Z",
            "2026-03-13T15:10:35.000Z",
            "2026-02-30T10:00:00Z",
            "2026-03-13T25:00:00Z",
            "",
        ] {
            assert!(TradeTimestamp::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn dates_require_iso_format() {
        assert!(parse_date("2026-03-27", "date").is_ok());
        assert!(parse_date("2026-3-27", "date").is_err());
        assert!(parse_date("27/03/2026", "date").is_err());
        assert!(parse_date("2026-02-30", "date").is_err());
    }
}
