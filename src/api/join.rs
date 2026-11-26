// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Joining a slot with its invite code: `GET /join/:code`, `POST /join/:code/claim` and
//! `POST /join/:code/members`.
//!
//! The invite code is only used here. A device without a player, or whose token is unknown
//! because its player was purged (#5), gets a new player and its token.

use serde::{Deserialize, Serialize};
use worker::{D1Database, D1PreparedStatement, Request, Response, RouteContext};

use super::auth::{self, NewPlayer, UnknownToken};
use super::members::{NameRequest, display_name, member_conflict};
use super::problem::{ApiResult, Problem, ProblemType};
use super::rate_limit::{self, Limit};
use super::{database, json, now, param, read_json};
use crate::db;
use crate::db::members::NewMember;
use crate::domain::calendar::paris_date;
use crate::domain::civil::format_time;
use crate::domain::ids::{MemberId, PlayerId, SlotId};
use crate::domain::invite::InviteCode;
use crate::domain::slot::{Court, Role, Slot};
use crate::domain::token::Token;

/// The slot of the invite code in the path: 404 when no slot has it.
async fn slot_of_code(db: &D1Database, ctx: &RouteContext<()>) -> ApiResult<Slot> {
    let code = param(ctx, "code", InviteCode::parse)?;
    db::slots::find_by_invite(db, &code)
        .await?
        .ok_or_else(|| Problem::new(ProblemType::NotFound))
}

/// A name nobody holds yet.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct FreeName {
    member_id: String,
    name: String,
}

/// What a player sees before joining.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct JoinPreview {
    name: String,
    venue: String,
    court: Court,
    specialty: String,
    /// ISO weekday: 1 for Monday to 7 for Sunday.
    weekday: i8,
    time_from: String,
    interval_weeks: i64,
    /// The next session from today in Paris, `null` once the season is over.
    #[cfg_attr(test, schema(format = Date))]
    next_session: Option<String>,
    /// A closed list takes no new name: only the free names can be claimed.
    list_closed: bool,
    free_names: Vec<FreeName>,
}

/// Preview a slot from its invite code, with the names nobody holds yet. Needs no token.
#[cfg_attr(test, utoipa::path(
    get,
    path = "/api/v1/join/{code}",
    operation_id = "previewJoin",
    tag = "join",
    params(("code" = String, Path, description = "Invite code: eight characters without 0, O, 1 or I, in any case")),
    security(()),
    responses(
        (status = 200, description = "The slot and its free names", body = JoinPreview),
        (status = 404, response = super::problem::responses::NotFound),
    )
))]
pub(super) async fn preview(_req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let slot = slot_of_code(&db, &ctx).await?;
    let calendar = super::sessions::slot_calendar(&slot)?;
    let free_names = db::members::of_slot(&db, &slot.id)
        .await?
        .into_iter()
        .filter(|member| !member.left && member.player_id.is_none())
        .filter_map(|member| {
            member.name.map(|name| FreeName {
                member_id: member.id.to_string(),
                name,
            })
        })
        .collect();
    json(
        200,
        &JoinPreview {
            name: slot.name.clone(),
            venue: slot.venue.clone(),
            court: slot.court,
            specialty: slot.specialty.clone(),
            weekday: calendar.recurrence.weekday().to_monday_one_offset(),
            time_from: format_time(slot.time_from),
            interval_weeks: slot.interval_weeks(),
            next_session: calendar
                .recurrence
                .next_on_or_after(paris_date(now))
                .map(|date| date.to_string()),
            list_closed: slot.list_closed,
            free_names,
        },
    )
}

/// A slot joined: the member of the device, and its token when one was issued.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct Joined {
    #[cfg_attr(test, schema(format = "uuid"))]
    slot_id: String,
    member_id: String,
    /// A new player token when the device had none, or an unknown one; also set in the cookie.
    /// `null` when the device already had a player.
    player_token: Option<String>,
}

/// The response of a join: the cookie of a new token goes with it.
pub(super) fn joined(
    status: u16,
    slot: &SlotId,
    member: &MemberId,
    token: Option<&Token>,
) -> ApiResult<Response> {
    let mut response = json(
        status,
        &Joined {
            slot_id: slot.to_string(),
            member_id: member.to_string(),
            player_token: token.map(|token| token.expose().to_owned()),
        },
    )?;
    if let Some(token) = token {
        auth::set_token_cookie(&mut response, token)?;
    }
    Ok(response)
}

/// The device's player, or a new one with the statement that inserts it.
pub(super) struct Joiner {
    pub(super) player: PlayerId,
    pub(super) new: Option<NewPlayer>,
}

impl Joiner {
    pub(super) fn new(existing: Option<PlayerId>) -> ApiResult<Self> {
        if let Some(player) = existing {
            return Ok(Self { player, new: None });
        }
        let new = NewPlayer::generate()?;
        Ok(Self {
            player: new.id.clone(),
            new: Some(new),
        })
    }

