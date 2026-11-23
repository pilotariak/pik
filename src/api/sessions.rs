// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Sessions and answers: `/slots/:id/sessions…`.
//!
//! Sessions are not stored. Their dates, deadlines, effective values, states, call order and
//! sessions played come from [`calendar`](crate::domain::calendar), computed on every read from
//! the slot, its overrides, answers and lineups.

use std::collections::{HashMap, HashSet};

use jiff::Timestamp;
use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use worker::{D1Database, Request, Response, RouteContext};

use super::lineups::{self, LineupView};
use super::problem::{ApiResult, Problem, ProblemType};
use super::{Caller, Change, database, json, now, read_json, session_date, slot_caller};
use crate::db;
use crate::db::answers::{NewAnswer, StoredAnswer};
use crate::db::guests::Guest;
use crate::db::session_overrides::SessionOverride;
use crate::domain::calendar::call_order::{Answer, LineupEntry, call_order, sessions_played};
use crate::domain::calendar::session::{self as calendar, Override, Session, State, Status, Tally};
use crate::domain::calendar::{CalendarError, paris_date};
use crate::domain::civil::parse_date;
use crate::domain::ids::{GuestId, MemberId, SlotId};
use crate::domain::name::DisplayName;
use crate::domain::slot::{MAX_PLAYERS, Member, Slot, short_text};

/// Longest note of a session, in characters.
const MAX_NOTE_CHARS: usize = 200;
/// Most guests of a session.
const MAX_GUESTS: usize = 20;
/// Longest range of `GET /slots/:id/sessions`, in days.
const MAX_RANGE_DAYS: i32 = 366;

/// A player's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) enum AnswerStatus {
    /// Comes.
    Ok,
    /// Can come if needed: does not count toward the minimum.
    Dispo,
    /// Does not come.
    Ko,
}

impl From<Status> for AnswerStatus {
    fn from(status: Status) -> Self {
        match status {
            Status::Ok => Self::Ok,
            Status::Dispo => Self::Dispo,
            Status::Ko => Self::Ko,
        }
    }
}

impl From<AnswerStatus> for Status {
    fn from(status: AnswerStatus) -> Self {
        match status {
            AnswerStatus::Ok => Self::Ok,
            AnswerStatus::Dispo => Self::Dispo,
            AnswerStatus::Ko => Self::Ko,
        }
    }
}

/// State of a session, computed on every read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) enum SessionState {
    /// *Assurée*: the OK answers reach the minimum.
    On,
    /// *Il manque n*: before the deadline, `missing` more OK answers are needed.
    Missing,
    /// *À compléter*: at the deadline, the Dispo answers would complete the session.
    ToComplete,
    /// Not enough players at the deadline, even with the Dispo answers.
    CancelledShort,
    /// Cancelled by an organizer.
    Cancelled,
}

/// Answers of a session by status. `none`: active members who did not answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct Counts {
    ok: u32,
    dispo: u32,
    ko: u32,
    none: u32,
}

/// One session: its effective values, state and counts.
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct SessionSummary {
    /// Local date in Europe/Paris.
    #[cfg_attr(test, schema(format = Date, example = "2026-11-25"))]
    date: String,
    /// When it starts, RFC 3339 in UTC.
    #[cfg_attr(test, schema(format = DateTime))]
    starts_at: String,
    /// Answer deadline, RFC 3339 in UTC.
    #[cfg_attr(test, schema(format = DateTime))]
    deadline: String,
    deadline_passed: bool,
    state: SessionState,
    /// OK answers still needed: non-zero only when `state` is `missing`.
    missing: u32,
    min_players: u32,
    places: u32,
    cancelled: bool,
    note: Option<String>,
    counts: Counts,
    /// The caller's answer, if any.
    my_answer: Option<AnswerStatus>,
}

/// What the calendar needs to summarize the sessions of a slot over a range of dates.
pub(super) struct Sessions<'a> {
    calendar: calendar::Slot,
    overrides: HashMap<Date, SessionOverride>,
    answers: HashMap<Date, Vec<StoredAnswer>>,
    active: HashSet<MemberId>,
    me: Option<&'a MemberId>,
    now: Timestamp,
}

