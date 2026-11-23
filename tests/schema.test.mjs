// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

// Applies database/migrations/*.sql to the D1 binding of the Worker with applyMigrations() of
// worker.mjs, and checks what the schema promises: foreign keys and
// ON DELETE CASCADE are enforced, deleting a slot leaves none of its rows, and a member who
// leaves keeps their answers and lineup entries and frees their name.

import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { applyMigrations, startWorker } from "./worker.mjs";

// Every table of the schema, by what ties its rows to a slot. A new table must be added here,
// so that the deletion test covers it.
const BY_SLOT = {
  slots: "id",
  members: "slot_id",
  session_overrides: "slot_id",
  answers: "slot_id",
  guests: "slot_id",
  lineups: "slot_id",
  lineup_entries: "slot_id",
};
const BY_MEMBER = {
  recoveries: "member_id",
};
// Not slot data: a device shared by all the slots of that device (ADR-0002), and request keys
// that hold an id but no data, purged after 24 h by the Cron (#5).
const NOT_SLOT_DATA = ["players", "idempotency_keys"];

const now = Date.UTC(2027, 2, 1, 12);

let mf;
let db;

before(async () => {
  mf = startWorker();
  db = await mf.getD1Database("DB");
  await applyMigrations(db);
});

after(async () => {
  await mf.dispose();
});

async function tables() {
  const { results } = await db
    .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
    .all();
  return results
    .map(({ name }) => name)
    .filter((name) => !name.startsWith("sqlite_") && !name.startsWith("_cf_"));
}

async function count(table, column, values) {
  const marks = values.map(() => "?").join(", ");
  const row = await db
    .prepare(`SELECT count(*) AS n FROM ${table} WHERE ${column} IN (${marks})`)
    .bind(...values)
    .first();
  return row.n;
}

async function insertPlayer(id) {
  await db
    .prepare(
      "INSERT INTO players (id, token_hash, created_at, last_seen_at) VALUES (?, randomblob(32), ?, ?)",
    )
    .bind(id, now, now)
    .run();
}

// A weekly slot with a row in every table: an organizer on a device shared with other slots, a
// member on a device of their own, an unclaimed name, a cancelled date, answers (one set by the
// organizer), a guest, a lineup with a player who waits for the next game, a recovery link and
// an idempotency key.
async function seedSlot(slot, { sharedDevice }) {
  const id = (name) => `${slot}-${name}`;
  const date = "2027-03-16";
  await insertPlayer(id("device"));
  const statements = [
    [
      `INSERT INTO slots (id, name, venue, court, specialty, interval_weeks, anchor_date, time_from,
         season_start, season_end, invite_code, created_at)
       VALUES (?, 'Mardi', 'Trinquet Moderne', 'trinquet', 'gomme_creuse', 1, '2027-03-02', '19:00',
         '2027-03-01', '2027-06-30', ?, ?)`,
      slot,
      id("invite"),
      now,
    ],
    [
      `INSERT INTO members (id, slot_id, player_id, display_name, name_key, role, claimed_at, created_at)
       VALUES (?, ?, ?, 'Maite', 'maite', 'organizer', ?, ?)`,
      id("maite"),
      slot,
      sharedDevice,
      now,
      now,
    ],
    [
      `INSERT INTO members (id, slot_id, player_id, display_name, name_key, claimed_at, created_at)
       VALUES (?, ?, ?, 'Bixente', 'bixente', ?, ?)`,
      id("bixente"),
      slot,
      id("device"),
      now,
      now,
    ],
    [
      `INSERT INTO members (id, slot_id, display_name, name_key, created_at)
       VALUES (?, ?, 'Ana', 'ana', ?)`,
      id("ana"),
      slot,
      now,
    ],
    [
      `INSERT INTO session_overrides (slot_id, date, cancelled_at, note) VALUES (?, '2027-03-09', ?, 'Fronton fermé')`,
      slot,
      now,
    ],
    [
      `INSERT INTO answers (slot_id, date, member_id, status, answered_at) VALUES (?, ?, ?, 'ok', ?)`,
      slot,
      date,
      id("maite"),
      now,
    ],
    [
      `INSERT INTO answers (slot_id, date, member_id, status, answered_at) VALUES (?, ?, ?, 'ok', ?)`,
      slot,
      date,
      id("bixente"),
      now + 1,
    ],
    [
      `INSERT INTO answers (slot_id, date, member_id, status, answered_at, set_by) VALUES (?, ?, ?, 'dispo', ?, ?)`,
      slot,
      date,
      id("ana"),
      now + 2,
      id("maite"),
    ],
    [
      `INSERT INTO guests (id, slot_id, date, display_name) VALUES (?, ?, ?, 'Txomin')`,
      id("txomin"),
      slot,
      date,
    ],
    [
      `INSERT INTO lineups (slot_id, date, updated_at, updated_by) VALUES (?, ?, ?, ?)`,
      slot,
      date,
      now,
      id("maite"),
    ],
    [
      `INSERT INTO lineup_entries (slot_id, date, position, member_id, game, team) VALUES (?, ?, 1, ?, 1, 'a')`,
      slot,
      date,
      id("maite"),
    ],
    [
      `INSERT INTO lineup_entries (slot_id, date, position, member_id, game, team) VALUES (?, ?, 2, ?, 1, 'a')`,
      slot,
      date,
      id("bixente"),
    ],
    [
      `INSERT INTO lineup_entries (slot_id, date, position, guest_id, game, team) VALUES (?, ?, 3, ?, 1, 'b')`,
      slot,
      date,
      id("txomin"),
    ],
    [
      `INSERT INTO lineup_entries (slot_id, date, position, member_id) VALUES (?, ?, 4, ?)`,
      slot,
      date,
      id("ana"),
    ],
    [
      `INSERT INTO recoveries (token_hash, member_id, created_by, expires_at) VALUES (randomblob(32), ?, ?, ?)`,
      id("ana"),
      id("maite"),
      now + 24 * 3600 * 1000,
    ],
    [
      `INSERT INTO idempotency_keys (key, route, request_hash, result_ref, created_at)
       VALUES (?, 'POST /api/v1/slots', randomblob(32), ?, ?)`,
      id("key"),
      slot,
      now,
    ],
  ];
  await db.batch(statements.map(([sql, ...values]) => db.prepare(sql).bind(...values)));
  return {
    members: [id("maite"), id("bixente"), id("ana")],
    device: id("device"),
    key: id("key"),
  };
}