    /// The statement that inserts the new player, first in the batch.
    pub(super) fn insert(
        &self,
        db: &D1Database,
        now: jiff::Timestamp,
    ) -> ApiResult<Vec<D1PreparedStatement>> {
        let mut statements = Vec::new();
        if let Some(new) = &self.new {
            statements.push(db::players::insert(db, &new.id, &new.token.hash(), now)?);
        }
        Ok(statements)
    }

    pub(super) fn token(&self) -> Option<&Token> {
        self.new.as_ref().map(|new| &new.token)
    }
}

/// 409 when the device is already a member of the slot.
async fn not_yet_member(
    db: &D1Database,
    slot: &SlotId,
    player: Option<&PlayerId>,
) -> ApiResult<()> {
    if let Some(player) = player
        && db::members::of_player_in_slot(db, slot, player)
            .await?
            .is_some()
    {
        return Err(Problem::new(ProblemType::AlreadyMember));
    }
    Ok(())
}

/// `POST /join/:code/claim`.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub(super) struct ClaimRequest {
    /// A free name of the preview.
    member_id: String,
}

/// "C'est moi": claim a free name. Returns a player token if the device has none.
#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/v1/join/{code}/claim",
    operation_id = "claimName",
    tag = "join",
    params(("code" = String, Path, description = "Invite code")),
    request_body = ClaimRequest,
    security((), ("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "The name is the device's", body = Joined,
            headers(("Set-Cookie" = String, description = "The new player token, when one was issued"))),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 409, response = super::problem::responses::Conflict),
        (status = 422, response = super::problem::responses::Unprocessable),
        (status = 429, response = super::problem::responses::TooManyRequests),
    )
))]
pub(super) async fn claim(mut req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    rate_limit::check(&ctx.env, &req, Limit::Claim).await?;
    let player = auth::optional_player(&req, &ctx.env, &db, now, UnknownToken::AsNone).await?;
    let slot = slot_of_code(&db, &ctx).await?;
    let (request, _): (ClaimRequest, _) = read_json(&mut req).await?;
    let member = MemberId::parse(&request.member_id)
        .ok_or_else(|| Problem::invalid("member_id is not a name of the slot"))?;
    let target = match db::members::find(&db, &slot.id, &member).await? {
        Some(found) if !found.left => found,
        _ => return Err(Problem::invalid("member_id is not a name of the slot")),
    };
    if target.player_id.is_some() {
        return Err(Problem::new(ProblemType::NameTaken));
    }
    not_yet_member(&db, &slot.id, player.as_ref()).await?;
    let joiner = Joiner::new(player)?;
    let mut statements = joiner.insert(&db, now)?;
    statements.push(db::members::claim(
        &db,
        &slot.id,
        &member,
        &joiner.player,
        now,
    )?);
    let results = db::batch(&db, statements).await.map_err(member_conflict)?;
    let claimed = match results.last() {
        Some(result) => db::changes(result)? > 0,
        None => false,
    };
    if !claimed {
        // Claimed by another device meanwhile. The new player, if any, stays unused: it holds
        // no data and the purge (#5) removes it.
        return Err(Problem::new(ProblemType::NameTaken));
    }
    joined(200, &slot.id, &member, joiner.token())
}

/// "Not in the list": add the device's name to the slot, unless the list is closed. Returns a
/// player token if the device has none.
#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/v1/join/{code}/members",
    operation_id = "joinWithName",
    tag = "join",
    params(("code" = String, Path, description = "Invite code")),
    request_body = NameRequest,
    security((), ("bearer" = []), ("cookie" = [])),
    responses(
        (status = 201, description = "The device's new member", body = Joined,
            headers(("Set-Cookie" = String, description = "The new player token, when one was issued"))),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 409, response = super::problem::responses::Conflict),
        (status = 422, response = super::problem::responses::Unprocessable),
        (status = 429, response = super::problem::responses::TooManyRequests),
    )
))]
pub(super) async fn add_name(mut req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    rate_limit::check(&ctx.env, &req, Limit::Claim).await?;
    let player = auth::optional_player(&req, &ctx.env, &db, now, UnknownToken::AsNone).await?;
    let slot = slot_of_code(&db, &ctx).await?;
    let (request, _): (NameRequest, _) = read_json(&mut req).await?;
    let name = display_name(&request.name)?;
    if slot.list_closed {
        return Err(Problem::new(ProblemType::ListClosed));
    }
    not_yet_member(&db, &slot.id, player.as_ref()).await?;
    let joiner = Joiner::new(player)?;
    let member = MemberId::generate()?;
    let mut statements = joiner.insert(&db, now)?;
    statements.push(db::members::insert(
        &db,
        &NewMember {
            id: &member,
            slot: &slot.id,
            player: Some(&joiner.player),
            name: &name,
            role: Role::Player,
        },
        now,
    )?);
    db::batch(&db, statements).await.map_err(member_conflict)?;
    joined(201, &slot.id, &member, joiner.token())
}