/// The calendar of a slot, which the slot's rules make valid.
pub(super) fn slot_calendar(slot: &Slot) -> ApiResult<calendar::Slot> {
    slot.calendar().map_err(|error| calendar_problem(&error))
}

/// 422 for a date that is not a session; any other calendar error is a slot the API should not
/// have let through.
fn calendar_problem(error: &CalendarError) -> Problem {
    match error {
        CalendarError::NotASessionDate(_) => {
            Problem::detail(ProblemType::NotASession, error.to_string())
        }
        CalendarError::PlacesBelowMinimum { .. } => Problem::invalid(error.to_string()),
        CalendarError::InvalidInterval(_)
        | CalendarError::AnchorOutsideSeason
        | CalendarError::InvalidDeadlineDays(_)
        | CalendarError::OutOfRange => {
            worker::console_error!("internal error: {error}");
            Problem::new(ProblemType::Internal)
        }
    }
}

impl<'a> Sessions<'a> {
    /// Loads the overrides and answers of a slot from `from` to `to`, both included.
    pub(super) async fn load(
        db: &D1Database,
        slot: &Slot,
        range: (Date, Date),
        me: Option<&'a MemberId>,
        now: Timestamp,
    ) -> ApiResult<Self> {
        let (from, to) = range;
        let overrides = db::session_overrides::in_range(db, &slot.id, from, to)
            .await?
            .into_iter()
            .map(|session| (session.date, session))
            .collect();
        let mut answers: HashMap<Date, Vec<StoredAnswer>> = HashMap::new();
        for answer in db::answers::in_range(db, &slot.id, from, to).await? {
            answers.entry(answer.date).or_default().push(answer);
        }
        let active = db::members::of_slot(db, &slot.id)
            .await?
            .into_iter()
            .filter(|member| !member.left)
            .map(|member| member.id)
            .collect();
        Ok(Self {
            calendar: slot_calendar(slot)?,
            overrides,
            answers,
            active,
            me,
            now,
        })
    }

    /// The session on `date` with its effective values: 422 off the recurrence.
    pub(super) fn session(&self, date: Date) -> ApiResult<Session> {
        self.calendar
            .session(
                date,
                self.overrides.get(&date).map(|session| &session.values),
            )
            .map_err(|error| calendar_problem(&error))
    }

    fn answers(&self, date: Date) -> &[StoredAnswer] {
        self.answers.get(&date).map_or(&[], Vec::as_slice)
    }

    /// The summary of the session on `date`.
    pub(super) fn summary(&self, date: Date) -> ApiResult<SessionSummary> {
        let session = self.session(date)?;
        let answers = self.answers(date);
        let tally: Tally = answers.iter().map(|answer| answer.status).collect();
        let (state, missing) = match session.state(tally, self.now) {
            State::On => (SessionState::On, 0),
            State::Missing(missing) => (SessionState::Missing, missing),
            State::ToComplete => (SessionState::ToComplete, 0),
            State::CancelledForLackOfPlayers => (SessionState::CancelledShort, 0),
            State::CancelledByOrganizer => (SessionState::Cancelled, 0),
        };
        let mut counts = Counts::default();
        let mut answered = 0;
        for answer in answers {
            match answer.status {
                Status::Ok => counts.ok += 1,
                Status::Dispo => counts.dispo += 1,
                Status::Ko => counts.ko += 1,
            }
            if self.active.contains(&answer.member_id) {
                answered += 1;
            }
        }
        counts.none = u32::try_from(self.active.len().saturating_sub(answered)).unwrap_or(u32::MAX);
        let my_answer = self.me.and_then(|me| {
            answers
                .iter()
                .find(|answer| &answer.member_id == me)
                .map(|answer| answer.status.into())
        });
        Ok(SessionSummary {
            date: date.to_string(),
            starts_at: session.starts_at.to_string(),
            deadline: session.deadline.to_string(),
            deadline_passed: self.now >= session.deadline,
            state,
            missing,
            min_players: session.min_players,
            places: session.places,
            cancelled: session.cancelled,
            note: self
                .overrides
                .get(&date)
                .and_then(|session| session.note.clone()),
            counts,
            my_answer,
        })
    }
}

