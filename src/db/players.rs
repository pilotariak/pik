// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! `players`: one per device (kancha ADR-0002), found by the SHA-256 of its token.

use jiff::{SignedDuration, Timestamp};
use serde::Deserialize;
use worker::{D1Database, D1PreparedStatement, Result};

use super::{blob, instant, statement, text};
use crate::domain::ids::PlayerId;
use crate::domain::token::TokenHash;

/// `last_seen_at` is written at most once an hour per device, not on every request.
const LAST_SEEN_RESOLUTION: SignedDuration = SignedDuration::from_hours(1);

#[derive(Deserialize)]
struct Row {
    id: PlayerId,
}

/// The player whose token has this hash. Refreshes its `last_seen_at`, which the purge (#5)
/// reads.
pub(crate) async fn find_by_token(
    db: &D1Database,
    hash: &TokenHash,
    now: Timestamp,
) -> Result<Option<PlayerId>> {
    let Some(row) = statement(
        db,
        "SELECT id FROM players WHERE token_hash = ?",
        &[blob(hash.as_bytes())],
    )?
    .first::<Row>(None)
    .await?
    else {
        return Ok(None);
    };
    let stale = now - LAST_SEEN_RESOLUTION;
    statement(
        db,
        "UPDATE players SET last_seen_at = ? WHERE id = ? AND last_seen_at < ?",
        &[instant(now), text(row.id.as_str()), instant(stale)],
    )?
    .run()
    .await?;
    Ok(Some(row.id))
}

/// Inserts a new player with the hash of its first token.
pub(crate) fn insert(
    db: &D1Database,
    id: &PlayerId,
    hash: &TokenHash,
    now: Timestamp,
) -> Result<D1PreparedStatement> {
    statement(
        db,
        "INSERT INTO players (id, token_hash, created_at, last_seen_at) VALUES (?, ?, ?, ?)",
        &[
            text(id.as_str()),
            blob(hash.as_bytes()),
            instant(now),
            instant(now),
        ],
    )
}

/// Gives a player a new token: the previous one stops working.
pub(crate) fn rotate_token(
    db: &D1Database,
    id: &PlayerId,
    hash: &TokenHash,
) -> Result<D1PreparedStatement> {
    statement(
        db,
        "UPDATE players SET token_hash = ? WHERE id = ?",
        &[blob(hash.as_bytes()), text(id.as_str())],
    )
}
