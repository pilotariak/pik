// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Effective values and state of a session.

use std::str::FromStr;

use jiff::Timestamp;
use jiff::civil::{Date, Time};

use super::deadline::DeadlineRule;
use super::recurrence::Recurrence;
use super::{CalendarError, paris_instant};

/// A player's answer for a session (`answers.status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// Comes.
    Ok,
    /// Can come if needed. Dispo answers do not count toward the minimum: they complete a
    /// session when the organizer calls them.
    Dispo,
    /// Does not come.
    Ko,
}

impl Status {
    /// The value stored in `answers.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Dispo => "dispo",
            Self::Ko => "ko",
        }
    }
}

impl FromStr for Status {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "ok" => Ok(Self::Ok),
            "dispo" => Ok(Self::Dispo),
            "ko" => Ok(Self::Ko),
            _ => Err(()),
        }
    }
}

/// What the calendar needs from a `slots` row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub recurrence: Recurrence,
    /// `time_from`: when a session starts, Paris time.
    pub time_from: Time,
    pub min_players: u32,
    pub places: u32,
    pub deadline: DeadlineRule,
}

/// What differs on one date: a `session_overrides` row. `None` keeps the slot's value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Override {
    /// `cancelled_at` is set. Setting it back to `NULL` undoes the cancellation.
    pub cancelled: bool,
    pub min_players: Option<u32>,
    pub places: Option<u32>,
    pub deadline_at: Option<Timestamp>,
}

/// One session of a slot, with its effective values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session {
    pub date: Date,
    pub starts_at: Timestamp,
    pub min_players: u32,
    pub places: u32,
    pub deadline: Timestamp,
    pub cancelled: bool,
}

impl Slot {
    /// The session on `date`: each value comes from the date's override when set, otherwise
    /// from the slot. `places >= min_players` must hold on the effective values.
    pub fn session(
        &self,
        date: Date,
        overrides: Option<&Override>,
    ) -> Result<Session, CalendarError> {
        if !self.recurrence.contains(date) {
            return Err(CalendarError::NotASessionDate(date));
        }
        let overrides = overrides.copied().unwrap_or_default();
        let min_players = overrides.min_players.unwrap_or(self.min_players);
        let places = overrides.places.unwrap_or(self.places);
        if places < min_players {
            return Err(CalendarError::PlacesBelowMinimum {
                places,
                min_players,
            });
        }
        let deadline = match overrides.deadline_at {
            Some(deadline) => deadline,
            None => self.deadline.for_date(date)?,
        };
        Ok(Session {
            date,
            starts_at: paris_instant(date, self.time_from)?,
            min_players,
            places,
            deadline,
            cancelled: overrides.cancelled,
        })
    }
}

/// Number of OK and Dispo answers of a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub ok: u32,
    pub dispo: u32,
}

impl FromIterator<Status> for Tally {
    fn from_iter<I: IntoIterator<Item = Status>>(statuses: I) -> Self {
        statuses
            .into_iter()
            .fold(Self::default(), |mut tally, status| {
                match status {
                    Status::Ok => tally.ok += 1,
                    Status::Dispo => tally.dispo += 1,
                    Status::Ko => {}
                }
                tally
            })
    }
}

/// State of a session, computed on every read and never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// *Assurée*: the OK answers reach the minimum.
    On,
    /// *Il manque n*: before the deadline, n more OK answers are needed.
    Missing(u32),
    /// *À compléter*: at the deadline, the OK answers do not reach the minimum, but they would
    /// with the Dispo answers.
    ToComplete,
    /// At the deadline, even the Dispo answers would not reach the minimum.
    CancelledForLackOfPlayers,
    /// The organizer cancelled the date. It stays visible.
    CancelledByOrganizer,
}

