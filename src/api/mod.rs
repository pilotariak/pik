// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! The REST API, one module per resource. Every route lives under `/api/v1/`.
//!
//! Each resource module holds its handlers, its request and response types and their
//! `#[utoipa::path]` annotations, which `openapi.rs` (compiled for tests only) gathers into
//! `openapi.json`. A handler returns an [`ApiResult`]: its `Err` is a problem, sent as
//! `application/problem+json`.

mod auth;
mod health;
mod idempotency;
mod join;
mod lineups;
mod members;
#[cfg(test)]
mod openapi;
pub(crate) mod problem;
mod rate_limit;
mod recovery;
mod sessions;
mod slots;

use core::future::Future;

use jiff::Timestamp;
use jiff::civil::Date;
use serde::Serialize;
use serde::de::DeserializeOwned;
use worker::{D1Database, Env, Request, Response, RouteContext, Router};

use crate::db;
use crate::domain::civil::parse_date;
use crate::domain::ids::SlotId;
use crate::domain::slot::{Member, Slot};
use problem::{ApiResult, PROBLEM_JSON, Problem, ProblemType};

/// Prefixes a path with `/api/v1`, the only place the API version appears. Kancha web forwards
/// `/api/*` unchanged, and a breaking change goes to `/api/v2/` while installed app versions
/// keep calling `/api/v1/`.
macro_rules! v1 {
    ($path:literal) => {
        concat!("/api/v1", $path)
    };
}

/// The largest request body: every body of the API is a few hundred bytes.
const MAX_BODY_BYTES: usize = 16 * 1024;

/// Sends the problem of a failed handler.
async fn run(handler: impl Future<Output = ApiResult<Response>>) -> worker::Result<Response> {
    match handler.await {
        Ok(response) => Ok(response),
        Err(problem) => problem.into_response(),
    }
}

