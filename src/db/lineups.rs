// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! `lineups`: the header of a session's lineup. Its `version` is the `ETag` of the session, and
//! a session without a lineup has version 0.

use jiff::Timestamp;
use jiff::civil::Date;
use serde::Deserialize;
use worker::{D1Database, D1PreparedStatement, Result};

use super::{date, instant, int, statement, text};
use crate::domain::ids::{GuestId, MemberId, SlotId};

#[derive(Deserialize)]
struct Row {
    version: i64,
}

/// The lineup version of a session, 0 without a lineup.
pub(crate) async fn version(db: &D1Database, slot: &SlotId, day: Date) -> Result<i64> {
    Ok(statement(
        db,
        "SELECT version FROM lineups WHERE slot_id = ? AND date = ?",
        &[text(slot.as_str()), date(day)],
    )?
    .first::<Row>(None)
    .await?
    .map_or(0, |row| row.version))
}

/// Moves the lineup of a session from version `expected` to the next one, as the first
/// statement of the batch that writes its entries.
///
/// When the version is no longer `expected`, the statement fails and the whole batch with it:
/// from version 0, the INSERT hits the primary key of a lineup written meanwhile; from a later
/// version, `updated_at` gets NULL and breaks its NOT NULL constraint. D1 runs a batch as one
/// transaction, so no write can slip between this check and the entries.
pub(crate) fn advance(
    db: &D1Database,
    slot: &SlotId,
    day: Date,
    expected: i64,
    by: &MemberId,
    now: Timestamp,
) -> Result<D1PreparedStatement> {
    if expected == 0 {
        statement(
            db,
            "INSERT INTO lineups (slot_id, date, version, updated_at, updated_by) \
             VALUES (?, ?, 1, ?, ?)",
            &[
                text(slot.as_str()),
                date(day),
                instant(now),
                text(by.as_str()),
            ],
        )
    } else {
        statement(
            db,
            "UPDATE lineups SET version = version + 1, \
             updated_at = CASE WHEN version = ?3 THEN ?4 END, updated_by = ?5 \
             WHERE slot_id = ?1 AND date = ?2",
            &[
                text(slot.as_str()),
                date(day),
                int(expected),
                instant(now),
                text(by.as_str()),
            ],
        )
    }
}

/// Moves the version of a session's lineup when one of `removed` guests is in it: removing a
/// guest deletes their entry (ON DELETE CASCADE), and a lineup that changes gets a new `ETag`.
/// Runs before the guests are deleted, in the same batch.
pub(crate) fn bump_if_guests_in_it(
    db: &D1Database,
    slot: &SlotId,
    day: Date,
    removed: &[GuestId],
    now: Timestamp,
) -> Result<D1PreparedStatement> {
    let ids = serde_json::to_string(removed)?;
    statement(
        db,
        "UPDATE lineups SET version = version + 1, updated_at = ?3 \
         WHERE slot_id = ?1 AND date = ?2 AND EXISTS (SELECT 1 FROM lineup_entries \
         WHERE slot_id = ?1 AND date = ?2 AND guest_id IN (SELECT value FROM json_each(?4)))",
        &[text(slot.as_str()), date(day), instant(now), text(&ids)],
    )
}
