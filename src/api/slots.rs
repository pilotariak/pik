// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Slots: `POST /slots`, `GET /me`, `GET|PATCH|DELETE /slots/:id`,
//! `POST /slots/:id/invite-code` and `PUT /slots/:id/list`.
//!
//! `GET /me` lives here: it lists the player's slots, one card each, and the landing page shows
//! it on the slots card.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use worker::{D1Database, Request, Response, RouteContext};

use super::auth::{self, NewPlayer, UnknownToken};
use super::idempotency::{self, Idempotent};
use super::members::{MemberView, member_view};
use super::problem::{ApiResult, Problem, ProblemType};
use super::rate_limit::{self, Limit};
use super::sessions::{SessionSummary, Sessions, slot_calendar, upcoming};
use super::{
    Caller, Change, database, from_value, json, no_content, now, read_json, read_value, slot_caller,
};
use crate::db;
use crate::db::members::NewMember;
use crate::domain::calendar::paris_date;
use crate::domain::calendar::recurrence::Interval;
use crate::domain::civil::{format_time, parse_date, parse_time};
use crate::domain::ids::{MemberId, PlayerId, SlotId};
use crate::domain::invite::InviteCode;
use crate::domain::name::DisplayName;
use crate::domain::slot::{Court, Member, Role, Slot};
use crate::domain::token::Token;

/// Route of `POST /slots` in `idempotency_keys`.
const CREATE_ROUTE: &str = "POST /api/v1/slots";
/// Sessions of `GET /slots/:id`.
const UPCOMING_SESSIONS: usize = 4;
/// Draws of a new invite code before giving up: two slots with one code are a 1 in 2^40 event.
const INVITE_CODE_ATTEMPTS: usize = 3;

/// When a slot meets.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct Schedule {
    /// ISO weekday of every session: 1 for Monday to 7 for Sunday.
    weekday: i8,
    #[cfg_attr(test, schema(example = "18:00"))]
    time_from: String,
    time_to: Option<String>,
    /// 1: every week, 2: every other week.
    interval_weeks: i64,
}

/// Until when players answer: `days_before` days before a session, at `time`, Paris time.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct DeadlineRuleView {
    days_before: i64,
    #[cfg_attr(test, schema(example = "20:00"))]
    time: String,
}

/// The settings of a slot.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct SlotView {
    id: String,
    name: String,
    venue: String,
    court: Court,
    specialty: String,
    schedule: Schedule,
    #[cfg_attr(test, schema(format = Date))]
    anchor_date: String,
    #[cfg_attr(test, schema(format = Date))]
    season_start: String,
    #[cfg_attr(test, schema(format = Date))]
    season_end: String,
    min_players: u32,
    places: u32,
    /// Points of a game: 30, 35 or 40.
    target: u32,
    deadline_rule: DeadlineRuleView,
    /// Eight characters without 0, O, 1 or I, only used to join.
    invite_code: String,
    list_closed: bool,
}

fn schedule(slot: &Slot) -> ApiResult<Schedule> {
    Ok(Schedule {
        weekday: slot_calendar(slot)?
            .recurrence
            .weekday()
            .to_monday_one_offset(),
        time_from: format_time(slot.time_from),
        time_to: slot.time_to.map(format_time),
        interval_weeks: slot.interval_weeks(),
    })
}

fn slot_view(slot: &Slot) -> ApiResult<SlotView> {
    Ok(SlotView {
        id: slot.id.to_string(),
        name: slot.name.clone(),
        venue: slot.venue.clone(),
        court: slot.court,
        specialty: slot.specialty.clone(),
        schedule: schedule(slot)?,
        anchor_date: slot.anchor_date.to_string(),
        season_start: slot.season_start.to_string(),
        season_end: slot.season_end.to_string(),
        min_players: slot.min_players,
        places: slot.places,
        target: slot.target,
        deadline_rule: DeadlineRuleView {
            days_before: slot.deadline_days_before,
            time: format_time(slot.deadline_time),
        },
        invite_code: slot.invite_code.to_string(),
        list_closed: slot.list_closed,
    })
}