/// The summaries of the next `count` sessions of a slot from the Paris date of `now`.
pub(super) async fn upcoming(
    db: &D1Database,
    slot: &Slot,
    count: usize,
    me: &MemberId,
    now: Timestamp,
) -> ApiResult<Vec<SessionSummary>> {
    let calendar = slot_calendar(slot)?;
    let today = paris_date(now);
    let Some(first) = calendar.recurrence.next_on_or_after(today) else {
        return Ok(Vec::new());
    };
    let dates: Vec<Date> = calendar
        .recurrence
        .dates()
        .filter(|date| *date >= first)
        .take(count)
        .collect();
    let last = dates.last().copied().unwrap_or(first);
    let sessions = Sessions::load(db, slot, (first, last), Some(me), now).await?;
    dates
        .into_iter()
        .map(|date| sessions.summary(date))
        .collect()
}

/// `GET /slots/:id/sessions` query.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(utoipa::IntoParams))]
#[cfg_attr(test, into_params(parameter_in = Query))]
#[serde(deny_unknown_fields)]
pub(super) struct Range {
    /// First date, `YYYY-MM-DD` in Europe/Paris. Default: today in Paris.
    from: Option<String>,
    /// Last date, included. Default: the end of the season, or 366 days after `from` if sooner.
    /// At most 366 days after `from`.
    to: Option<String>,
}

/// Sessions of a date range.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct SessionList {
    sessions: Vec<SessionSummary>,
}

/// The sessions of a slot in a date range, with their states.
#[cfg_attr(test, utoipa::path(
    get,
    path = "/api/v1/slots/{id}/sessions",
    operation_id = "listSessions",
    tag = "sessions",
    params(("id" = String, Path, description = "Slot id"), Range),
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "Sessions in the range, by date", body = SessionList),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 422, response = super::problem::responses::Unprocessable),
    )
))]
pub(super) async fn list(req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    let url = req.url()?;
    let query: Range = read_query(&url)?;
    let read = |value: Option<String>, default: Date, name: &str| -> ApiResult<Date> {
        value.map_or(Ok(default), |value| {
            parse_date(&value)
                .ok_or_else(|| Problem::invalid(format!("{name} is a YYYY-MM-DD date")))
        })
    };
    let from = read(query.from, paris_date(now), "from")?;
    // Default: to the end of the season, within the longest range. After the season, the range
    // is the single day `from`, which has no session.
    let longest = from
        .checked_add(jiff::Span::new().days(MAX_RANGE_DAYS))
        .map_err(|_| Problem::invalid("from is out of range"))?;
    let to = read(query.to, slot.season_end.min(longest).max(from), "to")?;
    if to < from {
        return Err(Problem::invalid("to must not be before from"));
    }
    let days = from
        .until(to)
        .map_err(|_| Problem::invalid("out of range"))?
        .get_days();
    if days > MAX_RANGE_DAYS {
        return Err(Problem::invalid(format!(
            "a range covers at most {MAX_RANGE_DAYS} days"
        )));
    }
    let calendar = slot_calendar(&slot)?;
    let dates: Vec<Date> = calendar
        .recurrence
        .dates()
        .filter(|date| (from..=to).contains(date))
        .collect();
    let sessions = Sessions::load(&db, &slot, (from, to), Some(&member.id), now).await?;
    let sessions = dates
        .into_iter()
        .map(|date| sessions.summary(date))
        .collect::<ApiResult<_>>()?;
    json(200, &SessionList { sessions })
}

