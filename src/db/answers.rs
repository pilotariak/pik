// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! `answers`: OK, Dispo or KO of a member for one session.

use jiff::Timestamp;
use jiff::civil::Date;
use serde::Deserialize;
use worker::wasm_bindgen::JsValue;
use worker::{D1Database, D1PreparedStatement, Result};

use super::{corrupt, date, instant, read_date, read_instant, statement, text};
use crate::domain::calendar::session::Status;
use crate::domain::ids::{MemberId, SlotId};

/// An `answers` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredAnswer {
    pub(crate) date: Date,
    pub(crate) member_id: MemberId,
    pub(crate) status: Status,
    /// Changes only when the status changes: the call order.
    pub(crate) answered_at: Timestamp,
}

#[derive(Deserialize)]
struct Row {
    date: String,
    member_id: MemberId,
    status: String,
    answered_at: i64,
}

impl TryFrom<Row> for StoredAnswer {
    type Error = worker::Error;

    fn try_from(row: Row) -> Result<Self> {
        Ok(Self {
            date: read_date("answers.date", &row.date)?,
            member_id: row.member_id,
            status: row.status.parse().map_err(|()| corrupt("answers.status"))?,
            answered_at: read_instant("answers.answered_at", row.answered_at)?,
        })
    }
}

/// The answers of a slot from `from` to `to`, both included.
pub(crate) async fn in_range(
    db: &D1Database,
    slot: &SlotId,
    from: Date,
    to: Date,
) -> Result<Vec<StoredAnswer>> {
    statement(
        db,
        "SELECT date, member_id, status, answered_at FROM answers \
         WHERE slot_id = ? AND date BETWEEN ? AND ? ORDER BY date, answered_at, member_id",
        &[text(slot.as_str()), date(from), date(to)],
    )?
    .all()
    .await?
    .results::<Row>()?
    .into_iter()
    .map(StoredAnswer::try_from)
    .collect()
}

/// An answer to write.
pub(crate) struct NewAnswer<'a> {
    pub(crate) slot: &'a SlotId,
    pub(crate) date: Date,
    pub(crate) member: &'a MemberId,
    pub(crate) status: Status,
    /// The organizer who answered for the member, `None` when the member answered.
    pub(crate) set_by: Option<&'a MemberId>,
}

/// Writes an answer, last write wins. `answered_at` moves only when the status changes, so that
/// sending the same answer again keeps the member's place in the call order.
pub(crate) async fn upsert(db: &D1Database, answer: &NewAnswer<'_>, now: Timestamp) -> Result<()> {
    statement(
        db,
        "INSERT INTO answers (slot_id, date, member_id, status, answered_at, set_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT (slot_id, date, member_id) DO UPDATE SET \
         answered_at = CASE WHEN answers.status = excluded.status THEN answers.answered_at \
         ELSE excluded.answered_at END, \
         status = excluded.status, set_by = excluded.set_by",
        &[
            text(answer.slot.as_str()),
            date(answer.date),
            text(answer.member.as_str()),
            text(answer.status.as_str()),
            instant(now),
            answer
                .set_by
                .map_or(JsValue::NULL, |member| text(member.as_str())),
        ],
    )?
    .run()
    .await?;
    Ok(())
}

/// Deletes the answers of a member who left, from `from` on: they no longer count toward the
/// sessions to come. Past answers stay, as the sessions played do. Runs after
/// [`members::leave`](super::members::leave) in the same batch, and only if it applied.
pub(crate) fn delete_after_leaving(
    db: &D1Database,
    member: &MemberId,
    from: Date,
) -> Result<D1PreparedStatement> {
    statement(
        db,
        "DELETE FROM answers WHERE member_id = ?1 AND date >= ?2 \
         AND EXISTS (SELECT 1 FROM members WHERE id = ?1 AND left_at IS NOT NULL)",
        &[text(member.as_str()), date(from)],
    )
}