/// `POST /slots`.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub(super) struct CreateSlot {
    name: String,
    venue: String,
    court: Court,
    /// `gomme_creuse`, `gomme_pleine`… or a Frontis specialty id: a-z, 0-9, _ and -.
    specialty: String,
    /// The first session, `YYYY-MM-DD`: it gives the weekday, fixed for the season.
    #[cfg_attr(test, schema(format = Date))]
    anchor_date: String,
    /// 1 or 2, fixed for the season.
    interval_weeks: i64,
    /// `HH:MM`, Paris time.
    time_from: String,
    time_to: Option<String>,
    #[cfg_attr(test, schema(format = Date))]
    season_start: String,
    #[cfg_attr(test, schema(format = Date))]
    season_end: String,
    /// Default 4.
    min_players: Option<u32>,
    /// Default: 4, or `min_players` if larger.
    places: Option<u32>,
    /// 30, 35 or 40. Default 40.
    target: Option<u32>,
    /// 0 to 6. Default 1.
    deadline_days_before: Option<i64>,
    /// `HH:MM`. Default 20:00.
    deadline_time: Option<String>,
    /// The creator's name in the slot: they become its first organizer.
    organizer_name: String,
}

/// A slot created, or the one a replayed `Idempotency-Key` created.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct SlotCreated {
    slot_id: String,
    invite_code: String,
    /// The creator's member, an organizer.
    member_id: String,
    /// A new player token when the request came without one; also set in the cookie. `null`
    /// when the device already had a player.
    player_token: Option<String>,
}

fn time_field(field: &str, value: &str) -> ApiResult<jiff::civil::Time> {
    parse_time(value).ok_or_else(|| Problem::invalid(format!("{field} is an HH:MM time")))
}

fn date_field(field: &str, value: &str) -> ApiResult<jiff::civil::Date> {
    parse_date(value).ok_or_else(|| Problem::invalid(format!("{field} is a YYYY-MM-DD date")))
}

fn display_name(field: &str, value: &str) -> ApiResult<DisplayName> {
    DisplayName::parse(value).map_err(|error| Problem::invalid(format!("{field}: {error}")))
}

impl CreateSlot {
    fn slot(&self) -> ApiResult<Slot> {
        let min_players = self.min_players.unwrap_or(4);
        let mut slot = Slot {
            id: SlotId::generate()?,
            name: self.name.clone(),
            venue: self.venue.clone(),
            court: self.court,
            specialty: self.specialty.clone(),
            interval: Interval::try_from(self.interval_weeks)
                .map_err(|_| Problem::invalid("interval_weeks is 1 or 2"))?,
            anchor_date: date_field("anchor_date", &self.anchor_date)?,
            time_from: time_field("time_from", &self.time_from)?,
            time_to: self
                .time_to
                .as_deref()
                .map(|value| time_field("time_to", value))
                .transpose()?,
            min_players,
            places: self.places.unwrap_or(min_players.max(4)),
            target: self.target.unwrap_or(40),
            deadline_days_before: self.deadline_days_before.unwrap_or(1),
            deadline_time: self
                .deadline_time
                .as_deref()
                .map_or(Ok(jiff::civil::Time::constant(20, 0, 0, 0)), |value| {
                    time_field("deadline_time", value)
                })?,
            season_start: date_field("season_start", &self.season_start)?,
            season_end: date_field("season_end", &self.season_end)?,
            invite_code: InviteCode::generate()?,
            list_closed: false,
        };
        slot.validate()
            .map_err(|error| Problem::invalid(error.to_string()))?;
        Ok(slot)
    }
}

/// The response of a creation: 201, `Location`, and the cookie of a new token.
fn created(slot: &Slot, member: &MemberId, token: Option<&Token>) -> ApiResult<Response> {
    let mut response = json(
        201,
        &SlotCreated {
            slot_id: slot.id.to_string(),
            invite_code: slot.invite_code.to_string(),
            member_id: member.to_string(),
            player_token: token.map(|token| token.expose().to_owned()),
        },
    )?;
    response
        .headers_mut()
        .set("Location", &format!("/api/v1/slots/{}", slot.id))?;
    if let Some(token) = token {
        auth::set_token_cookie(&mut response, token)?;
    }
    Ok(response)
}

/// Answers a replayed `POST /slots` with the slot the key created. When that request created a
/// player, its token never reached the client: the replay issues a fresh one, which invalidates
/// the first.
async fn replay_creation(
    db: &D1Database,
    slot_ref: &str,
    caller: Option<&PlayerId>,
) -> ApiResult<Response> {
    let (slot, creator) = created_slot(db, slot_ref).await?;
    let token = match (caller, &creator.player_id) {
        (None, Some(player)) => {
            let token = Token::generate()?;
            db::players::rotate_token(db, player, &token.hash())?
                .run()
                .await?;
            Some(token)
        }
        _ => None,
    };
    created(&slot, &creator.id, token.as_ref())
}