/// Reads the query string into `T`: 422 on an unknown or repeated parameter.
fn read_query<T: serde::de::DeserializeOwned>(url: &worker::Url) -> ApiResult<T> {
    let mut map = serde_json::Map::new();
    for (name, value) in url.query_pairs() {
        if map
            .insert(
                name.into_owned(),
                serde_json::Value::String(value.into_owned()),
            )
            .is_some()
        {
            return Err(Problem::invalid("a query parameter is repeated"));
        }
    }
    serde_json::from_value(serde_json::Value::Object(map))
        .map_err(|error| Problem::invalid(error.to_string()))
}

/// An answer of a session.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct AnswerView {
    member_id: String,
    /// `null`: a member who left, shown as "Ancien joueur".
    name: Option<String>,
    status: AnswerStatus,
    #[cfg_attr(test, schema(format = DateTime))]
    answered_at: String,
    /// Sessions played this season.
    played: u32,
    /// The caller's own answer.
    me: bool,
}

/// An active member who did not answer.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct NoAnswerView {
    member_id: String,
    name: Option<String>,
    played: u32,
    me: bool,
}

/// A guest of a session.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct GuestView {
    id: String,
    name: String,
}

impl From<Guest> for GuestView {
    fn from(guest: Guest) -> Self {
        Self {
            id: guest.id.to_string(),
            name: guest.name,
        }
    }
}

/// One session for the Séance and Composer screens.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct SessionDetail {
    #[serde(flatten)]
    summary: SessionSummary,
    /// Answers: OK by answer time, then Dispo, then KO.
    answers: Vec<AnswerView>,
    no_answer: Vec<NoAnswerView>,
    guests: Vec<GuestView>,
    /// Member ids in the order they are called: the first `places` take the places.
    call_order: Vec<String>,
    lineup: LineupView,
}

/// Sessions played this season by each member, from the lineups before today.
async fn played(db: &D1Database, slot: &SlotId, today: Date) -> ApiResult<HashMap<MemberId, u32>> {
    let entries = db::lineup_entries::played_before(db, slot, today).await?;
    let cancelled: HashSet<Date> = db::session_overrides::of_slot(db, slot)
        .await?
        .into_iter()
        .filter(|session| session.values.cancelled)
        .map(|session| session.date)
        .collect();
    let lineup: Vec<LineupEntry<'_>> = entries
        .iter()
        .map(|entry| LineupEntry {
            date: entry.date,
            member_id: Some(entry.member_id.as_str()),
            game: entry.game,
        })
        .collect();
    Ok(sessions_played(&lineup, &cancelled, today)
        .into_iter()
        .filter_map(|(member, count)| MemberId::parse(member).map(|id| (id, count)))
        .collect())
}

/// Order of answers in a session: OK, Dispo, then KO, each by answer time.
fn status_rank(status: Status) -> u8 {
    match status {
        Status::Ok => 0,
        Status::Dispo => 1,
        Status::Ko => 2,
    }
}

