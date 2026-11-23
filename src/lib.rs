// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! pik, the slots service behind Kancha's Créneaux tab: a Cloudflare Worker with a D1 database.
//!
//! `api` holds the REST API under `/api/v1/`, one module per resource, and `web` the public
//! pages outside it. This file only wires them into the Worker entry points.

mod api;
mod web;

use worker::{Context, Env, Request, Response, Result, Router, event};

// The workers-rs Router answers 405 to any other method on a registered path, and 404 to an
// unknown path.
#[event(fetch)]
async fn fetch(request: Request, env: Env, _context: Context) -> Result<Response> {
    let router = web::routes(api::routes(Router::new()));
    router.run(request, env).await
}
