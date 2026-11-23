// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! CORS of the API, for the Kancha website (kancha ADR-0004, still proposed): its pages call pik
//! on pik's own origin with the token cookie, so the browser needs CORS with credentials.
//!
//! One list, the `ALLOWED_ORIGINS` Worker variable, drives CORS and the `Origin` check of cookie
//! writes in `auth.rs`. An allowed origin is echoed in `Access-Control-Allow-Origin`, never `*`,
//! which browsers refuse with credentials. A preflight (`OPTIONS`) from an allowed origin gets
//! 204 before the router, which would answer 405; any other `OPTIONS` gets that 405, without
//! CORS headers. Outside `/api/`, pik sends no CORS header.

use worker::{Env, Method, Request, Response, console_error};

/// The Worker variable of the allowed origins: exact origins, separated by commas, such as
/// `https://kancha.pilotariak.com`. `[vars]` of wrangler.toml holds the production list, and the
/// `dev` script of package.json replaces it with `--var`, which always passes a string. Preview
/// deployments of the website need their own pik environment with their own list.
const ALLOWED_ORIGINS: &str = "ALLOWED_ORIGINS";

/// The methods of the API routes. `GET`, `HEAD` and `POST` need no listing, but listing them
/// costs nothing.
const ALLOW_METHODS: &str = "GET, POST, PUT, PATCH, DELETE";

/// The request headers that Kancha sends and that are not CORS-safelisted: `Content-Type:
/// application/json` is not either.
const ALLOW_HEADERS: &str = "Content-Type, Idempotency-Key, Authorization, If-Match";

/// The response headers that Kancha reads beyond the CORS-safelisted ones: the wait of a 429 and
/// the lineup version of a session.
const EXPOSE_HEADERS: &str = "Retry-After, ETag";

/// How long a browser may reuse a preflight, in seconds: two hours, the cap of Chromium.
const MAX_AGE_SECONDS: &str = "7200";

/// The origins whose pages may call the API with the cookie.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AllowedOrigins(Vec<String>);

impl AllowedOrigins {
    /// The list of the `ALLOWED_ORIGINS` variable. Without it, or when it is not a string, no
    /// origin is allowed: the website gets no CORS header and its cookie writes get 403, while
    /// the native app, which sends no `Origin`, keeps working.
    pub(crate) fn from_env(env: &Env) -> Self {
        match env.var(ALLOWED_ORIGINS) {
            Ok(value) => Self::parse(&value.to_string()),
            Err(error) => {
                console_error!("no origin is allowed: {error}");
                Self::default()
            }
        }
    }

    /// Origins separated by commas, blanks around them ignored.
    fn parse(value: &str) -> Self {
        Self(
            value
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    }

    /// Whether the `Origin` a browser sent is in the list. The match is exact, so that
    /// `https://kancha.pilotariak.com.evil.example` or `null` never passes.
    pub(crate) fn allows(&self, origin: &str) -> bool {
        self.0.iter().any(|allowed| allowed == origin)
    }
}

/// The CORS side of an API request, read before the router takes the request.
#[derive(Debug)]
pub(crate) struct Cors {
    /// The request's `Origin` when it is allowed, echoed in `Access-Control-Allow-Origin`.
    origin: Option<String>,
    /// An `OPTIONS` request.
    preflight: bool,
}

impl Cors {
    /// The CORS side of a request. The variable is read only for a request with an `Origin`.
    pub(crate) fn of(req: &Request, env: &Env) -> worker::Result<Self> {
        let origin = req
            .headers()
            .get("Origin")?
            .filter(|origin| AllowedOrigins::from_env(env).allows(origin));
        Ok(Self {
            origin,
            preflight: matches!(req.method(), Method::Options),
        })
    }

    /// A preflight from an allowed origin, which [`Cors::preflight`] answers before the router.
    pub(crate) fn answers_preflight(&self) -> bool {
        self.preflight && self.origin.is_some()
    }

    /// The answer to a preflight: 204, whose headers [`Cors::apply`] adds.
    pub(crate) fn preflight() -> worker::Result<Response> {
        Ok(Response::empty()?.with_status(204))
    }

    /// Adds the CORS headers to an API response, problems included: without them, the browser
    /// hides the problem document from the page.
    pub(crate) fn apply(&self, response: &mut Response) -> worker::Result<()> {
        let headers = response.headers_mut();
        // The response depends on `Origin`, whether it is allowed or not. `append`, so that a
        // `Vary` set by a route is kept.
        headers.append("Vary", "Origin")?;
        let Some(origin) = &self.origin else {
            return Ok(());
        };
        headers.set("Access-Control-Allow-Origin", origin)?;
        headers.set("Access-Control-Allow-Credentials", "true")?;
        if self.preflight {
            headers.set("Access-Control-Allow-Methods", ALLOW_METHODS)?;
            headers.set("Access-Control-Allow-Headers", ALLOW_HEADERS)?;
            headers.set("Access-Control-Max-Age", MAX_AGE_SECONDS)?;
        } else {
            headers.set("Access-Control-Expose-Headers", EXPOSE_HEADERS)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::AllowedOrigins;

    #[test]
    fn parses_origins_separated_by_commas() {
        let origins =
            AllowedOrigins::parse(" https://kancha.pilotariak.com ,http://localhost:8081,");
        assert_eq!(
            origins,
            AllowedOrigins(vec![
                "https://kancha.pilotariak.com".to_owned(),
                "http://localhost:8081".to_owned(),
            ]),
            "blanks and empty entries are dropped"
        );
        assert_eq!(
            AllowedOrigins::parse(""),
            AllowedOrigins::default(),
            "an empty variable allows no origin"
        );
    }

    #[test]
    fn allows_only_exact_origins() {
        let origins = AllowedOrigins::parse("https://kancha.pilotariak.com");
        assert!(
            origins.allows("https://kancha.pilotariak.com"),
            "the listed origin"
        );
        for origin in [
            "https://kancha.pilotariak.com.evil.example",
            "https://kancha.pilotariak.com/",
            "http://kancha.pilotariak.com",
            "https://KANCHA.pilotariak.com",
            "https://kancha.pilotariak.com:8443",
            "http://localhost:8081",
            "null",
            "",
        ] {
            assert!(!origins.allows(origin), "{origin:?} is not listed");
        }
        assert!(
            !AllowedOrigins::default().allows("null"),
            "an empty list allows nothing, not even null"
        );
    }
}
