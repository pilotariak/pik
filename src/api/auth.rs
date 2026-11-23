// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Who calls: the player token, the `Origin` check of cookie writes, slot membership and the
//! organizer role (kancha ADR-0002 and ADR-0004, both still proposed).
//!
//! The token comes from `Authorization: Bearer` (native app) or from the cookie that pik sets
//! (web), the header first. Only its SHA-256 is looked up, and it is never logged.

use jiff::Timestamp;
use worker::{D1Database, Env, Method, Request, Response};

use super::cors::AllowedOrigins;
use super::problem::{ApiResult, Problem, ProblemType};
use crate::db;
use crate::domain::ids::{PlayerId, SlotId};
use crate::domain::slot::Member;
use crate::domain::token::Token;

/// The token cookie, which pik sets on its own origin: the Kancha website calls pik there with
/// credentialed CORS (ADR-0004, `cors.rs`). `__Secure-` makes browsers refuse it without `Secure`
/// or from an origin that is not secure; Chrome counts `http://localhost` as secure, so `make dev`
/// keeps it (docs/how-to/howto-api.md). `SameSite=Lax` still sends it on the website's calls:
/// `kancha.pilotariak.com` and `pik.pilotariak.com` are the same site, and so are two ports of
/// `localhost`. `Path=/api` covers every API version.
pub(crate) const COOKIE: &str = "__Secure-pik_token";

/// Browsers keep a cookie at most 400 days.
const COOKIE_MAX_AGE: u32 = 400 * 24 * 3600;

/// Where the token came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Bearer,
    Cookie,
}

/// A token as sent: `None` when it cannot be a token, which counts as an unknown one.
struct Presented {
    token: Option<Token>,
    source: Source,
}

/// The value of the token cookie in a `Cookie` header.
fn cookie_value(header: &str) -> Option<&str> {
    header.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == COOKIE).then_some(value)
    })
}

/// The token sent with the request, if any, after the `Origin` check of a cookie write: its
/// `Origin` must be one of the `ALLOWED_ORIGINS` that CORS answers (`cors.rs`). The native app
/// sends `Authorization`, which needs no `Origin` check.
fn presented(req: &Request, env: &Env) -> ApiResult<Option<Presented>> {
    let headers = req.headers();
    if let Some(authorization) = headers.get("Authorization")? {
        let token = authorization
            .strip_prefix("Bearer ")
            .and_then(|value| Token::parse(value.trim()));
        return Ok(Some(Presented {
            token,
            source: Source::Bearer,
        }));
    }
    let Some(value) = headers
        .get("Cookie")?
        .and_then(|header| cookie_value(&header).map(str::to_owned))
    else {
        return Ok(None);
    };
    let write = !matches!(req.method(), Method::Get | Method::Head);
    if write {
        let origin = headers.get("Origin")?;
        if !origin.is_some_and(|origin| AllowedOrigins::from_env(env).allows(&origin)) {
            return Err(Problem::new(ProblemType::OriginNotAllowed));
        }
    }
    Ok(Some(Presented {
        token: Token::parse(&value),
        source: Source::Cookie,
    }))
}

/// 401 for an unknown token. A cookie with an unknown token, from a device whose player was
/// purged (#5), is deleted, so that the next call is anonymous.
fn unknown(source: Source) -> Problem {
    match source {
        Source::Cookie => Problem::unauthorized_clearing_cookie(),
        Source::Bearer => Problem::new(ProblemType::Unauthorized),
    }
}

/// The player of the request: 401 without a token or with an unknown one.
pub(crate) async fn player(
    req: &Request,
    env: &Env,
    db: &D1Database,
    now: Timestamp,
) -> ApiResult<PlayerId> {
    let Some(presented) = presented(req, env)? else {
        return Err(Problem::new(ProblemType::Unauthorized));
    };
    let Some(token) = presented.token else {
        return Err(unknown(presented.source));
    };
    db::players::find_by_token(db, &token.hash(), now)
        .await?
        .ok_or_else(|| unknown(presented.source))
}

/// How a route without a required token treats an unknown one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnknownToken {
    /// 401, as on every route but the join routes.
    Reject,
    /// Like no token: the join routes create a new player, since the player of a device can be
    /// purged (#5).
    AsNone,
}

/// The player of the request if it sent a token, `None` without one.
pub(crate) async fn optional_player(
    req: &Request,
    env: &Env,
    db: &D1Database,
    now: Timestamp,
    unknown_token: UnknownToken,
) -> ApiResult<Option<PlayerId>> {
    let Some(presented) = presented(req, env)? else {
        return Ok(None);
    };
    let found = match &presented.token {
        Some(token) => db::players::find_by_token(db, &token.hash(), now).await?,
        None => None,
    };
    match (found, unknown_token) {
        (Some(player), _) => Ok(Some(player)),
        (None, UnknownToken::AsNone) => Ok(None),
        (None, UnknownToken::Reject) => Err(unknown(presented.source)),
    }
}

/// The caller's active member of a slot: 404 when the slot does not exist or the caller is not
/// a member, so that a 403 does not tell that the slot exists.
pub(crate) async fn member(db: &D1Database, slot: &SlotId, player: &PlayerId) -> ApiResult<Member> {
    db::members::of_player_in_slot(db, slot, player)
        .await?
        .ok_or_else(|| Problem::new(ProblemType::NotFound))
}

/// 403 unless the member is an organizer.
pub(crate) fn organizer(member: &Member) -> ApiResult<()> {
    if member.is_organizer() {
        Ok(())
    } else {
        Err(Problem::new(ProblemType::OrganizerOnly))
    }
}

/// The `Set-Cookie` value that stores a new token.
fn token_cookie(token: &Token) -> String {
    format!(
        "{COOKIE}={}; Path=/api; Max-Age={COOKIE_MAX_AGE}; HttpOnly; Secure; SameSite=Lax",
        token.expose()
    )
}

/// The `Set-Cookie` value that deletes the token cookie.
pub(crate) fn clear_cookie() -> String {
    format!("{COOKIE}=; Path=/api; Max-Age=0; HttpOnly; Secure; SameSite=Lax")
}

/// Sets the cookie of a new token on a response. `append`, never `set`: one `Set-Cookie` must
/// not replace another.
pub(crate) fn set_token_cookie(response: &mut Response, token: &Token) -> ApiResult<()> {
    response
        .headers_mut()
        .append("Set-Cookie", &token_cookie(token))?;
    Ok(())
}

/// A player created by this request, and its token, returned once.
pub(crate) struct NewPlayer {
    pub(crate) id: PlayerId,
    pub(crate) token: Token,
}

impl NewPlayer {
    pub(crate) fn generate() -> ApiResult<Self> {
        Ok(Self {
            id: PlayerId::generate()?,
            token: Token::generate()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{COOKIE, cookie_value};

    #[test]
    fn reads_the_token_cookie_among_others() {
        let header = format!("theme=dark; {COOKIE}=abc; other=1");
        assert_eq!(cookie_value(&header), Some("abc"), "the token cookie");
        assert_eq!(
            cookie_value("theme=dark; pik_token=abc"),
            None,
            "a cookie without the __Secure- prefix is not the token"
        );
        assert_eq!(cookie_value(""), None, "no cookie");
    }
}
