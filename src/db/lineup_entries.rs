// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! `lineup_entries`: who is selected for a session, and in which game and team.

use jiff::civil::Date;
use serde::Deserialize;
use worker::wasm_bindgen::JsValue;
use worker::{D1Database, D1PreparedStatement, Result};

use super::{corrupt, date, int, opt_int, read_date, statement, text};
use crate::domain::ids::{GuestId, MemberId, SlotId};
use crate::domain::lineup::{Entry, Seat, Team};

#[derive(Deserialize)]
struct Row {
    position: i64,
    member_id: Option<MemberId>,
    guest_id: Option<GuestId>,
    game: Option<i64>,
    team: Option<String>,
}

impl TryFrom<Row> for Entry {
    type Error = worker::Error;

    fn try_from(row: Row) -> Result<Self> {
        let seat = match (row.member_id, row.guest_id) {
            (Some(member), None) => Seat::Member(member),
            (None, Some(guest)) => Seat::Guest(guest),
            _ => return Err(corrupt("lineup_entries.member_id")),
        };
        let game = match (row.game, row.team.as_deref()) {
            (None, None) => None,
            (Some(game), Some("a")) => Some((game, Team::A)),
            (Some(game), Some("b")) => Some((game, Team::B)),
            _ => return Err(corrupt("lineup_entries.team")),
        };
        Ok(Self {
            position: row.position,
            seat,
            game,
        })
    }
}

/// The entries of a session's lineup, by position.
pub(crate) async fn of_session(db: &D1Database, slot: &SlotId, day: Date) -> Result<Vec<Entry>> {
    statement(
        db,
        "SELECT position, member_id, guest_id, game, team FROM lineup_entries \
         WHERE slot_id = ? AND date = ? ORDER BY position",
        &[text(slot.as_str()), date(day)],
    )?
    .all()
    .await?
    .results::<Row>()?
    .into_iter()
    .map(Entry::try_from)
    .collect()
}

/// A member's entry on one date, for the sessions played.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Played {
    pub(crate) date: Date,
    pub(crate) member_id: MemberId,
    pub(crate) game: Option<i64>,
}

#[derive(Deserialize)]
struct PlayedRow {
    date: String,
    member_id: MemberId,
    game: Option<i64>,
}

/// The member entries of every lineup of a slot before `before`: guests do not count.
pub(crate) async fn played_before(
    db: &D1Database,
    slot: &SlotId,
    before: Date,
) -> Result<Vec<Played>> {
    statement(
        db,
        "SELECT date, member_id, game FROM lineup_entries \
         WHERE slot_id = ? AND date < ? AND member_id IS NOT NULL",
        &[text(slot.as_str()), date(before)],
    )?
    .all()
    .await?
    .results::<PlayedRow>()?
    .into_iter()
    .map(|row| {
        Ok(Played {
            date: read_date("lineup_entries.date", &row.date)?,
            member_id: row.member_id,
            game: row.game,
        })
    })
    .collect()
}

/// Deletes the entries of a session's lineup, before writing the new ones in the same batch.
pub(crate) fn delete_session(
    db: &D1Database,
    slot: &SlotId,
    day: Date,
) -> Result<D1PreparedStatement> {
    statement(
        db,
        "DELETE FROM lineup_entries WHERE slot_id = ? AND date = ?",
        &[text(slot.as_str()), date(day)],
    )
}

/// Inserts an entry. Fails on the foreign key if the lineup header is missing.
pub(crate) fn insert(
    db: &D1Database,
    slot: &SlotId,
    day: Date,
    entry: &Entry,
) -> Result<D1PreparedStatement> {
    let (member, guest) = match &entry.seat {
        Seat::Member(member) => (text(member.as_str()), JsValue::NULL),
        Seat::Guest(guest) => (JsValue::NULL, text(guest.as_str())),
    };
    statement(
        db,
        "INSERT INTO lineup_entries (slot_id, date, position, member_id, guest_id, game, team) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        &[
            text(slot.as_str()),
            date(day),
            int(entry.position),
            member,
            guest,
            opt_int(entry.game.map(|(game, _)| game)),
            entry
                .game
                .map_or(JsValue::NULL, |(_, team)| text(team.as_str())),
        ],
    )
}