impl Session {
    /// The state of the session at `now`. The deadline itself already counts as reached.
    pub fn state(&self, tally: Tally, now: Timestamp) -> State {
        if self.cancelled {
            State::CancelledByOrganizer
        } else if tally.ok >= self.min_players {
            State::On
        } else if now < self.deadline {
            State::Missing(self.min_players - tally.ok)
        } else if tally.ok + tally.dispo >= self.min_players {
            State::ToComplete
        } else {
            State::CancelledForLackOfPlayers
        }
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::{date, time};

    use super::*;
    use crate::calendar::recurrence::Interval;

    fn utc(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    /// Mondays at 19:00 from 1 March to 30 November 2027, at least 4 players, 6 places, answers
    /// until the Sunday before at 20:00.
    fn slot() -> Slot {
        Slot {
            recurrence: Recurrence::new(
                date(2027, 3, 1),
                Interval::Weekly,
                date(2027, 3, 1),
                date(2027, 11, 30),
            )
            .unwrap(),
            time_from: time(19, 0, 0, 0),
            min_players: 4,
            places: 6,
            deadline: DeadlineRule::default(),
        }
    }

    fn tally(ok: u32, dispo: u32) -> Tally {
        Tally { ok, dispo }
    }

    #[test]
    fn sessions_on_each_side_of_the_2027_clock_changes() {
        let slot = slot();
        for (date, starts_at, deadline) in [
            (
                date(2027, 3, 22),
                "2027-03-22T18:00:00Z",
                "2027-03-21T19:00:00Z",
            ),
            (
                date(2027, 3, 29),
                "2027-03-29T17:00:00Z",
                "2027-03-28T18:00:00Z",
            ),
            (
                date(2027, 10, 25),
                "2027-10-25T17:00:00Z",
                "2027-10-24T18:00:00Z",
            ),
            (
                date(2027, 11, 1),
                "2027-11-01T18:00:00Z",
                "2027-10-31T19:00:00Z",
            ),
        ] {
            let session = slot.session(date, None).unwrap();
            assert_eq!(session.starts_at, utc(starts_at), "{date}");
            assert_eq!(session.deadline, utc(deadline), "{date}");
        }
    }

    #[test]
    fn effective_values_come_from_the_slot_without_override() {
        let session = slot().session(date(2027, 3, 22), None).unwrap();
        assert_eq!(
            session,
            Session {
                date: date(2027, 3, 22),
                starts_at: utc("2027-03-22T18:00:00Z"),
                min_players: 4,
                places: 6,
                deadline: utc("2027-03-21T19:00:00Z"),
                cancelled: false,
            }
        );
    }

    #[test]
    fn each_override_replaces_only_its_own_value() {
        let slot = slot();
        let day = date(2027, 3, 22);

        let fewer = Override {
            min_players: Some(2),
            ..Override::default()
        };
        let session = slot.session(day, Some(&fewer)).unwrap();
        assert_eq!((session.min_players, session.places), (2, 6));
        assert_eq!(session.deadline, utc("2027-03-21T19:00:00Z"));

        let earlier = Override {
            places: Some(4),
            deadline_at: Some(utc("2027-03-20T11:00:00Z")),
            ..Override::default()
        };
        let session = slot.session(day, Some(&earlier)).unwrap();
        assert_eq!((session.min_players, session.places), (4, 4));
        assert_eq!(session.deadline, utc("2027-03-20T11:00:00Z"));
    }

    #[test]
    fn places_must_reach_the_minimum_on_the_effective_values() {
        let slot = slot();
        let day = date(2027, 3, 22);
        // The slot has 6 places: a minimum of 8 for one date does not fit.
        let more = Override {
            min_players: Some(8),
            ..Override::default()
        };
        assert_eq!(
            slot.session(day, Some(&more)),
            Err(CalendarError::PlacesBelowMinimum {
                places: 6,
                min_players: 8,
            })
        );
        // Unless that date also gets more places.
        let more_with_places = Override {
            places: Some(8),
            ..more
        };
        assert!(slot.session(day, Some(&more_with_places)).is_ok());
        // Fewer places than the slot's minimum of 4.
        let three = Override {
            places: Some(3),
            ..Override::default()
        };
        assert_eq!(
            slot.session(day, Some(&three)),
            Err(CalendarError::PlacesBelowMinimum {
                places: 3,
                min_players: 4,
            })
        );
    }

    #[test]
    fn only_the_dates_of_the_recurrence_are_sessions() {
        assert_eq!(
            slot().session(date(2027, 3, 23), None),
            Err(CalendarError::NotASessionDate(date(2027, 3, 23)))
        );
        assert_eq!(
            slot().session(date(2027, 12, 6), None),
            Err(CalendarError::NotASessionDate(date(2027, 12, 6)))
        );
    }

    #[test]
    fn before_the_deadline_the_session_is_on_or_missing_ok_answers() {
        let session = slot().session(date(2027, 3, 22), None).unwrap();
        let before = utc("2027-03-21T18:59:59.999Z");
        assert_eq!(session.state(tally(4, 0), before), State::On);
        assert_eq!(session.state(tally(5, 0), before), State::On);
        assert_eq!(session.state(tally(1, 0), before), State::Missing(3));
        assert_eq!(session.state(tally(0, 0), before), State::Missing(4));
    }

    #[test]
    fn dispo_answers_do_not_count_toward_the_minimum() {
        let session = slot().session(date(2027, 3, 22), None).unwrap();
        let before = utc("2027-03-21T12:00:00Z");
        assert_eq!(session.state(tally(2, 5), before), State::Missing(2));
        let at = session.deadline;
        assert_eq!(session.state(tally(2, 5), at), State::ToComplete);
    }

    #[test]
    fn at_the_deadline_the_session_is_on_to_complete_or_cancelled() {
        let session = slot().session(date(2027, 3, 22), None).unwrap();
        let at = utc("2027-03-21T19:00:00Z");
        assert_eq!(session.deadline, at);
        assert_eq!(session.state(tally(4, 0), at), State::On);
        assert_eq!(session.state(tally(3, 1), at), State::ToComplete);
        assert_eq!(session.state(tally(1, 3), at), State::ToComplete);
        assert_eq!(
            session.state(tally(1, 2), at),
            State::CancelledForLackOfPlayers
        );
        // And after it.
        let after = utc("2027-03-22T18:00:00Z");
        assert_eq!(
            session.state(tally(0, 0), after),
            State::CancelledForLackOfPlayers
        );
    }

    #[test]
    fn cancelled_date_then_uncancelled() {
        let slot = slot();
        let day = date(2027, 3, 22);
        let before = utc("2027-03-20T12:00:00Z");
        let enough = tally(5, 0);

        let mut overrides = Override {
            cancelled: true,
            min_players: Some(5),
            ..Override::default()
        };
        let session = slot.session(day, Some(&overrides)).unwrap();
        assert_eq!(session.state(enough, before), State::CancelledByOrganizer);
        assert_eq!(
            session.state(enough, session.deadline),
            State::CancelledByOrganizer
        );

        // `cancelled_at` back to NULL: the date is a normal session again, and keeps the rest of
        // its override.
        overrides.cancelled = false;
        let session = slot.session(day, Some(&overrides)).unwrap();
        assert_eq!(session.min_players, 5);
        assert_eq!(session.state(enough, before), State::On);
        assert_eq!(session.state(tally(4, 0), before), State::Missing(1));
    }

    #[test]
    fn tally_counts_ok_and_dispo_answers() {
        let tally: Tally = [
            Status::Ok,
            Status::Ko,
            Status::Dispo,
            Status::Ok,
            Status::Ko,
        ]
        .into_iter()
        .collect();
        assert_eq!(tally, Tally { ok: 2, dispo: 1 });
    }

    #[test]
    fn status_reads_and_writes_the_stored_values() {
        for status in [Status::Ok, Status::Dispo, Status::Ko] {
            assert_eq!(status.as_str().parse(), Ok(status));
        }
        assert_eq!("OK".parse::<Status>(), Err(()));
        assert_eq!("maybe".parse::<Status>(), Err(()));
    }
}
