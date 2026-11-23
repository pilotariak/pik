// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! `guests`: replacements for one session.

use jiff::civil::Date;
use serde::Deserialize;
use worker::{D1Database, D1PreparedStatement, Result};

use super::{date, statement, text};
use crate::domain::ids::{GuestId, SlotId};

/// A `guests` row.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct Guest {
    pub(crate) id: GuestId,
    #[serde(rename = "display_name")]
    pub(crate) name: String,
}

/// The guests of a session, in the order they were added.
pub(crate) async fn of_session(db: &D1Database, slot: &SlotId, day: Date) -> Result<Vec<Guest>> {
    statement(
        db,
        "SELECT id, display_name FROM guests WHERE slot_id = ? AND date = ? ORDER BY rowid",
        &[text(slot.as_str()), date(day)],
    )?
    .all()
    .await?
    .results()
}

/// Adds a guest to a session.
pub(crate) fn insert(
    db: &D1Database,
    slot: &SlotId,
    day: Date,
    guest: &Guest,
) -> Result<D1PreparedStatement> {
    statement(
        db,
        "INSERT INTO guests (id, slot_id, date, display_name) VALUES (?, ?, ?, ?)",
        &[
            text(guest.id.as_str()),
            text(slot.as_str()),
            date(day),
            text(&guest.name),
        ],
    )
}

/// Removes a guest, and their lineup entry with them (ON DELETE CASCADE).
pub(crate) fn delete(db: &D1Database, slot: &SlotId, id: &GuestId) -> Result<D1PreparedStatement> {
    statement(
        db,
        "DELETE FROM guests WHERE slot_id = ? AND id = ?",
        &[text(slot.as_str()), text(id.as_str())],
    )
}
