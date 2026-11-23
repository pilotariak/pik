// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! `slots`.

use jiff::Timestamp;
use serde::Deserialize;
use worker::{D1Database, D1PreparedStatement, Result};

use super::{
    corrupt, date, flag, instant, int, opt_text, read_count, read_date, read_time, statement, text,
    time,
};
use crate::domain::calendar::recurrence::Interval;
use crate::domain::civil::format_time;
use crate::domain::ids::SlotId;
use crate::domain::invite::InviteCode;
use crate::domain::slot::Slot;

const COLUMNS: &str = "id, name, venue, court, specialty, interval_weeks, anchor_date, time_from, \
    time_to, min_players, places, target, deadline_days_before, deadline_time, season_start, \
    season_end, invite_code, list_closed";

#[derive(Deserialize)]
struct Row {
    id: SlotId,
    name: String,
    venue: String,
    court: String,
    specialty: String,
    interval_weeks: i64,
    anchor_date: String,
    time_from: String,
    time_to: Option<String>,
    min_players: i64,
    places: i64,
    target: i64,
    deadline_days_before: i64,
    deadline_time: String,
    season_start: String,
    season_end: String,
    invite_code: InviteCode,
    list_closed: i64,
}

impl TryFrom<Row> for Slot {
    type Error = worker::Error;

    fn try_from(row: Row) -> Result<Self> {
        Ok(Self {
            id: row.id,
            name: row.name,
            venue: row.venue,
            court: row.court.parse().map_err(|()| corrupt("slots.court"))?,
            specialty: row.specialty,
            interval: Interval::try_from(row.interval_weeks)
                .map_err(|_| corrupt("slots.interval_weeks"))?,
            anchor_date: read_date("slots.anchor_date", &row.anchor_date)?,
            time_from: read_time("slots.time_from", &row.time_from)?,
            time_to: row
                .time_to
                .map(|value| read_time("slots.time_to", &value))
                .transpose()?,
            min_players: read_count("slots.min_players", row.min_players)?,
            places: read_count("slots.places", row.places)?,
            target: read_count("slots.target", row.target)?,
            deadline_days_before: row.deadline_days_before,
            deadline_time: read_time("slots.deadline_time", &row.deadline_time)?,
            season_start: read_date("slots.season_start", &row.season_start)?,
            season_end: read_date("slots.season_end", &row.season_end)?,
            invite_code: row.invite_code,
            list_closed: row.list_closed != 0,
        })
    }
}

/// The slot with this id.
pub(crate) async fn find(db: &D1Database, id: &SlotId) -> Result<Option<Slot>> {
    let sql = format!("SELECT {COLUMNS} FROM slots WHERE id = ?");
    statement(db, &sql, &[text(id.as_str())])?
        .first::<Row>(None)
        .await?
        .map(Slot::try_from)
        .transpose()
}

/// The slot that has this invite code.
pub(crate) async fn find_by_invite(db: &D1Database, code: &InviteCode) -> Result<Option<Slot>> {
    let sql = format!("SELECT {COLUMNS} FROM slots WHERE invite_code = ?");
    statement(db, &sql, &[text(code.as_str())])?
        .first::<Row>(None)
        .await?
        .map(Slot::try_from)
        .transpose()
}

/// Inserts a new slot. `timezone` keeps its default, Europe/Paris.
pub(crate) fn insert(db: &D1Database, slot: &Slot, now: Timestamp) -> Result<D1PreparedStatement> {
    let sql = format!(
        "INSERT INTO slots ({COLUMNS}, created_at) VALUES ({})",
        ["?"; 19].join(", ")
    );
    statement(
        db,
        &sql,
        &[
            text(slot.id.as_str()),
            text(&slot.name),
            text(&slot.venue),
            text(slot.court.as_str()),
            text(&slot.specialty),
            int(slot.interval_weeks()),
            date(slot.anchor_date),
            time(slot.time_from),
            opt_text(slot.time_to.map(format_time).as_deref()),
            int(slot.min_players.into()),
            int(slot.places.into()),
            int(slot.target.into()),
            int(slot.deadline_days_before),
            time(slot.deadline_time),
            date(slot.season_start),
            date(slot.season_end),
            text(slot.invite_code.as_str()),
            flag(slot.list_closed),
            instant(now),
        ],
    )
}

/// Writes the values that `PATCH /slots/:id` can change. Weekday, frequency and season stay.
pub(crate) async fn update_settings(db: &D1Database, slot: &Slot) -> Result<()> {
    statement(
        db,
        "UPDATE slots SET name = ?, venue = ?, time_from = ?, time_to = ?, min_players = ?, \
         places = ?, target = ?, deadline_days_before = ?, deadline_time = ? WHERE id = ?",
        &[
            text(&slot.name),
            text(&slot.venue),
            time(slot.time_from),
            opt_text(slot.time_to.map(format_time).as_deref()),
            int(slot.min_players.into()),
            int(slot.places.into()),
            int(slot.target.into()),
            int(slot.deadline_days_before),
            time(slot.deadline_time),
            text(slot.id.as_str()),
        ],
    )?
    .run()
    .await?;
    Ok(())
}

/// Replaces the invite code. Fails on the UNIQUE constraint if another slot has it.
pub(crate) async fn set_invite_code(db: &D1Database, id: &SlotId, code: &InviteCode) -> Result<()> {
    statement(
        db,
        "UPDATE slots SET invite_code = ? WHERE id = ?",
        &[text(code.as_str()), text(id.as_str())],
    )?
    .run()
    .await?;
    Ok(())
}

/// Closes or reopens the list of names.
pub(crate) async fn set_list_closed(db: &D1Database, id: &SlotId, closed: bool) -> Result<()> {
    statement(
        db,
        "UPDATE slots SET list_closed = ? WHERE id = ?",
        &[flag(closed), text(id.as_str())],
    )?
    .run()
    .await?;
    Ok(())
}

/// Deletes a slot: every row tied to it goes with it (ON DELETE CASCADE).
pub(crate) async fn delete(db: &D1Database, id: &SlotId) -> Result<()> {
    statement(db, "DELETE FROM slots WHERE id = ?", &[text(id.as_str())])?
        .run()
        .await?;
    Ok(())
}