/// The slot a key created and its creator: 404 once the slot was deleted.
async fn created_slot(db: &D1Database, slot_ref: &str) -> ApiResult<(Slot, Member)> {
    let not_found = || Problem::new(ProblemType::NotFound);
    let slot_id = SlotId::parse(slot_ref).ok_or_else(not_found)?;
    let slot = db::slots::find(db, &slot_id).await?.ok_or_else(not_found)?;
    let creator = db::members::creator(db, &slot_id)
        .await?
        .ok_or_else(not_found)?;
    Ok((slot, creator))
}

/// The response to a request whose `Idempotency-Key` was used, `None` for a new key.
async fn replay(
    db: &D1Database,
    key: &Idempotent,
    caller: Option<&PlayerId>,
    now: Timestamp,
) -> ApiResult<Option<Response>> {
    match key.replay(db, now).await {
        Ok(Some(slot_ref)) => replay_creation(db, &slot_ref, caller).await.map(Some),
        Ok(None) => Ok(None),
        Err(reused) => {
            // The first request came without a token, and its response set the cookie of the
            // player it created, which this retry sends back: that player gets the slot, and
            // keeps its token. Any other caller reused the key.
            let Some(caller) = caller else {
                return Err(reused);
            };
            let Some(slot_ref) = key.replay_anonymous(db, now).await? else {
                return Err(reused);
            };
            let (slot, creator) = created_slot(db, &slot_ref).await?;
            if creator.player_id.as_ref() != Some(caller) {
                return Err(reused);
            }
            created(&slot, &creator.id, None).map(Some)
        }
    }
}

/// Create a slot. The creator becomes its first organizer, and a device without a token gets a
/// player token. Accepts `Idempotency-Key`.
#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/v1/slots",
    operation_id = "createSlot",
    tag = "slots",
    request_body = CreateSlot,
    params(("Idempotency-Key" = Option<String>, Header, description = "A UUID v4 drawn by the client: a retry with the same key returns the first slot")),
    security((), ("bearer" = []), ("cookie" = [])),
    responses(
        (status = 201, description = "Slot created, or the slot of a replayed key with a fresh token", body = SlotCreated,
            headers(("Location" = String), ("Set-Cookie" = String, description = "The new player token, when one was issued"))),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 422, response = super::problem::responses::Unprocessable),
        (status = 429, response = super::problem::responses::TooManyRequests),
    )
))]
pub(super) async fn create(mut req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    rate_limit::check(&ctx.env, &req, Limit::CreateSlot).await?;
    let caller = auth::optional_player(&req, &db, now, UnknownToken::Reject).await?;
    let (request, value): (CreateSlot, _) = read_json(&mut req).await?;
    let idempotent = idempotency::read(&req, caller.as_ref(), &value)?;
    if let Some(key) = &idempotent
        && let Some(response) = replay(&db, key, caller.as_ref(), now).await?
    {
        return Ok(response);
    }
    let organizer_name = display_name("organizer_name", &request.organizer_name)?;
    let mut slot = request.slot()?;
    let new_player = match caller {
        Some(_) => None,
        None => Some(NewPlayer::generate()?),
    };
    let player = caller
        .clone()
        .or_else(|| new_player.as_ref().map(|new| new.id.clone()))
        .ok_or_else(|| Problem::new(ProblemType::Internal))?;
    let member = MemberId::generate()?;

    for attempt in 1..=INVITE_CODE_ATTEMPTS {
        let creation = Creation {
            slot: &slot,
            member: &member,
            player: &player,
            organizer_name: &organizer_name,
            new_player: new_player.as_ref(),
            idempotent: idempotent.as_ref(),
        };
        match creation.insert(&db, now).await {
            Ok(()) => {
                return created(&slot, &member, new_player.as_ref().map(|new| &new.token));
            }
            Err(error) => {
                if let Some(key) = &idempotent
                    && let Some(response) = replay(&db, key, caller.as_ref(), now).await?
                {
                    // A concurrent request with the same key created the slot first.
                    return Ok(response);
                }
                if attempt < INVITE_CODE_ATTEMPTS && error.to_string().contains("invite_code") {
                    slot.invite_code = InviteCode::generate()?;
                    continue;
                }
                return Err(error.into());
            }
        }
    }
    Err(Problem::new(ProblemType::Internal))
}

/// The rows of a new slot.
struct Creation<'a> {
    slot: &'a Slot,
    member: &'a MemberId,
    player: &'a PlayerId,
    organizer_name: &'a DisplayName,
    new_player: Option<&'a NewPlayer>,
    idempotent: Option<&'a Idempotent>,
}

