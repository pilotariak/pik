// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! The OpenAPI 3.1 document of the API, committed as `openapi.json`: the contract between pik and
//! Kancha, which generates its types from it.
//!
//! utoipa builds it from the `#[utoipa::path]` annotations of the handlers and the `ToSchema`
//! derives of their types. pik does not serve the document, so all of it exists under
//! `cfg(test)` only, with utoipa as a dev-dependency: the Worker's wasm carries no schema code.
//! The annotations are not tied to the router, so [`tests::openapi_json_is_up_to_date`] fails
//! when the generated document differs from the committed file. Regenerate it with
//! `make openapi`.

use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use super::auth::COOKIE;
use super::problem::responses::{
    Conflict, Forbidden, Gone, NotFound, PreconditionFailed, PreconditionRequired, TooManyRequests,
    Unauthorized, Unprocessable,
};
use super::problem::{ProblemDocument, ProblemType};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "pik",
        // The version of the contract, which `/api/v1` carries: not the crate version, which
        // release-please bumps on every release.
        version = "1",
        description = "Slots API of Kancha's Créneaux tab: courts that a group of friends books \
            every week or every other week, and who comes to each session. Paths are under \
            /api/v1. Errors are RFC 9457 problem details with a stable type. Dates in paths are \
            local dates in Europe/Paris; instants in bodies are RFC 3339 in UTC. Every response \
            sends Cache-Control: private, no-store.",
        license(name = "Apache-2.0", identifier = "Apache-2.0"),
    ),
    servers((url = "https://pik.pilotariak.com")),
    paths(
        super::health::get,
        super::slots::create,
        super::slots::me,
        super::slots::get,
        super::slots::update,
        super::slots::delete,
        super::slots::rotate_invite_code,
        super::slots::set_list,
        super::join::preview,
        super::join::claim,
        super::join::add_name,
        super::members::add,
        super::members::update,
        super::members::release,
        super::members::leave,
        super::sessions::list,
        super::sessions::get,
        super::sessions::update,
        super::sessions::answer,
        super::sessions::guests,
        super::lineups::put,
        super::recovery::create,
        super::recovery::redeem,
    ),
    components(
        schemas(ProblemDocument, ProblemType),
        responses(
            Unauthorized,
            Forbidden,
            NotFound,
            Conflict,
            Gone,
            PreconditionFailed,
            PreconditionRequired,
            Unprocessable,
            TooManyRequests,
        ),
    ),
    modifiers(&SecuritySchemes),
    tags(
        (name = "health", description = "Liveness"),
        (name = "slots", description = "Create, read, change and delete slots; the player's slots"),
        (name = "join", description = "Join a slot with its invite code"),
        (name = "members", description = "The list of names of a slot"),
        (name = "sessions", description = "Sessions, answers and guests"),
        (name = "lineups", description = "Selection and pairs of a session"),
        (name = "recovery", description = "Recovery links for a member who lost their device"),
    ),
)]
struct ApiDoc;

/// The two ways to send a player token (kancha ADR-0004).
struct SecuritySchemes;

impl Modify for SecuritySchemes {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some("Player token, for the native app"))
                    .build(),
            ),
        );
        components.add_security_scheme(
            "cookie",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                COOKIE,
                "Player token in an HttpOnly cookie set by pik, for the website. A write with \
                 the cookie must come with an allowed Origin.",
            ))),
        );
    }
}

/// The document as committed: pretty JSON with a final newline.
fn document() -> String {
    let mut json = ApiDoc::openapi()
        .to_pretty_json()
        .expect("should serialize the OpenAPI document");
    json.push('\n');
    json
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::document;

    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/openapi.json");

    /// Set to rewrite openapi.json instead of checking it: `make openapi`.
    const WRITE: &str = "PIK_WRITE_OPENAPI";

    #[test]
    fn openapi_json_is_up_to_date() {
        let generated = document();
        if std::env::var_os(WRITE).is_some() {
            fs::write(PATH, &generated).expect("should write openapi.json");
            return;
        }
        let committed = fs::read_to_string(PATH).unwrap_or_default();
        assert!(
            committed == generated,
            "openapi.json differs from the document generated from the code: run `make openapi` \
             and commit the result"
        );
    }

    #[test]
    fn the_document_is_openapi_3_1_with_every_route() {
        let document: serde_json::Value =
            serde_json::from_str(&document()).expect("should be JSON");
        assert_eq!(document["openapi"], "3.1.0", "OpenAPI 3.1");
        let operations: usize = document["paths"]
            .as_object()
            .expect("should have paths")
            .values()
            .map(|path| path.as_object().expect("should be an object").len())
            .sum();
        assert_eq!(
            operations, 23,
            "the health route and the 22 routes of the slots API"
        );
    }
}