// Rows of a slot in every table, by table.
async function rowsOf(slot, members) {
  const rows = {};
  for (const [table, column] of Object.entries(BY_SLOT)) {
    rows[table] = await count(table, column, [slot]);
  }
  for (const [table, column] of Object.entries(BY_MEMBER)) {
    rows[table] = await count(table, column, members);
  }
  return rows;
}

test("the migrations create every table of the schema", async () => {
  assert.deepEqual(
    await tables(),
    [...Object.keys(BY_SLOT), ...Object.keys(BY_MEMBER), ...NOT_SLOT_DATA].sort(),
  );
});

test("D1 enforces foreign keys", async () => {
  assert.equal((await db.prepare("PRAGMA foreign_keys").first()).foreign_keys, 1);

  await assert.rejects(
    db
      .prepare(
        "INSERT INTO members (id, slot_id, display_name, name_key, created_at) VALUES ('orphan', 'no-such-slot', 'Ana', 'ana', ?)",
      )
      .bind(now)
      .run(),
    /FOREIGN KEY constraint failed/,
  );

  // A lineup entry needs its lineup header (composite key).
  await seedSlot("fk", { sharedDevice: null });
  await assert.rejects(
    db
      .prepare(
        "INSERT INTO lineup_entries (slot_id, date, position, member_id) VALUES ('fk', '2027-03-23', 1, 'fk-ana')",
      )
      .run(),
    /FOREIGN KEY constraint failed/,
  );
});

test("deleting a slot leaves no row of it in any table", async () => {
  await insertPlayer("shared-device");
  const deleted = await seedSlot("deleted", { sharedDevice: "shared-device" });
  const kept = await seedSlot("kept", { sharedDevice: "shared-device" });

  const before = await rowsOf("deleted", deleted.members);
  for (const [table, n] of Object.entries(before)) {
    assert.ok(n > 0, `the seed has a row in ${table}`);
  }
  const keptBefore = await rowsOf("kept", kept.members);

  await db.prepare("DELETE FROM slots WHERE id = ?").bind("deleted").run();

  const remaining = await rowsOf("deleted", deleted.members);
  assert.deepEqual(
    remaining,
    Object.fromEntries(Object.keys(before).map((table) => [table, 0])),
  );
  // ON DELETE CASCADE stops at the deleted slot.
  assert.deepEqual(await rowsOf("kept", kept.members), keptBefore);

  // Not slot data, left as designed: the devices (the shared one still has a member in the other
  // slot), and the idempotency key, which only holds the slot id until the Cron purges it.
  assert.equal(await count("players", "id", ["shared-device", deleted.device]), 2);
  assert.equal(await count("idempotency_keys", "key", [deleted.key]), 1);
});

test("a member who leaves keeps their answers and lineup entries, and frees their name", async () => {
  await insertPlayer("leaver-device");
  await seedSlot("leave", { sharedDevice: "leaver-device" });
  const insertMember = (id, player, name, key) =>
    db
      .prepare(
        "INSERT INTO members (id, slot_id, player_id, display_name, name_key, created_at) VALUES (?, 'leave', ?, ?, ?, ?)",
      )
      .bind(id, player, name, key, now)
      .run();

  // While Maite is a member, nobody else takes her name, whatever its case.
  await assert.rejects(
    insertMember("leave-maite-2", null, "MAITE", "maite"),
    /UNIQUE constraint failed/,
  );
  // Leaving clears the name: a row with left_at and a name breaks the CHECK.
  await assert.rejects(
    db.prepare("UPDATE members SET left_at = ? WHERE id = 'leave-maite'").bind(now).run(),
    /CHECK constraint failed/,
  );

  await db
    .prepare(
      "UPDATE members SET display_name = NULL, name_key = NULL, player_id = NULL, left_at = ? WHERE id = 'leave-maite'",
    )
    .bind(now)
    .run();

  // The row stays, shown as "Ancien joueur", with its answers and lineup entries.
  const left = await db
    .prepare("SELECT display_name, name_key, player_id, left_at FROM members WHERE id = 'leave-maite'")
    .first();
  assert.deepEqual(left, { display_name: null, name_key: null, player_id: null, left_at: now });
  assert.equal(await count("answers", "member_id", ["leave-maite"]), 1);
  assert.equal(await count("lineup_entries", "member_id", ["leave-maite"]), 1);
  assert.equal(await count("answers", "set_by", ["leave-maite"]), 1);

  // The name is free again, and the device can join the slot again.
  await insertMember("leave-maite-2", "leaver-device", "Maite", "maite");
  assert.equal(await count("members", "slot_id", ["leave"]), 4);
});