/// Registers the API routes on the router.
pub(crate) fn routes(router: Router<'_, ()>) -> Router<'_, ()> {
    router
        .get(v1!("/health"), |_, _| health::get())
        // slots
        .post_async(v1!("/slots"), |req, ctx| run(slots::create(req, ctx)))
        .get_async(v1!("/me"), |req, ctx| run(slots::me(req, ctx)))
        .get_async(v1!("/slots/:id"), |req, ctx| run(slots::get(req, ctx)))
        .patch_async(v1!("/slots/:id"), |req, ctx| run(slots::update(req, ctx)))
        .delete_async(v1!("/slots/:id"), |req, ctx| run(slots::delete(req, ctx)))
        .post_async(v1!("/slots/:id/invite-code"), |req, ctx| {
            run(slots::rotate_invite_code(req, ctx))
        })
        .put_async(v1!("/slots/:id/list"), |req, ctx| {
            run(slots::set_list(req, ctx))
        })
        // join
        .get_async(v1!("/join/:code"), |req, ctx| run(join::preview(req, ctx)))
        .post_async(v1!("/join/:code/claim"), |req, ctx| {
            run(join::claim(req, ctx))
        })
        .post_async(v1!("/join/:code/members"), |req, ctx| {
            run(join::add_name(req, ctx))
        })
        // members
        .post_async(v1!("/slots/:id/members"), |req, ctx| {
            run(members::add(req, ctx))
        })
        .patch_async(v1!("/slots/:id/members/:member"), |req, ctx| {
            run(members::update(req, ctx))
        })
        .delete_async(v1!("/slots/:id/members/:member/claim"), |req, ctx| {
            run(members::release(req, ctx))
        })
        .delete_async(v1!("/slots/:id/members/me"), |req, ctx| {
            run(members::leave(req, ctx))
        })
        // sessions
        .get_async(v1!("/slots/:id/sessions"), |req, ctx| {
            run(sessions::list(req, ctx))
        })
        .get_async(v1!("/slots/:id/sessions/:date"), |req, ctx| {
            run(sessions::get(req, ctx))
        })
        .patch_async(v1!("/slots/:id/sessions/:date"), |req, ctx| {
            run(sessions::update(req, ctx))
        })
        .put_async(v1!("/slots/:id/sessions/:date/answer"), |req, ctx| {
            run(sessions::answer(req, ctx))
        })
        .put_async(v1!("/slots/:id/sessions/:date/guests"), |req, ctx| {
            run(sessions::guests(req, ctx))
        })
        // lineups
        .put_async(v1!("/slots/:id/sessions/:date/lineup"), |req, ctx| {
            run(lineups::put(req, ctx))
        })
        // recovery
        .post_async(v1!("/slots/:id/members/:member/recovery"), |req, ctx| {
            run(recovery::create(req, ctx))
        })
        .post_async(v1!("/recover/:token"), |req, ctx| {
            run(recovery::redeem(req, ctx))
        })
}

/// What every `/api/*` response needs, after the router: no caching of private data, and a
/// problem instead of the router's plain-text 404 and 405.
pub(crate) fn finish(response: Response) -> worker::Result<Response> {
    let status = response.status_code();
    let plain = !response
        .headers()
        .get("Content-Type")?
        .is_some_and(|content_type| content_type.starts_with(PROBLEM_JSON));
    let mut response = match status {
        404 if plain => Problem::new(ProblemType::NotFound).into_response()?,
        405 if plain => Problem::new(ProblemType::MethodNotAllowed).into_response()?,
        _ => response,
    };
    response
        .headers_mut()
        .set("Cache-Control", "private, no-store")?;
    Ok(response)
}

/// The current instant, from the Worker's clock. Domain functions take it as an argument:
/// `jiff::Timestamp::now()` panics on wasm32-unknown-unknown, and no header, parameter or
/// variable overrides it.
fn now() -> ApiResult<Timestamp> {
    let millis = i64::try_from(worker::Date::now().as_millis())
        .map_err(|_| Problem::new(ProblemType::Internal))?;
    Timestamp::from_millisecond(millis).map_err(|_| Problem::new(ProblemType::Internal))
}

/// The D1 binding.
fn database(env: &Env) -> ApiResult<D1Database> {
    Ok(env.d1("DB")?)
}

/// The caller of a `/slots/:id…` route: their active member, and the slot.
struct Caller {
    member: Member,
    slot: Slot,
}

/// 401 without a known token, then 404 unless the caller is an active member of the slot. Both
/// come before any check of the path or the body, so that a non-member learns nothing.
async fn slot_caller(
    req: &Request,
    ctx: &RouteContext<()>,
    db: &D1Database,
    now: Timestamp,
) -> ApiResult<Caller> {
    let player = auth::player(req, db, now).await?;
    let slot_id = param(ctx, "id", SlotId::parse)?;
    let member = auth::member(db, &slot_id, &player).await?;
    let slot = db::slots::find(db, &slot_id)
        .await?
        .ok_or_else(|| Problem::new(ProblemType::NotFound))?;
    Ok(Caller { member, slot })
}

/// A path parameter: 404 when it cannot be what the route expects.
fn param<T>(ctx: &RouteContext<()>, name: &str, parse: impl Fn(&str) -> Option<T>) -> ApiResult<T> {
    ctx.param(name)
        .and_then(|value| parse(value))
        .ok_or_else(|| Problem::new(ProblemType::NotFound))
}

/// The `:date` of a session path: 422 when it is not a `YYYY-MM-DD` date.
fn session_date(ctx: &RouteContext<()>) -> ApiResult<Date> {
    ctx.param("date")
        .and_then(|value| parse_date(value))
        .ok_or_else(|| {
            Problem::detail(
                ProblemType::NotASession,
                "the date of a session is a YYYY-MM-DD date in Europe/Paris",
            )
        })
}

/// The JSON body of a request: 422 when it is too large or not JSON.
async fn read_value(req: &mut Request) -> ApiResult<serde_json::Value> {
    let text = req.text().await?;
    if text.len() > MAX_BODY_BYTES {
        return Err(Problem::invalid(format!(
            "the body is larger than {MAX_BODY_BYTES} bytes"
        )));
    }
    serde_json::from_str(&text).map_err(|_| Problem::invalid("the body is not JSON"))
}

/// A JSON value as `T`: 422 with what does not fit.
fn from_value<T: DeserializeOwned>(value: serde_json::Value) -> ApiResult<T> {
    serde_json::from_value(value).map_err(|error| Problem::invalid(error.to_string()))
}

/// The JSON body of a request, typed, and as a value for the idempotency hash.
async fn read_json<T: DeserializeOwned>(req: &mut Request) -> ApiResult<(T, serde_json::Value)> {
    let value = read_value(req).await?;
    Ok((from_value(value.clone())?, value))
}

/// A JSON response.
fn json<T: Serialize>(status: u16, body: &T) -> ApiResult<Response> {
    Ok(Response::from_json(body)?.with_status(status))
}

/// A 204 response.
fn no_content() -> ApiResult<Response> {
    Ok(Response::empty()?.with_status(204))
}

/// A nullable `PATCH` field: absent keeps the value, `null` resets it, a value sets it. Fields
/// of this type need `#[serde(default)]`, so that an absent field is [`Change::Keep`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Change<T> {
    #[default]
    Keep,
    Reset,
    Set(T),
}

impl<'de, T: serde::Deserialize<'de>> serde::Deserialize<'de> for Change<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(|value| value.map_or(Self::Reset, Self::Set))
    }
}

impl<T> Change<T> {
    /// The new value of a field whose current value is `current`, converted by `convert`.
    fn apply<U>(
        self,
        current: Option<U>,
        convert: impl FnOnce(T) -> ApiResult<U>,
    ) -> ApiResult<Option<U>> {
        match self {
            Self::Keep => Ok(current),
            Self::Reset => Ok(None),
            Self::Set(value) => convert(value).map(Some),
        }
    }
}
