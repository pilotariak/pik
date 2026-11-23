// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! `members`: a name in a slot, linked to a player once claimed.
//!
//! A member who leaves keeps their row, shown as "Ancien joueur", with their past answers and
//! lineup entries. The writes that could leave a slot without an organizer guard it in the same
//! statement, so that two concurrent requests cannot both pass a check made before.

use jiff::Timestamp;
use serde::Deserialize;
use worker::wasm_bindgen::JsValue;
use worker::{D1Database, D1PreparedStatement, Result};

use super::{changes, corrupt, instant, statement, text};
use crate::domain::ids::{MemberId, PlayerId, SlotId};
use crate::domain::name::DisplayName;
use crate::domain::slot::{Member, Role};

const COLUMNS: &str = "id, slot_id, player_id, display_name, role, left_at";

/// The write may run unless it takes the last organizer away: the member is not a claimed
/// organizer, or another active member is one. `?1` is the slot, `?2` the member.
const KEEPS_AN_ORGANIZER: &str = "(role <> 'organizer' OR player_id IS NULL OR EXISTS (\
    SELECT 1 FROM members AS other WHERE other.slot_id = ?1 AND other.id <> ?2 \
    AND other.role = 'organizer' AND other.left_at IS NULL AND other.player_id IS NOT NULL))";

#[derive(Deserialize)]
struct Row {
    id: MemberId,
    slot_id: SlotId,
    player_id: Option<PlayerId>,
    display_name: Option<String>,
    role: String,
    left_at: Option<i64>,
}

impl TryFrom<Row> for Member {
    type Error = worker::Error;

    fn try_from(row: Row) -> Result<Self> {
        Ok(Self {
            id: row.id,
            slot_id: row.slot_id,
            player_id: row.player_id,
            name: row.display_name,
            role: row.role.parse().map_err(|()| corrupt("members.role"))?,
            left: row.left_at.is_some(),
        })
    }
}

fn rows(rows: Vec<Row>) -> Result<Vec<Member>> {
    rows.into_iter().map(Member::try_from).collect()
}

/// The member with this id in this slot, whether they left or not.
pub(crate) async fn find(db: &D1Database, slot: &SlotId, id: &MemberId) -> Result<Option<Member>> {
    let sql = format!("SELECT {COLUMNS} FROM members WHERE slot_id = ? AND id = ?");
    statement(db, &sql, &[text(slot.as_str()), text(id.as_str())])?
        .first::<Row>(None)
        .await?
        .map(Member::try_from)
        .transpose()
}

/// The member with this id, in any slot.
pub(crate) async fn find_any(db: &D1Database, id: &MemberId) -> Result<Option<Member>> {
    let sql = format!("SELECT {COLUMNS} FROM members WHERE id = ?");
    statement(db, &sql, &[text(id.as_str())])?
        .first::<Row>(None)
        .await?
        .map(Member::try_from)
        .transpose()
}

/// The active member of this player in this slot.
pub(crate) async fn of_player_in_slot(
    db: &D1Database,
    slot: &SlotId,
    player: &PlayerId,
) -> Result<Option<Member>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM members WHERE slot_id = ? AND player_id = ? AND left_at IS NULL"
    );
    statement(db, &sql, &[text(slot.as_str()), text(player.as_str())])?
        .first::<Row>(None)
        .await?
        .map(Member::try_from)
        .transpose()
}

/// The active members of this player, one per slot, oldest first.
pub(crate) async fn of_player(db: &D1Database, player: &PlayerId) -> Result<Vec<Member>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM members WHERE player_id = ? AND left_at IS NULL \
         ORDER BY created_at, rowid"
    );
    rows(
        statement(db, &sql, &[text(player.as_str())])?
            .all()
            .await?
            .results()?,
    )
}

/// Every member of a slot, those who left included, in the order they were added.
pub(crate) async fn of_slot(db: &D1Database, slot: &SlotId) -> Result<Vec<Member>> {
    let sql = format!("SELECT {COLUMNS} FROM members WHERE slot_id = ? ORDER BY created_at, rowid");
    rows(
        statement(db, &sql, &[text(slot.as_str())])?
            .all()
            .await?
            .results()?,
    )
}

/// The first member of a slot: the organizer who created it.
pub(crate) async fn creator(db: &D1Database, slot: &SlotId) -> Result<Option<Member>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM members WHERE slot_id = ? ORDER BY created_at, rowid LIMIT 1"
    );
    statement(db, &sql, &[text(slot.as_str())])?
        .first::<Row>(None)
        .await?
        .map(Member::try_from)
        .transpose()
}

/// A new member of a slot.
pub(crate) struct NewMember<'a> {
    pub(crate) id: &'a MemberId,
    pub(crate) slot: &'a SlotId,
    /// `None`: a name added by an organizer, that nobody has claimed yet.
    pub(crate) player: Option<&'a PlayerId>,
    pub(crate) name: &'a DisplayName,
    pub(crate) role: Role,
}

