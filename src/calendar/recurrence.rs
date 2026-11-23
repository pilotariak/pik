// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Session dates of a slot.

use jiff::ToSpan;
use jiff::civil::{Date, Weekday};

use super::CalendarError;

/// How often a slot repeats (`slots.interval_weeks`).
///
/// Fixed for the season, like the weekday: answers are keyed by date, so a new schedule is a
/// new slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interval {
    Weekly,
    Fortnightly,
}

impl Interval {
    /// Days between two sessions.
    pub fn days(self) -> i64 {
        match self {
            Self::Weekly => 7,
            Self::Fortnightly => 14,
        }
    }
}

impl TryFrom<i64> for Interval {
    type Error = CalendarError;

    fn try_from(weeks: i64) -> Result<Self, CalendarError> {
        match weeks {
            1 => Ok(Self::Weekly),
            2 => Ok(Self::Fortnightly),
            _ => Err(CalendarError::InvalidInterval(weeks)),
        }
    }
}

/// The session dates of a slot: `anchor_date + k × 7 × interval_weeks` days for k ≥ 0, up to
/// `season_end`.
///
/// The anchor is the first session and gives the weekday. There is no rule on even or odd ISO
/// weeks: 2026 has 53 ISO weeks, and weeks 53 and 1 are both odd. Two slots that alternate on
/// the same weekday have anchors one week apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recurrence {
    anchor: Date,
    interval: Interval,
    season_end: Date,
}

impl Recurrence {
    /// The recurrence of a slot, from `anchor_date`, `interval_weeks`, `season_start` and
    /// `season_end`.
    pub fn new(
        anchor: Date,
        interval: Interval,
        season_start: Date,
        season_end: Date,
    ) -> Result<Self, CalendarError> {
        if !(season_start..=season_end).contains(&anchor) {
            return Err(CalendarError::AnchorOutsideSeason);
        }
        Ok(Self {
            anchor,
            interval,
            season_end,
        })
    }

    /// The weekday of every session.
    pub fn weekday(&self) -> Weekday {
        self.anchor.weekday()
    }

    /// Every session date of the season, in order.
    pub fn dates(&self) -> impl Iterator<Item = Date> {
        let season_end = self.season_end;
        self.anchor
            .series(self.interval.days().days())
            .take_while(move |date| *date <= season_end)
    }

    /// Whether a session falls on `date`.
    pub fn contains(&self, date: Date) -> bool {
        // Civil dates have no clock change: every day lasts 24 hours.
        let days = self.anchor.duration_until(date).as_hours() / 24;
        date <= self.season_end && days >= 0 && days % self.interval.days() == 0
    }