/// One session: answers by status, guests, sessions played, call order and lineup. Its `ETag`
/// is the lineup version, for `If-Match` on `PUT …/lineup`.
#[cfg_attr(test, utoipa::path(
    get,
    path = "/api/v1/slots/{id}/sessions/{date}",
    operation_id = "getSession",
    tag = "sessions",
    params(
        ("id" = String, Path, description = "Slot id"),
        ("date" = String, Path, description = "Session date, YYYY-MM-DD in Europe/Paris"),
    ),
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "The session", body = SessionDetail,
            headers(("ETag" = String, description = "Lineup version, \"0\" without a lineup"))),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 422, response = super::problem::responses::Unprocessable),
    )
))]
pub(super) async fn get(req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    let date = session_date(&ctx)?;
    let sessions = Sessions::load(&db, &slot, (date, date), Some(&member.id), now).await?;
    let summary = sessions.summary(date)?;

    let members = db::members::of_slot(&db, &slot.id).await?;
    let names: HashMap<&MemberId, &Member> = members.iter().map(|m| (&m.id, m)).collect();
    let played = played(&db, &slot.id, paris_date(now)).await?;
    let played_of = |id: &MemberId| played.get(id).copied().unwrap_or(0);

    let mut stored: Vec<&StoredAnswer> = sessions.answers(date).iter().collect();
    stored.sort_by_key(|answer| {
        (
            status_rank(answer.status),
            answer.answered_at,
            answer.member_id.clone(),
        )
    });
    let answers = stored
        .iter()
        .map(|answer| AnswerView {
            member_id: answer.member_id.to_string(),
            name: names
                .get(&answer.member_id)
                .and_then(|member| member.name.clone()),
            status: answer.status.into(),
            answered_at: answer.answered_at.to_string(),
            played: played_of(&answer.member_id),
            me: answer.member_id == member.id,
        })
        .collect();
    let answered: HashSet<&MemberId> = stored.iter().map(|answer| &answer.member_id).collect();
    let no_answer = members
        .iter()
        .filter(|other| !other.left && !answered.contains(&other.id))
        .map(|other| NoAnswerView {
            member_id: other.id.to_string(),
            name: other.name.clone(),
            played: played_of(&other.id),
            me: other.id == member.id,
        })
        .collect();

    let played_by_str: HashMap<&str, u32> =
        played.iter().map(|(id, n)| (id.as_str(), *n)).collect();
    let calls: Vec<Answer<'_>> = stored
        .iter()
        .map(|answer| Answer {
            member_id: answer.member_id.as_str(),
            status: answer.status,
            answered_at: answer.answered_at,
        })
        .collect();
    let call_order = call_order(&calls, &played_by_str)
        .into_iter()
        .map(str::to_owned)
        .collect();

    let guests = db::guests::of_session(&db, &slot.id, date).await?;
    let version = db::lineups::version(&db, &slot.id, date).await?;
    let entries = db::lineup_entries::of_session(&db, &slot.id, date).await?;
    let detail = SessionDetail {
        summary,
        answers,
        no_answer,
        guests: guests.into_iter().map(GuestView::from).collect(),
        call_order,
        lineup: lineups::view(version, &entries),
    };
    let mut response = json(200, &detail)?;
    response
        .headers_mut()
        .set("ETag", &lineups::etag(version))?;
    Ok(response)
}

/// `PATCH /slots/:id/sessions/:date`: absent fields keep their value, `null` goes back to the
/// slot's value.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub(super) struct SessionChanges {
    /// Cancel the date, or undo its cancellation.
    cancelled: Option<bool>,
    #[serde(default)]
    #[cfg_attr(test, schema(value_type = Option<String>))]
    note: Change<String>,
    #[serde(default)]
    #[cfg_attr(test, schema(value_type = Option<u32>, minimum = 1, maximum = 30))]
    min_players: Change<u32>,
    #[serde(default)]
    #[cfg_attr(test, schema(value_type = Option<u32>, minimum = 1, maximum = 30))]
    places: Change<u32>,
    /// Answer deadline of this date, RFC 3339.
    #[serde(default)]
    #[cfg_attr(test, schema(value_type = Option<String>, format = DateTime))]
    deadline_at: Change<String>,
}

/// A count of players for one date: 1 to 30.
fn players(field: &str, count: u32) -> ApiResult<u32> {
    if (1..=MAX_PLAYERS).contains(&count) {
        Ok(count)
    } else {
        Err(Problem::invalid(format!(
            "{field} is from 1 to {MAX_PLAYERS}"
        )))
    }
}

