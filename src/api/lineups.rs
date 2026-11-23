// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Lineups: `PUT /slots/:id/sessions/:date/lineup`.
//!
//! Several organizers may compose the same session. The lineup version is the `ETag` of
//! `GET …/sessions/:date`, and a write must send it in `If-Match`: a stale one gets 412, a
//! missing one 428.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use worker::{Request, Response, RouteContext};

use super::problem::{ApiResult, Problem, ProblemType};
use super::sessions::Sessions;
use super::{Caller, auth, database, json, now, read_json, session_date, slot_caller};
use crate::db;
use crate::domain::ids::{GuestId, MemberId};
use crate::domain::lineup::{self, Entry, Game, Seat};

/// A player of a lineup: `{"member": "m_…"}` or `{"guest": "g_…"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) enum SeatView {
    Member(String),
    Guest(String),
}

impl From<&Seat> for SeatView {
    fn from(seat: &Seat) -> Self {
        match seat {
            Seat::Member(id) => Self::Member(id.to_string()),
            Seat::Guest(id) => Self::Guest(id.to_string()),
        }
    }
}

/// One game: two teams of exactly two players.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct GameView {
    /// Game number, from 1.
    game: i64,
    a: Vec<SeatView>,
    b: Vec<SeatView>,
}

/// The lineup of a session. A session without a lineup has version 0 and empty lists.
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct LineupView {
    version: i64,
    /// Selected players, in order. A selected player outside every game waits for the next one.
    selection: Vec<SeatView>,
    games: Vec<GameView>,
}

/// The lineup of stored entries.
pub(super) fn view(version: i64, entries: &[Entry]) -> LineupView {
    let (selection, games) = lineup::split(entries);
    LineupView {
        version,
        selection: selection.iter().map(SeatView::from).collect(),
        games: games
            .iter()
            .map(|game| GameView {
                game: game.number,
                a: game.a.iter().map(SeatView::from).collect(),
                b: game.b.iter().map(SeatView::from).collect(),
            })
            .collect(),
    }
}

/// The `ETag` of a lineup version: `"0"` without a lineup.
pub(super) fn etag(version: i64) -> String {
    format!("\"{version}\"")
}

/// The version an `If-Match` header names. `None` for anything but one strong `ETag`: a weak
/// one never matches, as `If-Match` compares strongly.
fn if_match(header: &str) -> Option<i64> {
    header
        .trim()
        .strip_prefix('"')?
        .strip_suffix('"')?
        .parse()
        .ok()
}

/// `PUT …/lineup`.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub(super) struct LineupRequest {
    selection: Vec<SeatView>,
    games: Vec<GameView>,
}

/// Selection and pairs of one session (organizer). Needs `If-Match` with the `ETag` of
/// `GET …/sessions/:date`. A player plays in one game of a session at most.
#[cfg_attr(test, utoipa::path(
    put,
    path = "/api/v1/slots/{id}/sessions/{date}/lineup",
    operation_id = "putLineup",
    tag = "lineups",
    params(
        ("id" = String, Path, description = "Slot id"),
        ("date" = String, Path, description = "Session date, YYYY-MM-DD in Europe/Paris"),
        ("If-Match" = String, Header, description = "The ETag of GET …/sessions/:date"),
    ),
    request_body = LineupRequest,
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "The new lineup", body = LineupView,
            headers(("ETag" = String, description = "The new lineup version"))),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 409, response = super::problem::responses::Conflict),
        (status = 412, response = super::problem::responses::PreconditionFailed),
        (status = 422, response = super::problem::responses::Unprocessable),
        (status = 428, response = super::problem::responses::PreconditionRequired),
    )
))]
pub(super) async fn put(mut req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    auth::organizer(&member)?;
    let date = session_date(&ctx)?;
    let Some(header) = req.headers().get("If-Match")? else {
        return Err(Problem::new(ProblemType::IfMatchRequired));
    };
    let expected = if_match(&header).ok_or_else(|| Problem::new(ProblemType::LineupChanged))?;
    let (request, _): (LineupRequest, _) = read_json(&mut req).await?;

    let sessions = Sessions::load(&db, &slot, (date, date), Some(&member.id), now).await?;
    let session = sessions.session(date)?;
    if session.cancelled {
        return Err(Problem::new(ProblemType::SessionCancelled));
    }

    let members: HashSet<MemberId> = db::members::of_slot(&db, &slot.id)
        .await?
        .into_iter()
        .filter(|other| !other.left)
        .map(|other| other.id)
        .collect();
    let guests: HashSet<GuestId> = db::guests::of_session(&db, &slot.id, date)
        .await?
        .into_iter()
        .map(|guest| guest.id)
        .collect();
    let seat = |view: &SeatView| -> ApiResult<Seat> {
        match view {
            SeatView::Member(id) => MemberId::parse(id)
                .filter(|id| members.contains(id))
                .map(Seat::Member)
                .ok_or_else(|| Problem::invalid(format!("{id} is not a member of the slot"))),
            SeatView::Guest(id) => GuestId::parse(id)
                .filter(|id| guests.contains(id))
                .map(Seat::Guest)
                .ok_or_else(|| Problem::invalid(format!("{id} is not a guest of this session"))),
        }
    };
    let selection = request
        .selection
        .iter()
        .map(&seat)
        .collect::<ApiResult<Vec<_>>>()?;
    let games = request
        .games
        .iter()
        .map(|game| {
            Ok(Game {
                number: game.game,
                a: game.a.iter().map(&seat).collect::<ApiResult<_>>()?,
                b: game.b.iter().map(&seat).collect::<ApiResult<_>>()?,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let entries = lineup::entries(&selection, &games, session.places)
        .map_err(|error| Problem::invalid(error.to_string()))?;

    if db::lineups::version(&db, &slot.id, date).await? != expected {
        return Err(Problem::new(ProblemType::LineupChanged));
    }
    let mut statements = vec![
        db::lineups::advance(&db, &slot.id, date, expected, &member.id, now)?,
        db::lineup_entries::delete_session(&db, &slot.id, date)?,
    ];
    for entry in &entries {
        statements.push(db::lineup_entries::insert(&db, &slot.id, date, entry)?);
    }
    if let Err(error) = db::batch(&db, statements).await {
        // The guard of `advance` fails the batch when another write moved the version.
        if db::lineups::version(&db, &slot.id, date).await? != expected {
            return Err(Problem::new(ProblemType::LineupChanged));
        }
        return Err(error.into());
    }
    let version = expected + 1;
    let mut response = json(200, &view(version, &entries))?;
    response.headers_mut().set("ETag", &etag(version))?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::{etag, if_match};

    #[test]
    fn if_match_reads_one_strong_etag() {
        assert_eq!(if_match(&etag(3)), Some(3), "the ETag of version 3");
        assert_eq!(if_match(" \"0\" "), Some(0), "no lineup yet");
        for invalid in ["3", "W/\"3\"", "\"a\"", "*", "\"3\", \"4\""] {
            assert_eq!(if_match(invalid), None, "{invalid} names no version");
        }
    }
}