impl Creation<'_> {
    /// Inserts the player if new, the slot, its organizer and the idempotency key in one batch.
    async fn insert(&self, db: &D1Database, now: Timestamp) -> worker::Result<()> {
        let mut statements = Vec::new();
        if let Some(new) = self.new_player {
            statements.push(db::players::insert(db, &new.id, &new.token.hash(), now)?);
        }
        statements.push(db::slots::insert(db, self.slot, now)?);
        statements.push(db::members::insert(
            db,
            &NewMember {
                id: self.member,
                slot: &self.slot.id,
                player: Some(self.player),
                name: self.organizer_name,
                role: Role::Organizer,
            },
            now,
        )?);
        if let Some(key) = self.idempotent {
            statements.extend(key.record(db, CREATE_ROUTE, self.slot.id.as_str(), now)?);
        }
        db::batch(db, statements).await?;
        Ok(())
    }
}

/// A slot of the player, on the Créneaux tab.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct SlotCard {
    id: String,
    name: String,
    role: Role,
    member_id: String,
    venue: String,
    court: Court,
    specialty: String,
    schedule: Schedule,
    /// The next session from today in Paris, `null` once the season is over.
    next_session: Option<SessionSummary>,
}

/// `GET /me`.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct MySlots {
    slots: Vec<SlotCard>,
}

/// The player's slots, one card summary each: the Créneaux tab.
#[cfg_attr(test, utoipa::path(
    get,
    path = "/api/v1/me",
    operation_id = "getMySlots",
    tag = "slots",
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "The player's slots, oldest membership first", body = MySlots),
        (status = 401, response = super::problem::responses::Unauthorized),
    )
))]
pub(super) async fn me(req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let player = auth::player(&req, &db, now).await?;
    let today = paris_date(now);
    let mut slots = Vec::new();
    for member in db::members::of_player(&db, &player).await? {
        let Some(slot) = db::slots::find(&db, &member.slot_id).await? else {
            continue;
        };
        let next_session = match slot_calendar(&slot)?.recurrence.next_on_or_after(today) {
            Some(date) => Some(
                Sessions::load(&db, &slot, (date, date), Some(&member.id), now)
                    .await?
                    .summary(date)?,
            ),
            None => None,
        };
        slots.push(SlotCard {
            id: slot.id.to_string(),
            name: slot.name.clone(),
            role: member.role,
            member_id: member.id.to_string(),
            venue: slot.venue.clone(),
            court: slot.court,
            specialty: slot.specialty.clone(),
            schedule: schedule(&slot)?,
            next_session,
        });
    }
    json(200, &MySlots { slots })
}

/// The caller's member in a slot.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct MeInSlot {
    member_id: String,
    role: Role,
}

/// `GET /slots/:id`.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct SlotDetail {
    #[serde(flatten)]
    slot: SlotView,
    me: MeInSlot,
    /// Active members, in the order they were added.
    members: Vec<MemberView>,
    upcoming_sessions: Vec<SessionSummary>,
}

/// A slot with its members and upcoming sessions and their states.
#[cfg_attr(test, utoipa::path(
    get,
    path = "/api/v1/slots/{id}",
    operation_id = "getSlot",
    tag = "slots",
    params(("id" = String, Path, description = "Slot id")),
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "The slot", body = SlotDetail),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 404, response = super::problem::responses::NotFound),
    )
))]
pub(super) async fn get(req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    let members = db::members::of_slot(&db, &slot.id)
        .await?
        .iter()
        .filter(|other| !other.left)
        .map(member_view)
        .collect();
    let upcoming_sessions = upcoming(&db, &slot, UPCOMING_SESSIONS, &member.id, now).await?;
    json(
        200,
        &SlotDetail {
            slot: slot_view(&slot)?,
            me: MeInSlot {
                member_id: member.id.to_string(),
                role: member.role,
            },
            members,
            upcoming_sessions,
        },
    )
}

/// `PATCH /slots/:id`: absent fields keep their value. Weekday and frequency are fixed for the
/// season: `interval_weeks` or `anchor_date` answers 422.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub(super) struct SlotChanges {
    name: Option<String>,
    venue: Option<String>,
    time_from: Option<String>,
    /// `null` removes the end time.
    #[serde(default)]
    #[cfg_attr(test, schema(value_type = Option<String>))]
    time_to: Change<String>,
    min_players: Option<u32>,
    places: Option<u32>,
    target: Option<u32>,
    deadline_days_before: Option<i64>,
    deadline_time: Option<String>,
}