/// Cancel or uncancel one date, or change its note, minimum, places or deadline (organizer).
#[cfg_attr(test, utoipa::path(
    patch,
    path = "/api/v1/slots/{id}/sessions/{date}",
    operation_id = "updateSession",
    tag = "sessions",
    params(
        ("id" = String, Path, description = "Slot id"),
        ("date" = String, Path, description = "Session date, YYYY-MM-DD in Europe/Paris"),
    ),
    request_body = SessionChanges,
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "The session after the change", body = SessionSummary),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 422, response = super::problem::responses::Unprocessable),
    )
))]
pub(super) async fn update(mut req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    super::auth::organizer(&member)?;
    let date = session_date(&ctx)?;
    let (changes, _): (SessionChanges, _) = read_json(&mut req).await?;
    let calendar = slot_calendar(&slot)?;
    let current = db::session_overrides::in_range(&db, &slot.id, date, date)
        .await?
        .into_iter()
        .next();
    calendar
        .session(date, current.as_ref().map(|session| &session.values))
        .map_err(|error| calendar_problem(&error))?;

    let mut next = current.unwrap_or(SessionOverride {
        date,
        values: Override::default(),
        note: None,
    });
    if let Some(cancelled) = changes.cancelled {
        next.values.cancelled = cancelled;
    }
    next.note = changes.note.apply(next.note, |note| {
        short_text("note", &note, MAX_NOTE_CHARS)
            .map_err(|error| Problem::invalid(error.to_string()))
    })?;
    next.values.min_players = changes
        .min_players
        .apply(next.values.min_players, |count| {
            players("min_players", count)
        })?;
    next.values.places = changes
        .places
        .apply(next.values.places, |count| players("places", count))?;
    next.values.deadline_at = changes
        .deadline_at
        .apply(next.values.deadline_at, |value| {
            value
                .parse::<Timestamp>()
                .map_err(|_| Problem::invalid("deadline_at is an RFC 3339 instant"))
        })?;
    calendar
        .session(date, Some(&next.values))
        .map_err(|error| calendar_problem(&error))?;
    db::session_overrides::upsert(&db, &slot.id, &next, now).await?;

    let sessions = Sessions::load(&db, &slot, (date, date), Some(&member.id), now).await?;
    json(200, &sessions.summary(date)?)
}

/// `PUT …/answer`.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub(super) struct AnswerRequest {
    status: AnswerStatus,
    /// Another member, for an organizer who answers for them. Default: the caller.
    member_id: Option<String>,
}

/// An answer written, with the recomputed session.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct AnswerResult {
    member_id: String,
    status: AnswerStatus,
    session: SessionSummary,
}

/// The caller's answer, or a member's answer (organizer). Last write wins. After the deadline,
/// only `ko` is accepted.
#[cfg_attr(test, utoipa::path(
    put,
    path = "/api/v1/slots/{id}/sessions/{date}/answer",
    operation_id = "putAnswer",
    tag = "sessions",
    params(
        ("id" = String, Path, description = "Slot id"),
        ("date" = String, Path, description = "Session date, YYYY-MM-DD in Europe/Paris"),
    ),
    request_body = AnswerRequest,
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "The answer, with the recomputed counts and state", body = AnswerResult),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 409, response = super::problem::responses::Conflict),
        (status = 422, response = super::problem::responses::Unprocessable),
    )
))]
pub(super) async fn answer(mut req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    let date = session_date(&ctx)?;
    let (request, _): (AnswerRequest, _) = read_json(&mut req).await?;
    let target = match request.member_id.as_deref() {
        None => member.id.clone(),
        Some(id) if id == member.id.as_str() => member.id.clone(),
        Some(id) => {
            super::auth::organizer(&member)?;
            let target = MemberId::parse(id)
                .ok_or_else(|| Problem::invalid("member_id is not a member of the slot"))?;
            match db::members::find(&db, &slot.id, &target).await? {
                Some(found) if !found.left => found.id,
                _ => return Err(Problem::invalid("member_id is not a member of the slot")),
            }
        }
    };
    let sessions = Sessions::load(&db, &slot, (date, date), Some(&member.id), now).await?;
    let session = sessions.session(date)?;
    if session.cancelled {
        return Err(Problem::new(ProblemType::SessionCancelled));
    }
    if now >= session.deadline && request.status != AnswerStatus::Ko {
        return Err(Problem::new(ProblemType::DeadlinePassed));
    }
    let set_by = (target != member.id).then_some(&member.id);
    db::answers::upsert(
        &db,
        &NewAnswer {
            slot: &slot.id,
            date,
            member: &target,
            status: request.status.into(),
            set_by,
        },
        now,
    )
    .await?;
    let sessions = Sessions::load(&db, &slot, (date, date), Some(&member.id), now).await?;
    json(
        200,
        &AnswerResult {
            member_id: target.to_string(),
            status: request.status,
            session: sessions.summary(date)?,
        },
    )
}

