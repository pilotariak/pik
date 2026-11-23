// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! `recoveries`: single-use recovery links, found by the SHA-256 of their token.

use jiff::Timestamp;
use serde::Deserialize;
use worker::{D1Database, D1PreparedStatement, Result};

use super::{blob, instant, read_instant, statement, text};
use crate::domain::ids::MemberId;
use crate::domain::token::TokenHash;

/// A `recoveries` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Recovery {
    pub(crate) member_id: MemberId,
    pub(crate) expires_at: Timestamp,
    pub(crate) used: bool,
}

impl Recovery {
    /// Whether the link can still be redeemed at `now`.
    pub(crate) fn is_valid(&self, now: Timestamp) -> bool {
        !self.used && now < self.expires_at
    }
}

#[derive(Deserialize)]
struct Row {
    member_id: MemberId,
    expires_at: i64,
    used_at: Option<i64>,
}

/// The link whose token has this hash.
pub(crate) async fn find(db: &D1Database, hash: &TokenHash) -> Result<Option<Recovery>> {
    statement(
        db,
        "SELECT member_id, expires_at, used_at FROM recoveries WHERE token_hash = ?",
        &[blob(hash.as_bytes())],
    )?
    .first::<Row>(None)
    .await?
    .map(|row| {
        Ok(Recovery {
            member_id: row.member_id,
            expires_at: read_instant("recoveries.expires_at", row.expires_at)?,
            used: row.used_at.is_some(),
        })
    })
    .transpose()
}

/// Inserts a link for a member, created by an organizer.
pub(crate) fn insert(
    db: &D1Database,
    hash: &TokenHash,
    member: &MemberId,
    created_by: &MemberId,
    expires_at: Timestamp,
) -> Result<D1PreparedStatement> {
    statement(
        db,
        "INSERT INTO recoveries (token_hash, member_id, created_by, expires_at) \
         VALUES (?, ?, ?, ?)",
        &[
            blob(hash.as_bytes()),
            text(member.as_str()),
            text(created_by.as_str()),
            instant(expires_at),
        ],
    )
}

/// Marks a link used, as the first statement of the batch that links the member.
///
/// A link already used or expired gets a NULL `expires_at`, which breaks its NOT NULL
/// constraint and fails the whole batch: two devices that redeem the same link at once cannot
/// both succeed.
pub(crate) fn redeem(
    db: &D1Database,
    hash: &TokenHash,
    now: Timestamp,
) -> Result<D1PreparedStatement> {
    statement(
        db,
        "UPDATE recoveries SET used_at = ?2, \
         expires_at = CASE WHEN used_at IS NULL AND expires_at > ?2 THEN expires_at END \
         WHERE token_hash = ?1",
        &[blob(hash.as_bytes()), instant(now)],
    )
}

/// Gives an unused link a new token, for a replayed `Idempotency-Key`: the first token never
/// reached the organizer, and stops working.
pub(crate) fn rotate(
    db: &D1Database,
    old: &TokenHash,
    new: &TokenHash,
) -> Result<D1PreparedStatement> {
    statement(
        db,
        "UPDATE recoveries SET token_hash = ? WHERE token_hash = ? AND used_at IS NULL",
        &[blob(new.as_bytes()), blob(old.as_bytes())],
    )
}
