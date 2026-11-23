// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! `session_overrides`: what differs on one date of a slot.

use jiff::Timestamp;
use jiff::civil::Date;
use serde::Deserialize;
use worker::{D1Database, Result};

use super::{
    date, instant, opt_instant, opt_int, opt_text, read_count, read_date, read_instant, statement,
    text,
};
use crate::domain::calendar::session::Override;
use crate::domain::ids::SlotId;

/// A `session_overrides` row: the calendar's [`Override`] and the organizer's note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionOverride {
    pub(crate) date: Date,
    pub(crate) values: Override,
    pub(crate) note: Option<String>,
}

#[derive(Deserialize)]
struct Row {
    date: String,
    cancelled_at: Option<i64>,
    note: Option<String>,
    min_players: Option<i64>,
    places: Option<i64>,
    deadline_at: Option<i64>,
}

impl TryFrom<Row> for SessionOverride {
    type Error = worker::Error;

    fn try_from(row: Row) -> Result<Self> {
        Ok(Self {
            date: read_date("session_overrides.date", &row.date)?,
            values: Override {
                cancelled: row.cancelled_at.is_some(),
                min_players: row
                    .min_players
                    .map(|value| read_count("session_overrides.min_players", value))
                    .transpose()?,
                places: row
                    .places
                    .map(|value| read_count("session_overrides.places", value))
                    .transpose()?,
                deadline_at: row
                    .deadline_at
                    .map(|value| read_instant("session_overrides.deadline_at", value))
                    .transpose()?,
            },
            note: row.note,
        })
    }
}

const COLUMNS: &str = "date, cancelled_at, note, min_players, places, deadline_at";

/// The overrides of a slot from `from` to `to`, both included, by date.
pub(crate) async fn in_range(
    db: &D1Database,
    slot: &SlotId,
    from: Date,
    to: Date,
) -> Result<Vec<SessionOverride>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM session_overrides WHERE slot_id = ? AND date BETWEEN ? AND ? \
         ORDER BY date"
    );
    statement(db, &sql, &[text(slot.as_str()), date(from), date(to)])?
        .all()
        .await?
        .results::<Row>()?
        .into_iter()
        .map(SessionOverride::try_from)
        .collect()
}

/// Every override of a slot.
pub(crate) async fn of_slot(db: &D1Database, slot: &SlotId) -> Result<Vec<SessionOverride>> {
    let sql = format!("SELECT {COLUMNS} FROM session_overrides WHERE slot_id = ? ORDER BY date");
    statement(db, &sql, &[text(slot.as_str())])?
        .all()
        .await?
        .results::<Row>()?
        .into_iter()
        .map(SessionOverride::try_from)
        .collect()
}

/// Writes the override of a date. `cancelled_at` keeps its first instant while the date stays
/// cancelled, and goes back to `NULL` when the cancellation is undone.
pub(crate) async fn upsert(
    db: &D1Database,
    slot: &SlotId,
    session: &SessionOverride,
    now: Timestamp,
) -> Result<()> {
    statement(
        db,
        "INSERT INTO session_overrides (slot_id, date, cancelled_at, note, min_players, places, \
         deadline_at) VALUES (?1, ?2, CASE WHEN ?3 THEN ?4 END, ?5, ?6, ?7, ?8) \
         ON CONFLICT (slot_id, date) DO UPDATE SET \
         cancelled_at = CASE WHEN ?3 THEN coalesce(session_overrides.cancelled_at, ?4) END, \
         note = ?5, min_players = ?6, places = ?7, deadline_at = ?8",
        &[
            text(slot.as_str()),
            date(session.date),
            super::flag(session.values.cancelled),
            instant(now),
            opt_text(session.note.as_deref()),
            opt_int(session.values.min_players.map(i64::from)),
            opt_int(session.values.places.map(i64::from)),
            opt_instant(session.values.deadline_at),
        ],
    )?
    .run()
    .await?;
    Ok(())
}
