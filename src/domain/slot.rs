// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! A slot and its members, as stored in `slots` and `members`, and the rules on their values.

use core::fmt;
use core::str::FromStr;

use jiff::civil::{Date, Time};
use serde::{Deserialize, Serialize};

use super::calendar::CalendarError;
use super::calendar::deadline::DeadlineRule;
use super::calendar::recurrence::{Interval, Recurrence};
use super::calendar::session;
use super::ids::{MemberId, PlayerId, SlotId};
use super::invite::InviteCode;

/// Longest slot name, in characters.
const MAX_SLOT_NAME_CHARS: usize = 60;
/// Longest venue, in characters.
const MAX_VENUE_CHARS: usize = 80;
/// Longest specialty id, in characters.
const MAX_SPECIALTY_CHARS: usize = 40;
/// Most players a session can have.
pub(crate) const MAX_PLAYERS: u32 = 30;
/// Longest season, in days: one year, leap day included.
const MAX_SEASON_DAYS: i32 = 366;

/// `slots.court`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(crate) enum Court {
    Trinquet,
    MurAGauche,
    PlaceLibre,
}

impl Court {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Trinquet => "trinquet",
            Self::MurAGauche => "mur_a_gauche",
            Self::PlaceLibre => "place_libre",
        }
    }
}

impl FromStr for Court {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "trinquet" => Ok(Self::Trinquet),
            "mur_a_gauche" => Ok(Self::MurAGauche),
            "place_libre" => Ok(Self::PlaceLibre),
            _ => Err(()),
        }
    }
}

/// `members.role`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(crate) enum Role {
    /// Changes the slot for everyone: settings, dates, list of names, lineups.
    Organizer,
    /// Answers for themselves.
    Player,
}

impl Role {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Organizer => "organizer",
            Self::Player => "player",
        }
    }
}

impl FromStr for Role {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "organizer" => Ok(Self::Organizer),
            "player" => Ok(Self::Player),
            _ => Err(()),
        }
    }
}

/// A `slots` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Slot {
    pub(crate) id: SlotId,
    pub(crate) name: String,
    pub(crate) venue: String,
    pub(crate) court: Court,
    /// `gomme_creuse`, `gomme_pleine`… or a Frontis specialty id.
    pub(crate) specialty: String,
    pub(crate) interval: Interval,
    /// The first session: it gives the weekday. Fixed, like the interval.
    pub(crate) anchor_date: Date,
    pub(crate) time_from: Time,
    pub(crate) time_to: Option<Time>,
    pub(crate) min_players: u32,
    pub(crate) places: u32,
    /// Points of a game: 30, 35 or 40.
    pub(crate) target: u32,
    pub(crate) deadline_days_before: i64,
    pub(crate) deadline_time: Time,
    pub(crate) season_start: Date,
    pub(crate) season_end: Date,
    pub(crate) invite_code: InviteCode,
    pub(crate) list_closed: bool,
}

/// A slot value that breaks a rule: the API answers 422 with this text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InvalidSetting {
    pub(crate) field: &'static str,
    pub(crate) reason: String,
}

impl fmt::Display for InvalidSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.reason)
    }
}

impl std::error::Error for InvalidSetting {}

fn invalid(field: &'static str, reason: impl Into<String>) -> InvalidSetting {
    InvalidSetting {
        field,
        reason: reason.into(),
    }
}

/// Trimmed text of 1 to `max` characters without control characters.
pub(crate) fn short_text(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<String, InvalidSetting> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid(field, "cannot be empty"));
    }
    if value.chars().count() > max {
        return Err(invalid(field, format!("has at most {max} characters")));
    }
    if value.chars().any(char::is_control) {
        return Err(invalid(field, "cannot hold control characters"));
    }
    Ok(value.to_owned())
}

impl Slot {
    /// The rules of a slot, checked on creation and on every change. The schema checks some of
    /// them too, but a 422 says which value is wrong.
    pub(crate) fn validate(&mut self) -> Result<(), InvalidSetting> {
        self.name = short_text("name", &self.name, MAX_SLOT_NAME_CHARS)?;
        self.venue = short_text("venue", &self.venue, MAX_VENUE_CHARS)?;
        let specialty_ok = !self.specialty.is_empty()
            && self.specialty.chars().count() <= MAX_SPECIALTY_CHARS
            && self
                .specialty
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        if !specialty_ok {
            return Err(invalid(
                "specialty",
                format!("1 to {MAX_SPECIALTY_CHARS} characters among a-z, 0-9, _ and -"),
            ));
        }
        if !(1..=MAX_PLAYERS).contains(&self.min_players) {
            return Err(invalid("min_players", format!("from 1 to {MAX_PLAYERS}")));
        }
        if !(self.min_players..=MAX_PLAYERS).contains(&self.places) {
            return Err(invalid(
                "places",
                format!("from min_players to {MAX_PLAYERS}"),
            ));
        }
        if ![30, 35, 40].contains(&self.target) {
            return Err(invalid("target", "30, 35 or 40"));
        }
        if self
            .time_to
            .is_some_and(|time_to| time_to <= self.time_from)
        {
            return Err(invalid("time_to", "must be after time_from"));
        }
        if self.season_end < self.season_start {
            return Err(invalid("season_end", "must not be before season_start"));
        }
        let season_days = self
            .season_start
            .until(self.season_end)
            .map_err(|_| invalid("season_end", "out of range"))?
            .get_days();
        if season_days > MAX_SEASON_DAYS {
            return Err(invalid(
                "season_end",
                format!("a season lasts at most {MAX_SEASON_DAYS} days"),
            ));
        }
        self.calendar().map_err(|error| match error {
            CalendarError::AnchorOutsideSeason => {
                invalid("anchor_date", "must be within the season")
            }
            CalendarError::InvalidDeadlineDays(_) => invalid("deadline_days_before", "from 0 to 6"),
            other => invalid("slot", other.to_string()),
        })?;
        Ok(())
    }

