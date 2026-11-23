// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Errors as RFC 9457 problem details (`application/problem+json`).
//!
//! Each error has a stable `type` URI, which Kancha maps to a translated message. The `detail`
//! says what to fix in English, for developers; it never holds a token or another member's data.

use serde::Serialize;
use worker::{Error, Response, console_error};

use super::auth;
use crate::domain::random::RandomError;

/// The media type of every error response.
pub(crate) const PROBLEM_JSON: &str = "application/problem+json";

macro_rules! problem_types {
    ($($(#[$doc:meta])* $variant:ident = ($uri:literal, $status:literal, $title:literal)),* $(,)?) => {
        /// The `type` of a problem: one per error, stable across versions.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
        #[cfg_attr(test, derive(utoipa::ToSchema))]
        pub(crate) enum ProblemType {
            $($(#[$doc])* #[serde(rename = $uri)] $variant,)*
        }

        impl ProblemType {
            pub(crate) fn status(self) -> u16 {
                match self {
                    $(Self::$variant => $status,)*
                }
            }

            fn title(self) -> &'static str {
                match self {
                    $(Self::$variant => $title,)*
                }
            }
        }
    };
}

problem_types! {
    /// No token, or a token that is not a player's.
    Unauthorized = ("https://pik.pilotariak.com/problems/unauthorized", 401, "A player token is required"),
    /// A write with the cookie whose `Origin` is not Kancha's.
    OriginNotAllowed = ("https://pik.pilotariak.com/problems/origin-not-allowed", 403, "This origin cannot write with the cookie"),
    /// A member who is not an organizer called an organizer route.
    OrganizerOnly = ("https://pik.pilotariak.com/problems/organizer-only", 403, "Only an organizer can do this"),
    /// The resource does not exist, or the caller is not a member of the slot.
    NotFound = ("https://pik.pilotariak.com/problems/not-found", 404, "Not found"),
    /// The path exists with other methods.
    MethodNotAllowed = ("https://pik.pilotariak.com/problems/method-not-allowed", 405, "Method not allowed"),
    /// Another member of the slot has this name, or the name was claimed.
    NameTaken = ("https://pik.pilotariak.com/problems/name-taken", 409, "This name is taken"),
    /// The organizer closed the list of names.
    ListClosed = ("https://pik.pilotariak.com/problems/list-closed", 409, "The list of names is closed"),
    /// The organizer cancelled this session.
    SessionCancelled = ("https://pik.pilotariak.com/problems/session-cancelled", 409, "This session is cancelled"),
    /// After the deadline, only a KO answer is accepted.
    DeadlinePassed = ("https://pik.pilotariak.com/problems/deadline-passed", 409, "The answer deadline has passed"),
    /// The slot would be left without an organizer.
    LastOrganizer = ("https://pik.pilotariak.com/problems/last-organizer", 409, "A slot keeps at least one organizer"),
    /// This device is already a member of the slot.
    AlreadyMember = ("https://pik.pilotariak.com/problems/already-member", 409, "This device is already a member of the slot"),
    /// The recovery link was used or has expired.
    RecoveryLinkGone = ("https://pik.pilotariak.com/problems/recovery-link-gone", 410, "This recovery link was used or has expired"),
    /// The lineup changed since the `ETag` the client sent.
    LineupChanged = ("https://pik.pilotariak.com/problems/lineup-changed", 412, "The lineup changed meanwhile"),
    /// A lineup write needs `If-Match`.
    IfMatchRequired = ("https://pik.pilotariak.com/problems/if-match-required", 428, "If-Match is required"),
    /// The body, a parameter or a header is invalid.
    InvalidRequest = ("https://pik.pilotariak.com/problems/invalid-request", 422, "The request is invalid"),
    /// The date is off the recurrence of the slot or outside its season.
    NotASession = ("https://pik.pilotariak.com/problems/not-a-session", 422, "No session falls on this date"),
    /// The `Idempotency-Key` was used for another request.
    IdempotencyKeyReused = ("https://pik.pilotariak.com/problems/idempotency-key-reused", 422, "This Idempotency-Key was used for another request"),
    /// Too many requests: retry after `Retry-After` seconds.
    RateLimited = ("https://pik.pilotariak.com/problems/rate-limited", 429, "Too many requests"),
    /// pik failed.
    Internal = ("https://pik.pilotariak.com/problems/internal", 500, "Internal error"),
}

/// The body of an error response.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(crate) struct ProblemDocument {
    #[serde(rename = "type")]
    kind: ProblemType,
    title: &'static str,
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

/// An error of a route.
#[derive(Debug)]
pub(crate) struct Problem {
    kind: ProblemType,
    detail: Option<String>,
    retry_after: Option<u32>,
    /// The 401 of an unknown token sent in the cookie also deletes the cookie: page scripts
    /// cannot, since it is `HttpOnly`.
    clear_cookie: bool,
}

/// The result of a route.
pub(crate) type ApiResult<T> = Result<T, Problem>;

impl Problem {
    pub(crate) fn new(kind: ProblemType) -> Self {
        Self {
            kind,
            detail: None,
            retry_after: None,
            clear_cookie: false,
        }
    }

    /// A problem with a `detail`.
    pub(crate) fn detail(kind: ProblemType, detail: impl Into<String>) -> Self {
        Self {
            detail: Some(detail.into()),
            ..Self::new(kind)
        }
    }

    /// 422 `invalid-request` with what is wrong.
    pub(crate) fn invalid(detail: impl Into<String>) -> Self {
        Self::detail(ProblemType::InvalidRequest, detail)
    }

    /// 429 with `Retry-After`.
    pub(crate) fn rate_limited(retry_after: u32) -> Self {
        Self {
            retry_after: Some(retry_after),
            ..Self::new(ProblemType::RateLimited)
        }
    }

    /// 401 that also deletes the token cookie.
    pub(crate) fn unauthorized_clearing_cookie() -> Self {
        Self {
            clear_cookie: true,
            ..Self::new(ProblemType::Unauthorized)
        }
    }

    /// The response, with `Content-Type: application/problem+json`.
    pub(crate) fn into_response(self) -> worker::Result<Response> {
        let status = self.kind.status();
        let mut response = Response::from_json(&ProblemDocument {
            kind: self.kind,
            title: self.kind.title(),
            status,
            detail: self.detail,
        })?
        .with_status(status);
        let headers = response.headers_mut();
        headers.set("Content-Type", PROBLEM_JSON)?;
        if let Some(seconds) = self.retry_after {
            headers.set("Retry-After", &seconds.to_string())?;
        }
        if self.clear_cookie {
            headers.append("Set-Cookie", &auth::clear_cookie())?;
        }
        Ok(response)
    }
}

/// Logs what failed without request data: a D1 message names a table or a constraint, never a
/// bound value. Other errors are logged by kind only, since a deserialization message can quote
/// a stored value.
fn log(error: &Error) {
    match error {
        Error::D1(_) | Error::JsError(_) | Error::RustError(_) => {
            console_error!("internal error: {error}");
        }
        other => {
            let kind = format!("{other:?}");
            let kind = kind.split(['(', ' ', '{']).next().unwrap_or("unknown");
            console_error!("internal error of kind {kind}");
        }
    }
}

impl From<Error> for Problem {
    fn from(error: Error) -> Self {
        log(&error);
        Self::new(ProblemType::Internal)
    }
}

impl From<RandomError> for Problem {
    fn from(error: RandomError) -> Self {
        console_error!("internal error: {error}");
        Self::new(ProblemType::Internal)
    }
}

/// Reusable error responses of the OpenAPI document. Only their types are read, by the
/// `ToResponse` derive.
#[cfg(test)]
#[expect(
    dead_code,
    reason = "the responses are never built, utoipa reads their types"
)]
pub(crate) mod responses {
    use super::ProblemDocument;

    /// No token, or an unknown token.
    #[derive(utoipa::ToResponse)]
    #[response(content_type = "application/problem+json")]
    pub(crate) struct Unauthorized(ProblemDocument);

    /// The caller is not an organizer of the slot, or wrote with the cookie from an origin that
    /// is not Kancha's.
    #[derive(utoipa::ToResponse)]
    #[response(content_type = "application/problem+json")]
    pub(crate) struct Forbidden(ProblemDocument);

    /// Not found, or the caller is not a member of the slot: a 403 would tell that it exists.
    #[derive(utoipa::ToResponse)]
    #[response(content_type = "application/problem+json")]
    pub(crate) struct NotFound(ProblemDocument);

    /// Name taken, list closed, session cancelled, deadline passed, last organizer, or device
    /// already a member.
    #[derive(utoipa::ToResponse)]
    #[response(content_type = "application/problem+json")]
    pub(crate) struct Conflict(ProblemDocument);

    /// The recovery link was used or has expired.
    #[derive(utoipa::ToResponse)]
    #[response(content_type = "application/problem+json")]
    pub(crate) struct Gone(ProblemDocument);

    /// `If-Match` does not match the current lineup version.
    #[derive(utoipa::ToResponse)]
    #[response(content_type = "application/problem+json")]
    pub(crate) struct PreconditionFailed(ProblemDocument);

    /// `If-Match` is missing.
    #[derive(utoipa::ToResponse)]
    #[response(content_type = "application/problem+json")]
    pub(crate) struct PreconditionRequired(ProblemDocument);

    /// Invalid body, parameter or header, a date that is not a session of the slot, or an
    /// `Idempotency-Key` reused with another request.
    #[derive(utoipa::ToResponse)]
    #[response(content_type = "application/problem+json")]
    pub(crate) struct Unprocessable(ProblemDocument);

    /// Rate limit reached.
    #[derive(utoipa::ToResponse)]
    #[response(
        content_type = "application/problem+json",
        headers(("Retry-After" = u32, description = "Seconds to wait before retrying"))
    )]
    pub(crate) struct TooManyRequests(ProblemDocument);
}
