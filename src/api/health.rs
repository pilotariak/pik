// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! `GET /api/v1/health`: liveness check.

use serde::Serialize;
use worker::{Response, Result};

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

/// Answers `{"status":"ok"}`.
pub(super) fn get() -> Result<Response> {
    Response::from_json(&Health { status: "ok" })
}