    /// What the calendar needs from this slot.
    pub(crate) fn calendar(&self) -> Result<session::Slot, CalendarError> {
        Ok(session::Slot {
            recurrence: Recurrence::new(
                self.anchor_date,
                self.interval,
                self.season_start,
                self.season_end,
            )?,
            time_from: self.time_from,
            min_players: self.min_players,
            places: self.places,
            deadline: DeadlineRule::new(self.deadline_days_before, self.deadline_time)?,
        })
    }

    /// `interval_weeks`: 1 or 2.
    pub(crate) fn interval_weeks(&self) -> i64 {
        self.interval.days() / 7
    }
}

/// A `members` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Member {
    pub(crate) id: MemberId,
    pub(crate) slot_id: SlotId,
    /// `None`: a name nobody has claimed (yet).
    pub(crate) player_id: Option<PlayerId>,
    /// `None` once the member left: shown as "Ancien joueur".
    pub(crate) name: Option<String>,
    pub(crate) role: Role,
    pub(crate) left: bool,
}

impl Member {
    pub(crate) fn is_organizer(&self) -> bool {
        self.role == Role::Organizer
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::{date, time};

    use super::{Court, InvalidSetting, Role, Slot};
    use crate::domain::calendar::recurrence::Interval;
    use crate::domain::ids::SlotId;
    use crate::domain::invite::InviteCode;

    fn slot() -> Slot {
        Slot {
            id: SlotId::parse("017f22e2-79b0-7cc3-98c4-dc0c0c07398f").expect("should be an id"),
            name: "  Mercredi  ".to_owned(),
            venue: "Trinquet".to_owned(),
            court: Court::Trinquet,
            specialty: "gomme_creuse".to_owned(),
            interval: Interval::Weekly,
            anchor_date: date(2026, 9, 2),
            time_from: time(18, 0, 0, 0),
            time_to: Some(time(19, 30, 0, 0)),
            min_players: 4,
            places: 8,
            target: 40,
            deadline_days_before: 1,
            deadline_time: time(20, 0, 0, 0),
            season_start: date(2026, 9, 1),
            season_end: date(2027, 6, 30),
            invite_code: InviteCode::parse("ABCD2345").expect("should be a code"),
            list_closed: false,
        }
    }

    fn field(mut slot: Slot) -> Option<&'static str> {
        slot.validate()
            .err()
            .map(|InvalidSetting { field, .. }| field)
    }

    #[test]
    fn a_valid_slot_passes_and_its_text_is_trimmed() {
        let mut slot = slot();
        assert_eq!(slot.validate(), Ok(()), "the fixture is valid");
        assert_eq!(slot.name, "Mercredi", "the name is trimmed");
        assert_eq!(slot.interval_weeks(), 1, "weekly");
    }

    #[test]
    fn each_rule_names_its_field() {
        for (change, expected) in [
            (
                Box::new(|s: &mut Slot| s.name = " ".to_owned()) as Box<dyn Fn(&mut Slot)>,
                "name",
            ),
            (Box::new(|s: &mut Slot| s.venue = "x".repeat(81)), "venue"),
            (
                Box::new(|s: &mut Slot| s.specialty = "Gomme".to_owned()),
                "specialty",
            ),
            (Box::new(|s: &mut Slot| s.min_players = 0), "min_players"),
            (Box::new(|s: &mut Slot| s.places = 3), "places"),
            (Box::new(|s: &mut Slot| s.places = 31), "places"),
            (Box::new(|s: &mut Slot| s.target = 50), "target"),
            (
                Box::new(|s: &mut Slot| s.time_to = Some(time(17, 0, 0, 0))),
                "time_to",
            ),
            (
                Box::new(|s: &mut Slot| s.season_end = date(2026, 8, 1)),
                "season_end",
            ),
            (
                Box::new(|s: &mut Slot| s.season_end = date(2027, 9, 3)),
                "season_end",
            ),
            (
                Box::new(|s: &mut Slot| s.anchor_date = date(2026, 8, 26)),
                "anchor_date",
            ),
            (
                Box::new(|s: &mut Slot| s.deadline_days_before = 7),
                "deadline_days_before",
            ),
        ] {
            let mut slot = slot();
            change(&mut slot);
            assert_eq!(field(slot), Some(expected), "{expected} is rejected");
        }
    }

    #[test]
    fn courts_and_roles_round_trip_through_their_stored_values() {
        for court in [Court::Trinquet, Court::MurAGauche, Court::PlaceLibre] {
            assert_eq!(court.as_str().parse(), Ok(court), "{court:?}");
        }
        for role in [Role::Organizer, Role::Player] {
            assert_eq!(role.as_str().parse(), Ok(role), "{role:?}");
        }
        assert_eq!("fronton".parse::<Court>(), Err(()), "not a court");
    }
}
