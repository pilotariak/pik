// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Recovery links: `POST /slots/:id/members/:member/recovery` and `POST /recover/:token`.
//!
//! An organizer creates a link for a member who lost their device. It works once, for 24
//! hours, and links that member, with their answers, to the player of the device that redeems
//! it. Only the SHA-256 of its token is stored.

use jiff::{SignedDuration, Timestamp};
use serde::Serialize;
use worker::{D1Database, Request, Response, RouteContext};

use super::auth::{self, UnknownToken};
use super::idempotency::{self, Idempotent};
use super::join::{Joiner, joined};
use super::members::member_conflict;
use super::problem::{ApiResult, Problem, ProblemType};
use super::rate_limit::{self, Limit};
use super::{Caller, database, json, now, param, slot_caller};
use crate::db;
use crate::domain::ids::MemberId;
use crate::domain::token::{Token, TokenHash};

/// How long a recovery link works.
const LINK_TTL: SignedDuration = SignedDuration::from_hours(24);

/// Route of the creation in `idempotency_keys`.
const CREATE_ROUTE: &str = "POST /api/v1/slots/:id/members/:member/recovery";

/// A recovery link, to send privately to the member.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct RecoveryLink {
    /// The token of `POST /recover/{token}`. Shown once: pik keeps only its hash.
    token: String,
    #[cfg_attr(test, schema(format = DateTime))]
    expires_at: String,
}

fn created(token: &Token, expires_at: Timestamp) -> ApiResult<Response> {
    json(
        201,
        &RecoveryLink {
            token: token.expose().to_owned(),
            expires_at: expires_at.to_string(),
        },
    )
}

/// Answers a replayed creation with the link the key created and a fresh token: the first one
/// never reached the organizer, and stops working.
async fn replay(db: &D1Database, key: &Idempotent, link_ref: &str) -> ApiResult<Response> {
    let hash = TokenHash::from_hex(link_ref).ok_or_else(|| Problem::new(ProblemType::NotFound))?;
    let link = db::recoveries::find(db, &hash)
        .await?
        .ok_or_else(|| Problem::new(ProblemType::NotFound))?;
    if link.used {
        return Err(Problem::new(ProblemType::RecoveryLinkGone));
    }
    let token = Token::generate()?;
    let fresh = token.hash();
    db::batch(
        db,
        vec![
            db::recoveries::rotate(db, &hash, &fresh)?,
            key.repoint(db, &fresh.to_hex())?,
        ],
    )
    .await?;
    created(&token, link.expires_at)
}

/// Create a recovery link for a member (organizer). Accepts `Idempotency-Key`.
#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/v1/slots/{id}/members/{member}/recovery",
    operation_id = "createRecoveryLink",
    tag = "recovery",
    params(
        ("id" = String, Path, description = "Slot id"),
        ("member" = String, Path, description = "Member id"),
        ("Idempotency-Key" = Option<String>, Header, description = "A UUID v4 drawn by the client: a retry with the same key returns the first link with a fresh token"),
    ),
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 201, description = "The link, valid 24 hours", body = RecoveryLink),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 410, response = super::problem::responses::Gone),
        (status = 422, response = super::problem::responses::Unprocessable),
    )
))]
pub(super) async fn create(req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    auth::organizer(&member)?;
    let target = param(&ctx, "member", MemberId::parse)?;
    match db::members::find(&db, &slot.id, &target).await? {
        Some(found) if !found.left => {}
        _ => return Err(Problem::new(ProblemType::NotFound)),
    }
    let caller = member.player_id.as_ref();
    let idempotent = idempotency::read(&req, caller, &serde_json::Value::Null)?;
    if let Some(key) = &idempotent
        && let Some(link_ref) = key.replay(&db, now).await?
    {
        return replay(&db, key, &link_ref).await;
    }
    let token = Token::generate()?;
    let hash = token.hash();
    let expires_at = now + LINK_TTL;
    let mut statements = vec![db::recoveries::insert(
        &db, &hash, &target, &member.id, expires_at,
    )?];
    if let Some(key) = &idempotent {
        statements.extend(key.record(&db, CREATE_ROUTE, &hash.to_hex(), now)?);
    }
    if let Err(error) = db::batch(&db, statements).await {
        if let Some(key) = &idempotent
            && let Some(link_ref) = key.replay(&db, now).await?
        {
            // A concurrent request with the same key created the link first.
            return replay(&db, key, &link_ref).await;
        }
        return Err(error.into());
    }
    created(&token, expires_at)
}

/// Redeem a recovery link: the member is linked to the device's player, or to a new player whose
/// token is returned, and keeps their answers. Works once, for 24 hours.
#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/v1/recover/{token}",
    operation_id = "redeemRecoveryLink",
    tag = "recovery",
    params(("token" = String, Path, description = "The token of the recovery link")),
    security((), ("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "The member is the device's", body = super::join::Joined,
            headers(("Set-Cookie" = String, description = "The new player token, when one was issued"))),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 409, response = super::problem::responses::Conflict),
        (status = 410, response = super::problem::responses::Gone),
        (status = 429, response = super::problem::responses::TooManyRequests),
    )
))]
pub(super) async fn redeem(req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    rate_limit::check(&ctx.env, &req, Limit::Recover).await?;
    let player = auth::optional_player(&req, &ctx.env, &db, now, UnknownToken::Reject).await?;
    let token = param(&ctx, "token", Token::parse)?;
    let hash = token.hash();
    let link = db::recoveries::find(&db, &hash)
        .await?
        .ok_or_else(|| Problem::new(ProblemType::NotFound))?;
    if !link.is_valid(now) {
        return Err(Problem::new(ProblemType::RecoveryLinkGone));
    }
    let member = db::members::find_any(&db, &link.member_id)
        .await?
        .filter(|member| !member.left)
        .ok_or_else(|| Problem::new(ProblemType::RecoveryLinkGone))?;
    if let Some(player) = &player
        && let Some(current) = db::members::of_player_in_slot(&db, &member.slot_id, player).await?
        && current.id != member.id
    {
        return Err(Problem::new(ProblemType::AlreadyMember));
    }
    let joiner = Joiner::new(player)?;
    let mut statements = vec![db::recoveries::redeem(&db, &hash, now)?];
    statements.extend(joiner.insert(&db, now)?);
    statements.push(db::members::relink(&db, &member.id, &joiner.player, now)?);
    if let Err(error) = db::batch(&db, statements).await {
        // The guard of `redeem` fails the batch when another device used the link meanwhile.
        let still_valid = db::recoveries::find(&db, &hash)
            .await?
            .is_some_and(|link| link.is_valid(now));
        if !still_valid {
            return Err(Problem::new(ProblemType::RecoveryLinkGone));
        }
        return Err(member_conflict(error));
    }
    joined(200, &member.slot_id, &member.id, joiner.token())
}
