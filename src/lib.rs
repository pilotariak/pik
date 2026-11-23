// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

pub mod calendar;

use serde::Serialize;
use worker::{Context, Env, Request, Response, Result, Router, event};

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

// Every pik route lives under /api/v1/: Kancha web forwards /api/* unchanged, and a breaking
// change goes to /api/v2/ while installed app versions keep calling /api/v1/.
#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    Router::new()
        .get("/api/v1/health", |_, _| {
            Response::from_json(&Health { status: "ok" })
        })
        .run(req, env)
        .await
}
