// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! The `Idempotency-Key` header of `POST /slots` and `POST …/recovery`.
//!
//! A retry with the same key returns what the first request created instead of creating it
//! again; the same key with another route, caller or body answers 422. The key is a secret, like
//! a token: only its SHA-256 is stored, for 24 hours, and the stored row points to what was
//! created, never to the response, which carried a token.

use jiff::{SignedDuration, Timestamp};
use worker::{D1Database, D1PreparedStatement, Request};

use super::problem::{ApiResult, Problem, ProblemType};
use crate::db::idempotency_keys::{self, NewKey};
use crate::domain::ids::PlayerId;
use crate::domain::token::{IdempotencyKey, sha256};

/// How long a key is kept, and replayed.
const KEY_TTL: SignedDuration = SignedDuration::from_hours(24);

/// A request that sent an `Idempotency-Key`. Every field is a SHA-256: the key itself is not
/// kept.
pub(crate) struct Idempotent {
    /// The key, as stored in `idempotency_keys.key`.
    key: String,
    /// Method and path, caller and body.
    request: [u8; 32],
    /// The same request without a token, when it came with one.
    anonymous: Option<[u8; 32]>,
}

/// The hash of a request: method and path, caller and body.
fn request_hash(req: &Request, caller: Option<&PlayerId>, body: &serde_json::Value) -> [u8; 32] {
    // serde_json::Value keeps object keys sorted, so the body hashes the same whatever the
    // order of its fields.
    let request = format!(
        "{} {}\n{}\n{body}",
        req.method(),
        req.path(),
        caller.map_or("-", PlayerId::as_str)
    );
    sha256(request.as_bytes())
}

/// The key of a request and the hash of what makes the request: route and path, caller and
/// body. `None` without the header, 422 when it is not a UUID v4.
pub(crate) fn read(
    req: &Request,
    caller: Option<&PlayerId>,
    body: &serde_json::Value,
) -> ApiResult<Option<Idempotent>> {
    let Some(header) = req.headers().get("Idempotency-Key")? else {
        return Ok(None);
    };
    let key = IdempotencyKey::parse(header.trim())
        .ok_or_else(|| Problem::invalid("Idempotency-Key must be a UUID v4"))?;
    Ok(Some(Idempotent {
        key: key.hash(),
        request: request_hash(req, caller, body),
        anonymous: caller.map(|_| request_hash(req, None, body)),
    }))
}

impl Idempotent {
    /// What the first request with this key created, if it was in the last 24 hours. 422 when
    /// that request was another one.
    pub(crate) async fn replay(
        &self,
        db: &D1Database,
        now: Timestamp,
    ) -> ApiResult<Option<String>> {
        let Some(stored) =
            idempotency_keys::find(db, &self.key, &self.request, now - KEY_TTL).await?
        else {
            return Ok(None);
        };
        if stored.same_request {
            Ok(Some(stored.result_ref))
        } else {
            Err(Problem::new(ProblemType::IdempotencyKeyReused))
        }
    }

    /// What the first request with this key created, when that request came without a token
    /// and this one with a token: the retry of a first request whose response set the cookie
    /// of a new player. The caller must check that it is that player.
    pub(crate) async fn replay_anonymous(
        &self,
        db: &D1Database,
        now: Timestamp,
    ) -> ApiResult<Option<String>> {
        let Some(anonymous) = &self.anonymous else {
            return Ok(None);
        };
        Ok(
            idempotency_keys::find(db, &self.key, anonymous, now - KEY_TTL)
                .await?
                .filter(|stored| stored.same_request)
                .map(|stored| stored.result_ref),
        )
    }

    /// The statements that record the key, for the batch that creates `result_ref`.
    pub(crate) fn record(
        &self,
        db: &D1Database,
        route: &str,
        result_ref: &str,
        now: Timestamp,
    ) -> worker::Result<[D1PreparedStatement; 2]> {
        idempotency_keys::insert(
            db,
            &NewKey {
                key_hash: &self.key,
                route,
                request_hash: &self.request,
                result_ref,
            },
            now - KEY_TTL,
            now,
        )
    }

    /// The statement that points the key to a new result.
    pub(crate) fn repoint(
        &self,
        db: &D1Database,
        result_ref: &str,
    ) -> worker::Result<D1PreparedStatement> {
        idempotency_keys::set_result_ref(db, &self.key, result_ref)
    }
}
