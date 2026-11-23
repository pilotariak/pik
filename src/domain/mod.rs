// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Pure logic and types: no I/O, no clock, and no `worker` import, so `cargo test` runs them on
//! the host. The one exception to "no I/O" is [`random`], which reads the platform CSPRNG.

pub(crate) mod calendar;
pub(crate) mod civil;
pub(crate) mod ids;
pub(crate) mod invite;
pub(crate) mod lineup;
pub(crate) mod name;
pub(crate) mod random;
pub(crate) mod slot;
pub(crate) mod token;
