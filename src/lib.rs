// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

use serde::Serialize;
use worker::{Context, Env, Request, Response, Result, Router, event};

/// Landing page served at `GET /`: plain HTML with inline CSS and script, no build step.
const LANDING_TEMPLATE: &str = include_str!("landing.html");

/// Replaced by the crate version, which release-please bumps in Cargo.toml.
const VERSION_PLACEHOLDER: &str = "__PIK_VERSION__";

/// Azkena's policy plus `connect-src 'self'`, since the status script fetches /api/v1/health.
/// Everything is inline except Google Fonts, hence `'unsafe-inline'` for styles and script.
const LANDING_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; \
    style-src 'unsafe-inline' https://fonts.googleapis.com; font-src https://fonts.gstatic.com; \
    img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'";

/// Unlike the API, the landing page holds no private data, so it may be cached briefly.
const LANDING_CACHE_CONTROL: &str = "public, max-age=300";

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

fn landing_html() -> String {
    LANDING_TEMPLATE.replace(VERSION_PLACEHOLDER, env!("CARGO_PKG_VERSION"))
}

fn landing() -> Result<Response> {
    // from_html sets Content-Type: text/html; charset=utf-8.
    let mut res = Response::from_html(landing_html())?;
    let headers = res.headers_mut();
    headers.set("Content-Security-Policy", LANDING_CSP)?;
    headers.set("Cache-Control", LANDING_CACHE_CONTROL)?;
    Ok(res)
}

// Every API route lives under /api/v1/: Kancha web forwards /api/* unchanged, and a breaking
// change goes to /api/v2/ while installed app versions keep calling /api/v1/. Only the landing
// page lives outside it. The Router answers 405 to any other method on a registered path.
#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    Router::new()
        .get("/", |_, _| landing())
        .get("/api/v1/health", |_, _| {
            Response::from_json(&Health { status: "ok" })
        })
        .run(req, env)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_html_shows_the_crate_version() {
        assert!(LANDING_TEMPLATE.contains(VERSION_PLACEHOLDER));
        let html = landing_html();
        assert!(!html.contains(VERSION_PLACEHOLDER));
        assert!(html.contains(&format!("v{}", env!("CARGO_PKG_VERSION"))));
    }
}
