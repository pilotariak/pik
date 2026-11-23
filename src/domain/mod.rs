// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Pure logic and types: no I/O, no clock, and no `worker` import, so `cargo test` runs them on
//! the host.

// The unit tests call every function, but the Worker does not yet: the slots API (#4) is the
// first caller, and the expectation fails once it uses them all.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "no route calls the calendar before the slots API (#4)"
    )
)]
pub(crate) mod calendar;
