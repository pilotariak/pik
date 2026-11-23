// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Text forms of local dates and times: `YYYY-MM-DD` and `HH:MM`, as stored in D1 and sent in
//! paths and bodies. Both are Paris local values (see [`calendar`](super::calendar)).

use jiff::civil::{Date, Time};

/// The two-digit number at `s[at..at + 2]`.
fn two_digits(bytes: &[u8], at: usize) -> Option<i8> {
    let tens = bytes.get(at).filter(|b| b.is_ascii_digit())?;
    let units = bytes.get(at + 1).filter(|b| b.is_ascii_digit())?;
    i8::try_from((tens - b'0') * 10 + (units - b'0')).ok()
}

/// A `YYYY-MM-DD` date. Any other form, or a day that does not exist, is `None`.
pub(crate) fn parse_date(s: &str) -> Option<Date> {
    let bytes = s.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = s.get(0..4)?;
    if !year.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i16 = year.parse().ok()?;
    Date::new(year, two_digits(bytes, 5)?, two_digits(bytes, 8)?).ok()
}

/// An `HH:MM` time from 00:00 to 23:59.
pub(crate) fn parse_time(s: &str) -> Option<Time> {
    let bytes = s.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return None;
    }
    Time::new(two_digits(bytes, 0)?, two_digits(bytes, 3)?, 0, 0).ok()
}

/// The `HH:MM` form of a time.
pub(crate) fn format_time(time: Time) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

#[cfg(test)]
mod tests {
    use jiff::civil::{date, time};

    use super::{format_time, parse_date, parse_time};

    #[test]
    fn dates_are_yyyy_mm_dd_only() {
        assert_eq!(
            parse_date("2026-11-25"),
            Some(date(2026, 11, 25)),
            "a valid date"
        );
        assert_eq!(
            date(2026, 11, 25).to_string(),
            "2026-11-25",
            "jiff writes the same form"
        );
        for invalid in [
            "2026-02-30",
            "2026-13-01",
            "20261125",
            "2026-11-25T00:00",
            "26-11-25",
            "2026-1-25",
            "+002026-11-25",
            "",
        ] {
            assert_eq!(parse_date(invalid), None, "{invalid:?} is not a date");
        }
    }

    #[test]
    fn times_are_hh_mm_only() {
        assert_eq!(
            parse_time("18:30"),
            Some(time(18, 30, 0, 0)),
            "a valid time"
        );
        assert_eq!(parse_time("00:00"), Some(time(0, 0, 0, 0)), "midnight");
        for invalid in ["24:00", "18:60", "8:30", "18:30:00", "18h30", ""] {
            assert_eq!(parse_time(invalid), None, "{invalid:?} is not a time");
        }
        assert_eq!(format_time(time(8, 5, 0, 0)), "08:05", "two digits each");
    }
}