    /// The first session on or after `date`, if the season has one.
    pub fn next_on_or_after(&self, date: Date) -> Option<Date> {
        self.dates().find(|session| *session >= date)
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::*;

    fn iso_week(date: Date) -> (i16, i8) {
        let week = date.iso_week_date();
        (week.year(), week.week())
    }

    #[test]
    fn weekly_slot_repeats_every_seven_days_until_the_season_ends() {
        let slot = Recurrence::new(
            date(2026, 9, 7),
            Interval::Weekly,
            date(2026, 9, 1),
            date(2026, 10, 6),
        )
        .unwrap();
        assert_eq!(slot.weekday(), Weekday::Monday);
        assert_eq!(
            slot.dates().collect::<Vec<_>>(),
            [
                date(2026, 9, 7),
                date(2026, 9, 14),
                date(2026, 9, 21),
                date(2026, 9, 28),
                date(2026, 10, 5),
            ]
        );
    }

    #[test]
    fn fortnightly_slot_keeps_fourteen_days_across_iso_week_53_of_2026() {
        // The premise: 2026 has 53 ISO weeks.
        assert_eq!(date(2026, 12, 28).iso_week_date().weeks_in_year(), 53);

        let slot = Recurrence::new(
            date(2026, 12, 14),
            Interval::Fortnightly,
            date(2026, 9, 1),
            date(2027, 1, 31),
        )
        .unwrap();
        let dates: Vec<_> = slot.dates().collect();
        assert_eq!(
            dates,
            [
                date(2026, 12, 14),
                date(2026, 12, 28),
                date(2027, 1, 11),
                date(2027, 1, 25),
            ]
        );
        assert_eq!(
            dates.iter().copied().map(iso_week).collect::<Vec<_>>(),
            [(2026, 51), (2026, 53), (2027, 2), (2027, 4)]
        );
    }

    #[test]
    fn alternating_slots_never_meet_although_weeks_53_and_1_are_both_odd() {
        let season = (date(2026, 9, 1), date(2027, 6, 30));
        let a = Recurrence::new(
            date(2026, 12, 21),
            Interval::Fortnightly,
            season.0,
            season.1,
        )
        .unwrap();
        let b = Recurrence::new(
            date(2026, 12, 28),
            Interval::Fortnightly,
            season.0,
            season.1,
        )
        .unwrap();

        // Week 53 belongs to B and week 1 to A, one week apart: an odd-week rule would give both
        // to the same slot, and that slot would play two weeks in a row.
        assert!(b.contains(date(2026, 12, 28)));
        assert!(a.contains(date(2027, 1, 4)));
        assert_eq!(iso_week(date(2026, 12, 28)), (2026, 53));
        assert_eq!(iso_week(date(2027, 1, 4)), (2027, 1));

        let mut both: Vec<_> = a.dates().chain(b.dates()).collect();
        both.sort();
        for pair in both.windows(2) {
            assert_eq!(pair[0].duration_until(pair[1]).as_hours(), 7 * 24);
        }
    }

    #[test]
    fn contains_only_the_dates_of_the_recurrence_within_the_season() {
        let slot = Recurrence::new(
            date(2026, 12, 14),
            Interval::Fortnightly,
            date(2026, 9, 1),
            date(2027, 1, 25),
        )
        .unwrap();
        assert!(slot.contains(date(2026, 12, 14)));
        assert!(slot.contains(date(2026, 12, 28)));
        assert!(slot.contains(date(2027, 1, 25)));
        // Same weekday, the off week.
        assert!(!slot.contains(date(2026, 12, 21)));
        // Another weekday.
        assert!(!slot.contains(date(2026, 12, 15)));
        // Two weeks before the first session.
        assert!(!slot.contains(date(2026, 11, 30)));
        // After the season.
        assert!(!slot.contains(date(2027, 2, 8)));
    }

    #[test]
    fn next_session_on_or_after_a_date() {
        let slot = Recurrence::new(
            date(2026, 12, 14),
            Interval::Fortnightly,
            date(2026, 9, 1),
            date(2027, 1, 25),
        )
        .unwrap();
        assert_eq!(
            slot.next_on_or_after(date(2026, 9, 1)),
            Some(date(2026, 12, 14))
        );
        assert_eq!(
            slot.next_on_or_after(date(2026, 12, 15)),
            Some(date(2026, 12, 28))
        );
        assert_eq!(
            slot.next_on_or_after(date(2026, 12, 28)),
            Some(date(2026, 12, 28))
        );
        assert_eq!(slot.next_on_or_after(date(2027, 1, 26)), None);
    }

    #[test]
    fn rejects_an_interval_other_than_one_or_two_weeks() {
        assert_eq!(Interval::try_from(1), Ok(Interval::Weekly));
        assert_eq!(Interval::try_from(2), Ok(Interval::Fortnightly));
        assert_eq!(
            Interval::try_from(3),
            Err(CalendarError::InvalidInterval(3))
        );
    }

    #[test]
    fn rejects_a_first_session_outside_the_season() {
        let new = |anchor| {
            Recurrence::new(
                anchor,
                Interval::Weekly,
                date(2026, 9, 1),
                date(2027, 6, 30),
            )
        };
        assert!(new(date(2026, 9, 1)).is_ok());
        assert!(new(date(2027, 6, 30)).is_ok());
        assert_eq!(
            new(date(2026, 8, 31)),
            Err(CalendarError::AnchorOutsideSeason)
        );
        assert_eq!(
            new(date(2027, 7, 1)),
            Err(CalendarError::AnchorOutsideSeason)
        );
    }
}
