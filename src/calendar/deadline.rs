// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Answer deadline of a session.

use jiff::civil::{Date, Time};
use jiff::{Timestamp, ToSpan};

use super::{CalendarError, paris_instant};

/// Until when players answer for a session: `days_before` days before its date, at `time`,
/// Paris time (`slots.deadline_days_before` and `slots.deadline_time`).
///
/// The slot stores a rule, not an instant: each date gets its own instant here, so the clock
/// changes are handled in one place. Cron Triggers run in UTC, and the offset is one hour in
/// winter and two in summer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadlineRule {
    days_before: i64,
    time: Time,
}

impl DeadlineRule {
    pub fn new(days_before: i64, time: Time) -> Result<Self, CalendarError> {
        if !(0..=6).contains(&days_before) {
            return Err(CalendarError::InvalidDeadlineDays(days_before));
        }
        Ok(Self { days_before, time })
    }

    /// The deadline of the session on `date`.
    pub fn for_date(&self, date: Date) -> Result<Timestamp, CalendarError> {
        let day = date
            .checked_sub(self.days_before.days())
            .map_err(|_| CalendarError::OutOfRange)?;
        paris_instant(day, self.time)
    }
}

/// The schema's default: the day before at 20:00.
impl Default for DeadlineRule {
    fn default() -> Self {
        Self {
            days_before: 1,
            time: Time::constant(20, 0, 0, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::{date, time};

    use super::*;

    fn utc(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    // Monday sessions, deadline on the Sunday before at 20:00: each 2027 clock change falls on
    // a Sunday, between the deadline of one session and the deadline of the next.
    #[test]
    fn deadline_on_each_side_of_the_2027_clock_changes() {
        let rule = DeadlineRule::default();
        for (session, deadline) in [
            // Winter time (UTC+1), then summer time (UTC+2) from Sunday 28 March.
            (date(2027, 3, 22), "2027-03-21T19:00:00Z"),
            (date(2027, 3, 29), "2027-03-28T18:00:00Z"),
            // Summer time, then winter time again from Sunday 31 October.
            (date(2027, 10, 25), "2027-10-24T18:00:00Z"),
            (date(2027, 11, 1), "2027-10-31T19:00:00Z"),
        ] {
            assert_eq!(rule.for_date(session).unwrap(), utc(deadline), "{session}");
        }
    }

    #[test]
    fn deadline_on_the_day_of_the_session() {
        let rule = DeadlineRule::new(0, time(12, 0, 0, 0)).unwrap();
        assert_eq!(
            rule.for_date(date(2027, 3, 28)).unwrap(),
            utc("2027-03-28T10:00:00Z")
        );
    }

    #[test]
    fn deadline_in_the_skipped_or_repeated_hour() {
        let rule = DeadlineRule::new(0, time(2, 30, 0, 0)).unwrap();
        // 02:30 does not exist on 28 March 2027: 03:30 summer time.
        assert_eq!(
            rule.for_date(date(2027, 3, 28)).unwrap(),
            utc("2027-03-28T01:30:00Z")
        );
        // 02:30 happens twice on 31 October 2027: the first one, still in summer time.
        assert_eq!(
            rule.for_date(date(2027, 10, 31)).unwrap(),
            utc("2027-10-31T00:30:00Z")
        );
    }

    #[test]
    fn deadline_up_to_six_days_before() {
        let rule = DeadlineRule::new(6, time(20, 0, 0, 0)).unwrap();
        // Saturday 3 April, summer time: the Sunday before is 28 March, already in summer time.
        assert_eq!(
            rule.for_date(date(2027, 4, 3)).unwrap(),
            utc("2027-03-28T18:00:00Z")
        );
        assert_eq!(
            DeadlineRule::new(7, time(20, 0, 0, 0)),
            Err(CalendarError::InvalidDeadlineDays(7))
        );
        assert_eq!(
            DeadlineRule::new(-1, time(20, 0, 0, 0)),
            Err(CalendarError::InvalidDeadlineDays(-1))
        );
    }
}