/// Inserts a member. Fails on a UNIQUE constraint if the name is taken in the slot, or if the
/// player is already a member of it.
pub(crate) fn insert(
    db: &D1Database,
    member: &NewMember<'_>,
    now: Timestamp,
) -> Result<D1PreparedStatement> {
    let claimed_at = member.player.map_or(JsValue::NULL, |_| instant(now));
    statement(
        db,
        "INSERT INTO members (id, slot_id, player_id, display_name, name_key, role, claimed_at, \
         created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        &[
            text(member.id.as_str()),
            text(member.slot.as_str()),
            member
                .player
                .map_or(JsValue::NULL, |player| text(player.as_str())),
            text(member.name.text()),
            text(member.name.key()),
            text(member.role.as_str()),
            claimed_at,
            instant(now),
        ],
    )
}

/// Links a free name to a player. Changes no row if the name was claimed or left meanwhile.
/// Fails on a UNIQUE constraint if the player is already a member of the slot.
pub(crate) fn claim(
    db: &D1Database,
    slot: &SlotId,
    id: &MemberId,
    player: &PlayerId,
    now: Timestamp,
) -> Result<D1PreparedStatement> {
    statement(
        db,
        "UPDATE members SET player_id = ?, claimed_at = ? \
         WHERE slot_id = ? AND id = ? AND player_id IS NULL AND left_at IS NULL",
        &[
            text(player.as_str()),
            instant(now),
            text(slot.as_str()),
            text(id.as_str()),
        ],
    )
}

/// Links a member to another player, claimed or not: a recovery link redeemed on a new device.
pub(crate) fn relink(
    db: &D1Database,
    id: &MemberId,
    player: &PlayerId,
    now: Timestamp,
) -> Result<D1PreparedStatement> {
    statement(
        db,
        "UPDATE members SET player_id = ?, claimed_at = ? WHERE id = ? AND left_at IS NULL",
        &[text(player.as_str()), instant(now), text(id.as_str())],
    )
}

/// Releases a claimed name: the member keeps their name and answers, and the name is free
/// again. Returns `false`, changing nothing, if that would leave the slot without an organizer.
pub(crate) async fn release(db: &D1Database, slot: &SlotId, id: &MemberId) -> Result<bool> {
    let sql = format!(
        "UPDATE members SET player_id = NULL, claimed_at = NULL \
         WHERE slot_id = ?1 AND id = ?2 AND left_at IS NULL AND {KEEPS_AN_ORGANIZER}"
    );
    let result = statement(db, &sql, &[text(slot.as_str()), text(id.as_str())])?
        .run()
        .await?;
    Ok(changes(&result)? > 0)
}

/// Changes a member's role. Returns `false`, changing nothing, if that would leave the slot
/// without an organizer.
pub(crate) async fn set_role(
    db: &D1Database,
    slot: &SlotId,
    id: &MemberId,
    role: Role,
) -> Result<bool> {
    let guard = if role == Role::Organizer {
        "1"
    } else {
        KEEPS_AN_ORGANIZER
    };
    let sql = format!(
        "UPDATE members SET role = ?3 WHERE slot_id = ?1 AND id = ?2 AND left_at IS NULL AND {guard}"
    );
    let result = statement(
        db,
        &sql,
        &[text(slot.as_str()), text(id.as_str()), text(role.as_str())],
    )?
    .run()
    .await?;
    Ok(changes(&result)? > 0)
}

/// Renames a member. Fails on the UNIQUE constraint if the name is taken in the slot.
pub(crate) async fn rename(
    db: &D1Database,
    slot: &SlotId,
    id: &MemberId,
    name: &DisplayName,
) -> Result<()> {
    statement(
        db,
        "UPDATE members SET display_name = ?, name_key = ? \
         WHERE slot_id = ? AND id = ? AND left_at IS NULL",
        &[
            text(name.text()),
            text(name.key()),
            text(slot.as_str()),
            text(id.as_str()),
        ],
    )?
    .run()
    .await?;
    Ok(())
}

/// A member leaves: the row stays without name, player or role, so the name and the device are
/// free again. Changes no row if that would leave the slot without an organizer.
pub(crate) fn leave(
    db: &D1Database,
    slot: &SlotId,
    id: &MemberId,
    now: Timestamp,
) -> Result<D1PreparedStatement> {
    let sql = format!(
        "UPDATE members SET display_name = NULL, name_key = NULL, player_id = NULL, \
         role = 'player', left_at = ?3 \
         WHERE slot_id = ?1 AND id = ?2 AND left_at IS NULL AND {KEEPS_AN_ORGANIZER}"
    );
    statement(
        db,
        &sql,
        &[text(slot.as_str()), text(id.as_str()), instant(now)],
    )
}
