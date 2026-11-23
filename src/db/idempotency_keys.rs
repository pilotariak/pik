// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! `idempotency_keys`: what a request with an `Idempotency-Key` created.
//!
//! `key` holds the SHA-256 of the key, never the key, and `result_ref` the id of what was
//! created, never the response: the response carried a token.

use jiff::Timestamp;
use serde::Deserialize;
use worker::{D1Database, D1PreparedStatement, Result};

use super::{blob, instant, statement, text};

/// A key used within the last 24 hours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredKey {
    pub(crate) result_ref: String,
    /// Whether the request matches the first one: same route, caller and body.
    pub(crate) same_request: bool,
}

#[derive(Deserialize)]
struct Row {
    result_ref: String,
    same_request: i64,
}

/// The key with this hash, if used after `not_before`.
pub(crate) async fn find(
    db: &D1Database,
    key_hash: &str,
    request_hash: &[u8; 32],
    not_before: Timestamp,
) -> Result<Option<StoredKey>> {
    Ok(statement(
        db,
        "SELECT result_ref, request_hash = ? AS same_request FROM idempotency_keys \
         WHERE key = ? AND created_at > ?",
        &[blob(request_hash), text(key_hash), instant(not_before)],
    )?
    .first::<Row>(None)
    .await?
    .map(|row| StoredKey {
        result_ref: row.result_ref,
        same_request: row.same_request != 0,
    }))
}

/// A key to record with what its request created.
pub(crate) struct NewKey<'a> {
    pub(crate) key_hash: &'a str,
    pub(crate) route: &'a str,
    pub(crate) request_hash: &'a [u8; 32],
    pub(crate) result_ref: &'a str,
}

/// Records a key, in the batch that creates its result. A key older than `not_before` is
/// replaced; a key in use fails on the primary key, and the batch with it, so that two
/// concurrent first requests do not both create.
pub(crate) fn insert(
    db: &D1Database,
    key: &NewKey<'_>,
    not_before: Timestamp,
    now: Timestamp,
) -> Result<[D1PreparedStatement; 2]> {
    Ok([
        statement(
            db,
            "DELETE FROM idempotency_keys WHERE key = ? AND created_at <= ?",
            &[text(key.key_hash), instant(not_before)],
        )?,
        statement(
            db,
            "INSERT INTO idempotency_keys (key, route, request_hash, result_ref, created_at) \
             VALUES (?, ?, ?, ?, ?)",
            &[
                text(key.key_hash),
                text(key.route),
                blob(key.request_hash),
                text(key.result_ref),
                instant(now),
            ],
        )?,
    ])
}

/// Points a key to a new result: a replayed recovery link gets a new token, so a new hash.
pub(crate) fn set_result_ref(
    db: &D1Database,
    key_hash: &str,
    result_ref: &str,
) -> Result<D1PreparedStatement> {
    statement(
        db,
        "UPDATE idempotency_keys SET result_ref = ? WHERE key = ?",
        &[text(result_ref), text(key_hash)],
    )
}
