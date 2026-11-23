// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! The REST API, one module per resource. Every route lives under `/api/v1/`.

mod health;

use worker::Router;

/// Prefixes a path with `/api/v1`, the only place the API version appears. Kancha web forwards
/// `/api/*` unchanged, and a breaking change goes to `/api/v2/` while installed app versions
/// keep calling `/api/v1/`.
macro_rules! v1 {
    ($path:literal) => {
        concat!("/api/v1", $path)
    };
}

/// Registers the API routes on the router.
pub(crate) fn routes(router: Router<'_, ()>) -> Router<'_, ()> {
    router.get(v1!("/health"), |_, _| health::get())
}
