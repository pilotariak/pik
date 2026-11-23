// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Members: `POST /slots/:id/members`, `PATCH /slots/:id/members/:member`,
//! `DELETE /slots/:id/members/:member/claim` and `DELETE /slots/:id/members/me`.

use serde::{Deserialize, Serialize};
use worker::{D1Database, Request, Response, RouteContext};

use super::problem::{ApiResult, Problem, ProblemType};
use super::{Caller, auth, database, json, no_content, now, param, read_json, slot_caller};
use crate::db;
use crate::db::members::NewMember;
use crate::domain::calendar::paris_date;
use crate::domain::ids::{MemberId, SlotId};
use crate::domain::name::DisplayName;
use crate::domain::slot::{Member, Role};

/// A member of a slot.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct MemberView {
    id: String,
    name: Option<String>,
    role: Role,
    /// Whether a device holds this name. A free name can be claimed through the invite link.
    claimed: bool,
}

pub(super) fn member_view(member: &Member) -> MemberView {
    MemberView {
        id: member.id.to_string(),
        name: member.name.clone(),
        role: member.role,
        claimed: member.player_id.is_some(),
    }
}

/// A display name, 422 when it breaks a rule.
pub(super) fn display_name(value: &str) -> ApiResult<DisplayName> {
    DisplayName::parse(value).map_err(|error| Problem::invalid(format!("name: {error}")))
}

/// 409 `name-taken` for the UNIQUE index on active names, 409 `already-member` for the one on
/// devices, the error itself otherwise.
pub(super) fn member_conflict(error: worker::Error) -> Problem {
    let message = error.to_string();
    if message.contains("name_key") {
        Problem::new(ProblemType::NameTaken)
    } else if message.contains("player_id") {
        Problem::new(ProblemType::AlreadyMember)
    } else {
        error.into()
    }
}

/// The active member of a slot named in the path: 404 when there is none.
async fn target(db: &D1Database, ctx: &RouteContext<()>, slot: &SlotId) -> ApiResult<Member> {
    let id = param(ctx, "member", MemberId::parse)?;
    match db::members::find(db, slot, &id).await? {
        Some(member) if !member.left => Ok(member),
        _ => Err(Problem::new(ProblemType::NotFound)),
    }
}

/// `POST /slots/:id/members`.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub(super) struct NameRequest {
    /// First name or nickname: 1 to 40 characters, unique in the slot whatever its case.
    pub(super) name: String,
}

/// Add a name to the list (organizer). Nobody holds it until a player claims it.
#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/v1/slots/{id}/members",
    operation_id = "addMember",
    tag = "members",
    params(("id" = String, Path, description = "Slot id")),
    request_body = NameRequest,
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 201, description = "The new member", body = MemberView),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 409, response = super::problem::responses::Conflict),
        (status = 422, response = super::problem::responses::Unprocessable),
    )
))]
pub(super) async fn add(mut req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    auth::organizer(&member)?;
    let (request, _): (NameRequest, _) = read_json(&mut req).await?;
    let name = display_name(&request.name)?;
    let id = MemberId::generate()?;
    db::members::insert(
        &db,
        &NewMember {
            id: &id,
            slot: &slot.id,
            player: None,
            name: &name,
            role: Role::Player,
        },
        now,
    )?
    .run()
    .await
    .map_err(member_conflict)?;
    json(
        201,
        &MemberView {
            id: id.to_string(),
            name: Some(name.text().to_owned()),
            role: Role::Player,
            claimed: false,
        },
    )
}

/// `PATCH /slots/:id/members/:member`: absent fields keep their value.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub(super) struct MemberChanges {
    role: Option<Role>,
    /// Fixes a misspelled name.
    name: Option<String>,
}

/// Change a member's role or display name (organizer). The last organizer of a slot cannot lose
/// the role.
#[cfg_attr(test, utoipa::path(
    patch,
    path = "/api/v1/slots/{id}/members/{member}",
    operation_id = "updateMember",
    tag = "members",
    params(
        ("id" = String, Path, description = "Slot id"),
        ("member" = String, Path, description = "Member id"),
    ),
    request_body = MemberChanges,
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "The member after the change", body = MemberView),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 409, response = super::problem::responses::Conflict),
        (status = 422, response = super::problem::responses::Unprocessable),
    )
))]
pub(super) async fn update(mut req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    auth::organizer(&member)?;
    let target = target(&db, &ctx, &slot.id).await?;
    let (changes, _): (MemberChanges, _) = read_json(&mut req).await?;
    let name = changes.name.as_deref().map(display_name).transpose()?;
    if let Some(name) = &name {
        let taken = db::members::of_slot(&db, &slot.id)
            .await?
            .into_iter()
            .any(|other| {
                other.id != target.id
                    && other
                        .name
                        .as_deref()
                        .and_then(|other| DisplayName::parse(other).ok())
                        .is_some_and(|other| other.key() == name.key())
            });
        if taken {
            return Err(Problem::new(ProblemType::NameTaken));
        }
    }
    if let Some(role) = changes.role
        && role != target.role
        && !db::members::set_role(&db, &slot.id, &target.id, role).await?
    {
        return Err(Problem::new(ProblemType::LastOrganizer));
    }
    if let Some(name) = &name {
        db::members::rename(&db, &slot.id, &target.id, name)
            .await
            .map_err(member_conflict)?;
    }
    let updated = db::members::find(&db, &slot.id, &target.id)
        .await?
        .ok_or_else(|| Problem::new(ProblemType::NotFound))?;
    json(200, &member_view(&updated))
}

/// Release a claimed name (organizer): the device no longer holds it, the member keeps their
/// name and answers, and the name can be claimed again. The last organizer's name cannot be
/// released.
#[cfg_attr(test, utoipa::path(
    delete,
    path = "/api/v1/slots/{id}/members/{member}/claim",
    operation_id = "releaseName",
    tag = "members",
    params(
        ("id" = String, Path, description = "Slot id"),
        ("member" = String, Path, description = "Member id"),
    ),
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 204, description = "The name is free"),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 409, response = super::problem::responses::Conflict),
    )
))]
pub(super) async fn release(req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    auth::organizer(&member)?;
    let target = target(&db, &ctx, &slot.id).await?;
    if target.player_id.is_some() && !db::members::release(&db, &slot.id, &target.id).await? {
        return Err(Problem::new(ProblemType::LastOrganizer));
    }
    no_content()
}

/// Leave the slot. The member's row stays as "Ancien joueur" with its past answers and sessions
/// played; their answers from today on are deleted. The last organizer cannot leave.
#[cfg_attr(test, utoipa::path(
    delete,
    path = "/api/v1/slots/{id}/members/me",
    operation_id = "leaveSlot",
    tag = "members",
    params(("id" = String, Path, description = "Slot id")),
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 204, description = "Left the slot"),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 409, response = super::problem::responses::Conflict),
    )
))]
pub(super) async fn leave(req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    let results = db::batch(
        &db,
        vec![
            db::members::leave(&db, &slot.id, &member.id, now)?,
            db::answers::delete_after_leaving(&db, &member.id, paris_date(now))?,
        ],
    )
    .await?;
    let left = match results.first() {
        Some(result) => db::changes(result)? > 0,
        None => false,
    };
    if !left {
        return Err(Problem::new(ProblemType::LastOrganizer));
    }
    no_content()
}
