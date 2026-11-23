// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! pik, the slots service behind Kancha's Créneaux tab: a Cloudflare Worker with a D1 database.
//!
//! `api` holds the REST API under `/api/v1/`, one module per resource, `db` the D1 queries, one
//! module per table, `domain` the pure logic and types, and `web` the public pages outside the
//! API. This file wires them into the Worker entry point, and adds after the router what every
//! response needs.

mod api;
mod db;
mod domain;
mod web;

use worker::{Context, Env, Request, Response, Result, Router, event};

use api::problem::Problem;

/// Security headers of every response. pik sets its own: when Kancha web forwards `/api/*`
/// (kancha ADR-0004), Pages does not apply its `_headers` file to these responses. A header the
/// route already set, like the landing page's `Content-Security-Policy`, is kept.
const SECURITY_HEADERS: &[(&str, &str)] = &[
    (
        "Content-Security-Policy",
        "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
    ),
    ("Cross-Origin-Resource-Policy", "same-origin"),
    ("Referrer-Policy", "no-referrer"),
    (
        "Strict-Transport-Security",
        "max-age=31536000; includeSubDomains",
    ),
    ("X-Content-Type-Options", "nosniff"),
    ("X-Frame-Options", "DENY"),
];

// The workers-rs Router answers 405 to any other method on a registered path, and 404 to an
// unknown path: `api::finish` turns both into problems under /api/.
#[event(fetch)]
async fn fetch(request: Request, env: Env, _context: Context) -> Result<Response> {
    let is_api = request.path().starts_with("/api/");
    let router = web::routes(api::routes(Router::new()));
    let response = match router.run(request, env).await {
        Ok(response) => response,
        Err(error) => Problem::from(error).into_response()?,
    };
    let mut response = if is_api {
        api::finish(response)?
    } else {
        response
    };
    let headers = response.headers_mut();
    for (name, value) in SECURITY_HEADERS {
        if !headers.has(name)? {
            headers.set(name, value)?;
        }
    }
    Ok(response)
}