/// A guest to keep or add, by name.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub(super) struct GuestName {
    name: String,
}

/// `PUT …/guests`: the whole list of guests of a session.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub(super) struct GuestsRequest {
    guests: Vec<GuestName>,
}

/// The guests of a session.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct GuestList {
    guests: Vec<GuestView>,
}

/// Replaces the guests of one session (organizer). A guest whose name is kept keeps their id and
/// lineup place, so sending the same list again changes nothing. Removing a guest who is in the
/// lineup removes them from it and moves its `ETag`.
#[cfg_attr(test, utoipa::path(
    put,
    path = "/api/v1/slots/{id}/sessions/{date}/guests",
    operation_id = "putGuests",
    tag = "sessions",
    params(
        ("id" = String, Path, description = "Slot id"),
        ("date" = String, Path, description = "Session date, YYYY-MM-DD in Europe/Paris"),
    ),
    request_body = GuestsRequest,
    security(("bearer" = []), ("cookie" = [])),
    responses(
        (status = 200, description = "The guests, in the order sent", body = GuestList),
        (status = 401, response = super::problem::responses::Unauthorized),
        (status = 403, response = super::problem::responses::Forbidden),
        (status = 404, response = super::problem::responses::NotFound),
        (status = 409, response = super::problem::responses::Conflict),
        (status = 422, response = super::problem::responses::Unprocessable),
    )
))]
pub(super) async fn guests(mut req: Request, ctx: RouteContext<()>) -> ApiResult<Response> {
    let db = database(&ctx.env)?;
    let now = now()?;
    let Caller { member, slot } = slot_caller(&req, &ctx, &db, now).await?;
    super::auth::organizer(&member)?;
    let date = session_date(&ctx)?;
    let (request, _): (GuestsRequest, _) = read_json(&mut req).await?;
    if request.guests.len() > MAX_GUESTS {
        return Err(Problem::invalid(format!(
            "a session has at most {MAX_GUESTS} guests"
        )));
    }
    let mut wanted = Vec::with_capacity(request.guests.len());
    let mut keys = HashSet::new();
    for guest in &request.guests {
        let name =
            DisplayName::parse(&guest.name).map_err(|error| Problem::invalid(error.to_string()))?;
        if !keys.insert(name.key().to_owned()) {
            return Err(Problem::invalid("a guest appears twice"));
        }
        wanted.push(name);
    }
    let sessions = Sessions::load(&db, &slot, (date, date), Some(&member.id), now).await?;
    if sessions.session(date)?.cancelled {
        return Err(Problem::new(ProblemType::SessionCancelled));
    }

    let existing = db::guests::of_session(&db, &slot.id, date).await?;
    let mut by_key: HashMap<String, Guest> = existing
        .into_iter()
        .filter_map(|guest| {
            DisplayName::parse(&guest.name)
                .ok()
                .map(|name| (name.key().to_owned(), guest))
        })
        .collect();
    let mut kept = Vec::with_capacity(wanted.len());
    let mut statements = Vec::new();
    let mut added = Vec::new();
    for name in &wanted {
        if let Some(guest) = by_key.remove(name.key()) {
            kept.push(guest);
        } else {
            let guest = Guest {
                id: GuestId::generate()?,
                name: name.text().to_owned(),
            };
            added.push(guest.clone());
            kept.push(guest);
        }
    }
    let removed: Vec<GuestId> = by_key.into_values().map(|guest| guest.id).collect();
    if !removed.is_empty() {
        statements.push(db::lineups::bump_if_guests_in_it(
            &db, &slot.id, date, &removed, now,
        )?);
        for id in &removed {
            statements.push(db::guests::delete(&db, &slot.id, id)?);
        }
    }
    for guest in &added {
        statements.push(db::guests::insert(&db, &slot.id, date, guest)?);
    }
    if !statements.is_empty() {
        db::batch(&db, statements).await?;
    }
    json(
        200,
        &GuestList {
            guests: kept.into_iter().map(GuestView::from).collect(),
        },
    )
}
