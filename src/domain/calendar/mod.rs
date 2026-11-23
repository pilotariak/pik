// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Session calendar of a slot.
//!
//! Sessions are not stored: their dates follow from the slot's recurrence, and only the values
//! that differ on one date are, in `session_overrides`. From the rows of a slot, this module
//! computes which dates it has ([`recurrence`]), until when players can answer ([`deadline`]),
//! the effective values and the state of each session ([`session`]), and the order in which
//! players are called ([`call_order`]).
//!
//! Every function is pure: none reads a clock or does I/O, so `cargo test` covers them on the
//! host. Callers pass `now`. Local dates and times are in Europe/Paris, and instants are UTC
//! [`Timestamp`]s, stored in D1 as milliseconds since the epoch.

pub(crate) mod call_order;
pub(crate) mod deadline;
pub(crate) mod recurrence;
pub(crate) mod session;

use std::fmt;

use jiff::Timestamp;
use jiff::civil::{Date, Time};
use jiff::tz::{self, TimeZone};

/// The time zone of every slot date and time.
///
/// A Worker has no system tz database, so `tz::get!` embeds the Europe/Paris rules from
/// jiff-tzdb at compile time: a tzdata update needs a rebuild.
pub(crate) static PARIS: TimeZone = tz::get!("Europe/Paris");

/// The instant of a local date and time in Paris.
///
/// A time skipped when the clocks go forward (02:30 on the last Sunday of March) takes the offset
/// in force before the change, so it lands at 03:30 summer time. A time repeated when the clocks
/// go back (02:30 on the last Sunday of October) also takes the offset in force before the
/// change: its first occurrence, in summer time. This is jiff's `compatible` disambiguation.
pub(crate) fn paris_instant(date: Date, time: Time) -> Result<Timestamp, CalendarError> {
    PARIS
        .to_timestamp(date.to_datetime(time))
        .map_err(|_| CalendarError::OutOfRange)
}

/// The local date in Paris at an instant.
pub(crate) fn paris_date(at: Timestamp) -> Date {
    PARIS.to_datetime(at).date()
}

/// A slot or a session whose values the calendar cannot use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CalendarError {
    /// `interval_weeks` is neither 1 nor 2.
    InvalidInterval(i64),
    /// `anchor_date` is outside `season_start..=season_end`.
    AnchorOutsideSeason,
    /// `deadline_days_before` is outside `0..=6`.
    InvalidDeadlineDays(i64),
    /// No session of the slot falls on this date.
    NotASessionDate(Date),
    /// The effective places of a date are fewer than its effective minimum.
    PlacesBelowMinimum { places: u32, min_players: u32 },
    /// A date or an instant outside the range jiff supports.
    OutOfRange,
}

impl fmt::Display for CalendarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInterval(weeks) => {
                write!(f, "a slot repeats every 1 or 2 weeks, not every {weeks}")
            }
            Self::AnchorOutsideSeason => f.write_str("the first session is outside the season"),
            Self::InvalidDeadlineDays(days) => {
                write!(
                    f,
                    "the deadline is 0 to 6 days before a session, not {days}"
                )
            }
            Self::NotASessionDate(date) => write!(f, "no session falls on {date}"),
            Self::PlacesBelowMinimum {
                places,
                min_players,
            } => write!(
                f,
                "{places} places are fewer than the minimum of {min_players}"
            ),
            Self::OutOfRange => f.write_str("date out of range"),
        }
    }
}

impl std::error::Error for CalendarError {}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use jiff::civil::{date, time};

    use super::{PARIS, paris_date, paris_instant};

    fn utc(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    // The premise of the clock change tests: in 2027 Paris goes to summer time on Sunday 28
    // March at 01:00 UTC and back on Sunday 31 October at 01:00 UTC.
    #[test]
    fn embedded_rules_change_clocks_on_the_2027_dates() {
        let mut changes = PARIS.following(utc("2027-01-01T00:00:00Z"));
        let spring = changes.next().unwrap();
        let autumn = changes.next().unwrap();
        assert_eq!(
            spring.timestamp(),
            utc("2027-03-28T01:00:00Z"),
            "summer time starts on the last Sunday of March at 01:00 UTC"
        );
        assert_eq!(spring.offset().seconds(), 2 * 3600, "summer time is UTC+2");
        assert_eq!(
            autumn.timestamp(),
            utc("2027-10-31T01:00:00Z"),
            "summer time ends on the last Sunday of October at 01:00 UTC"
        );
        assert_eq!(autumn.offset().seconds(), 3600, "winter time is UTC+1");
    }

    #[test]
    fn instant_of_a_local_time_follows_the_offset_of_the_day() {
        for (day, expected) in [
            (date(2027, 3, 27), "2027-03-27T19:00:00Z"),
            (date(2027, 3, 28), "2027-03-28T18:00:00Z"),
            (date(2027, 10, 30), "2027-10-30T18:00:00Z"),
            (date(2027, 10, 31), "2027-10-31T19:00:00Z"),
        ] {
            assert_eq!(
                paris_instant(day, time(20, 0, 0, 0)).unwrap(),
                utc(expected),
                "20:00 on {day} in Paris"
            );
        }
    }

    #[test]
    fn skipped_local_time_moves_forward_and_repeated_one_takes_summer_time() {
        assert_eq!(
            paris_instant(date(2027, 3, 28), time(2, 30, 0, 0)).unwrap(),
            utc("2027-03-28T01:30:00Z"),
            "02:30 does not exist on 28 March 2027: 03:30 summer time"
        );
        assert_eq!(
            paris_instant(date(2027, 10, 31), time(2, 30, 0, 0)).unwrap(),
            utc("2027-10-31T00:30:00Z"),
            "02:30 happens twice on 31 October 2027: the first one, in summer time"
        );
    }

    #[test]
    fn local_date_changes_at_midnight_paris_time() {
        // Midnight in Paris is 23:00 UTC in winter and 22:00 UTC in summer.
        for (at, expected) in [
            ("2027-01-14T23:30:00Z", date(2027, 1, 15)),
            ("2027-07-14T22:30:00Z", date(2027, 7, 15)),
            ("2027-07-14T21:59:59Z", date(2027, 7, 14)),
        ] {
            assert_eq!(paris_date(utc(at)), expected, "Paris date at {at}");
        }
    }
}
