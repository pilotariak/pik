// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! D1 queries, one module per table, returning `domain` types.
//!
//! Values are bound as plain JavaScript values: text as strings, integers and instants
//! (milliseconds since the epoch) as numbers, hashes as `Uint8Array`, which D1 stores as BLOB.
//! A statement that must run with others in one transaction is returned unexecuted, for
//! [`batch`].

pub(crate) mod answers;
pub(crate) mod guests;
pub(crate) mod idempotency_keys;
pub(crate) mod lineup_entries;
pub(crate) mod lineups;
pub(crate) mod members;
pub(crate) mod players;
pub(crate) mod recoveries;
pub(crate) mod session_overrides;
pub(crate) mod slots;

use jiff::Timestamp;
use jiff::civil::{Date, Time};
use worker::js_sys::Uint8Array;
use worker::wasm_bindgen::JsValue;
use worker::{D1Database, D1PreparedStatement, D1Result, Error, Result};

use crate::domain::civil::{format_time, parse_date, parse_time};

/// A text parameter.
pub(crate) fn text(value: &str) -> JsValue {
    JsValue::from_str(value)
}

/// A nullable text parameter.
pub(crate) fn opt_text(value: Option<&str>) -> JsValue {
    value.map_or(JsValue::NULL, JsValue::from_str)
}

/// An integer parameter. D1 takes JavaScript numbers, not `BigInt`.
#[expect(
    clippy::cast_precision_loss,
    reason = "counts and millisecond instants stay far below 2^53"
)]
pub(crate) fn int(value: i64) -> JsValue {
    JsValue::from_f64(value as f64)
}

/// A nullable integer parameter.
pub(crate) fn opt_int(value: Option<i64>) -> JsValue {
    value.map_or(JsValue::NULL, int)
}

/// A boolean, stored as 0 or 1.
pub(crate) fn flag(value: bool) -> JsValue {
    int(i64::from(value))
}

/// A BLOB parameter. A plain array would not compare equal to a stored BLOB.
pub(crate) fn blob(bytes: &[u8]) -> JsValue {
    Uint8Array::from(bytes).into()
}

/// A local date parameter, `YYYY-MM-DD`.
pub(crate) fn date(value: Date) -> JsValue {
    text(&value.to_string())
}

/// A local time parameter, `HH:MM`.
pub(crate) fn time(value: Time) -> JsValue {
    text(&format_time(value))
}

/// An instant parameter, in milliseconds since the epoch.
pub(crate) fn instant(value: Timestamp) -> JsValue {
    int(value.as_millisecond())
}

/// A nullable instant parameter.
pub(crate) fn opt_instant(value: Option<Timestamp>) -> JsValue {
    value.map_or(JsValue::NULL, instant)
}

/// A row that the schema should not allow. The message names the column, never its value: a
/// value can be player data.
pub(crate) fn corrupt(column: &str) -> Error {
    Error::RustError(format!("unexpected value in column {column}"))
}

/// A stored `YYYY-MM-DD` date.
pub(crate) fn read_date(column: &str, value: &str) -> Result<Date> {
    parse_date(value).ok_or_else(|| corrupt(column))
}

/// A stored `HH:MM` time.
pub(crate) fn read_time(column: &str, value: &str) -> Result<Time> {
    parse_time(value).ok_or_else(|| corrupt(column))
}

/// A stored instant.
pub(crate) fn read_instant(column: &str, millis: i64) -> Result<Timestamp> {
    Timestamp::from_millisecond(millis).map_err(|_| corrupt(column))
}

/// A stored non-negative count.
pub(crate) fn read_count(column: &str, value: i64) -> Result<u32> {
    u32::try_from(value).map_err(|_| corrupt(column))
}

/// A statement with its parameters bound.
pub(crate) fn statement(
    db: &D1Database,
    sql: &str,
    values: &[JsValue],
) -> Result<D1PreparedStatement> {
    db.prepare(sql).bind(values)
}

/// Runs statements in one transaction: if one fails, none applies.
pub(crate) async fn batch(
    db: &D1Database,
    statements: Vec<D1PreparedStatement>,
) -> Result<Vec<D1Result>> {
    db.batch(statements).await
}

/// Rows changed by a write.
pub(crate) fn changes(result: &D1Result) -> Result<usize> {
    Ok(result
        .meta()?
        .and_then(|meta| meta.changes)
        .unwrap_or_default())
}

/// Whether a D1 error is a constraint failure (UNIQUE, NOT NULL, FOREIGN KEY, CHECK). Only the
/// constraint is in the message, never a bound value.
pub(crate) fn is_constraint(error: &Error) -> bool {
    error.to_string().contains("constraint failed")
}
