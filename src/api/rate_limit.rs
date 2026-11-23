// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Rate limits of slot creation, name claims and recovery redemptions, with the Workers Rate
//! Limiting binding (`[[ratelimits]]` in wrangler.toml).

use worker::{Env, Request};

use super::problem::{ApiResult, Problem};

/// The period of every limit in wrangler.toml, in seconds: the binding allows 10 or 60. It
/// does not say when the window ends, so `Retry-After` is the whole period.
const PERIOD_SECONDS: u32 = 60;

/// A rate-limited action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Limit {
    /// `POST /slots`.
    CreateSlot,
    /// `POST /join/:code/claim` and `POST /join/:code/members`: each can create a player.
    Claim,
    /// `POST /recover/:token`.
    Recover,
}

impl Limit {
    /// The binding name in wrangler.toml, which tests/worker.mjs reads too.
    fn binding(self) -> &'static str {
        match self {
            Self::CreateSlot => "CREATE_SLOT_LIMITER",
            Self::Claim => "CLAIM_LIMITER",
            Self::Recover => "RECOVER_LIMITER",
        }
    }
}

/// 429 with `Retry-After` once the caller's address reached the limit.
///
/// The key is the client address of `CF-Connecting-IP`. A request without it, which a proxy
/// that does not forward it would send, shares one key with all such requests.
pub(crate) async fn check(env: &Env, req: &Request, limit: Limit) -> ApiResult<()> {
    let key = req
        .headers()
        .get("CF-Connecting-IP")?
        .unwrap_or_else(|| "unknown".to_owned());
    let outcome = env.rate_limiter(limit.binding())?.limit(key).await?;
    if outcome.success {
        Ok(())
    } else {
        Err(Problem::rate_limited(PERIOD_SECONDS))
    }
}