/// Change the name, venue, times, minimum, places, target or deadline rule (organizer).
#[cfg_attr(test, utoipa::path(
    patch,
    path = "/api/v1/slots/{id}",
    operation_id = "updateSlot",
    tag = "slots",
    params(("id" = String, Path, description = "Slot id")),
    request_body = SlotChanges,
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "The slot after the change", body = SlotView),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 422, response = super::problem::responses::Unprocessable),
    )
))]
pub(super) async fn update(mut req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, mut slot } = slot_caller(&req, &ctx, &db, now).await?;
    auth::organizer(&member)?;
    let value = read_value(&mut req).await?;
    for fixed in ["interval_weeks", "anchor_date"] {
        if value.get(fixed).is_some() {
            return Err(Problem::invalid(format!(
                "{fixed} is fixed for the season: create a new slot to change the weekday or the frequency"
            )));
        }
    }
    let changes: SlotChanges = from_value(value)?;
    if let Some(name) = changes.name {
        slot.name = name;
    }
    if let Some(venue) = changes.venue {
        slot.venue = venue;
    }
    if let Some(time_from) = changes.time_from {
        slot.time_from = time_field("time_from", &time_from)?;
    }
    slot.time_to = changes
        .time_to
        .apply(slot.time_to, |value| time_field("time_to", &value))?;
    if let Some(min_players) = changes.min_players {
        slot.min_players = min_players;
    }
    if let Some(places) = changes.places {
        slot.places = places;
    }
    if let Some(target) = changes.target {
        slot.target = target;
    }
    if let Some(days) = changes.deadline_days_before {
        slot.deadline_days_before = days;
    }
    if let Some(time) = changes.deadline_time {
        slot.deadline_time = time_field("deadline_time", &time)?;
    }
    slot.validate()
        .map_err(|error| Problem::invalid(error.to_string()))?;
    // A date with its own minimum or places must still fit the new values.
    let calendar = slot_calendar(&slot)?;
    for session in db::session_overrides::of_slot(&db, &slot.id).await? {
        calendar
            .session(session.date, Some(&session.values))
            .map_err(|error| {
                Problem::invalid(format!("the session of {}: {error}", session.date))
            })?;
    }
    db::slots::update_settings(&db, &slot).await?;
    json(200, &slot_view(&slot)?)
}

/// Delete the slot and all its data (organizer).
#[cfg_attr(test, utoipa::path(
    delete,
    path = "/api/v1/slots/{id}",
    operation_id = "deleteSlot",
    tag = "slots",
    params(("id" = String, Path, description = "Slot id")),
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 204, description = "Slot deleted, with its members, answers, guests, lineups and recovery links"),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
    )
))]
pub(super) async fn delete(req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    auth::organizer(&member)?;
    db::slots::delete(&db, &slot.id).await?;
    no_content()
}

/// The invite code of a slot.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct InviteCodeView {
    invite_code: String,
}

/// Rotate the invite code (organizer). Members who already joined keep calling with the slot id
/// and their token: only the old link stops working.
#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/v1/slots/{id}/invite-code",
    operation_id = "rotateInviteCode",
    tag = "slots",
    params(("id" = String, Path, description = "Slot id")),
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "The new invite code", body = InviteCodeView),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
    )
))]
pub(super) async fn rotate_invite_code(req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    auth::organizer(&member)?;
    for attempt in 1..=INVITE_CODE_ATTEMPTS {
        let code = InviteCode::generate()?;
        match db::slots::set_invite_code(&db, &slot.id, &code).await {
            Ok(()) => {
                return json(
                    200,
                    &InviteCodeView {
                        invite_code: code.to_string(),
                    },
                );
            }
            Err(error) if attempt < INVITE_CODE_ATTEMPTS && db::is_constraint(&error) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(Problem::new(ProblemType::Internal))
}

/// `PUT /slots/:id/list`.
#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub(super) struct ListState {
    /// A closed list takes no new name through the invite link. Free names can still be claimed.
    closed: bool,
}

/// Close or reopen the list of names (organizer).
#[cfg_attr(test, utoipa::path(
    put,
    path = "/api/v1/slots/{id}/list",
    operation_id = "setList",
    tag = "slots",
    params(("id" = String, Path, description = "Slot id")),
    request_body = ListState,
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "The list state", body = ListState),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 422, response = super::problem::responses::Unprocessable),
    )
))]
pub(super) async fn set_list(mut req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    auth::organizer(&member)?;
    let (state, _): (ListState, _) = read_json(&mut req).await?;
    db::slots::set_list_closed(&db, &slot.id, state.closed).await?;
    json(200, &state)
}
