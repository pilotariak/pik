// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

// Slots: POST /slots, GET /me, GET|PATCH|DELETE /slots/:id, POST /slots/:id/invite-code and
// PUT /slots/:id/list.

import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { after, before, describe, test } from "node:test";
import {
  COOKIE,
  addDays,
  assertProblem,
  checkAccess,
  createSlot,
  join,
  parisToday,
  sha256,
  slotBody,
  startApi,
  world,
} from "./api.mjs";

let mf;
let db;
let call;

before(async () => {
  ({ mf, db, call } = await startApi());
});

after(async () => {
  await mf.dispose();
});

describe("POST /slots", () => {
  test("without a token: 201, a new player token in the body and the cookie", async () => {
    const res = await call("POST", "/slots", { body: slotBody() });
    assert.equal(res.status, 201, res.text);
    assert.match(res.body.slot_id, /^s_[a-z2-7]{26}$/);
    assert.match(res.body.member_id, /^m_[a-z2-7]{26}$/);
    assert.match(res.body.invite_code, /^[A-HJ-NP-Z2-9]{8}$/, "8 characters without 0, O, 1, I");
    assert.match(res.body.player_token, /^[a-z2-7]{52}$/);
    assert.equal(res.headers.get("location"), `/api/v1/slots/${res.body.slot_id}`);
    assert.equal(
      res.headers.get("set-cookie"),
      `${COOKIE}=${res.body.player_token}; Path=/api; Max-Age=34560000; HttpOnly; Secure; SameSite=Lax`,
    );
    const me = await call("GET", "/me", { token: res.body.player_token });
    assert.equal(me.body.slots[0].role, "organizer", "the creator is an organizer");
  });

  test("with a known token: the same player, no new token", async () => {
    const { organizer } = await createSlot(call);
    const res = await call("POST", "/slots", {
      token: organizer.token,
      body: slotBody({ name: "Jeudi", organizer_name: "Maite" }),
    });
    assert.equal(res.status, 201, res.text);
    assert.equal(res.body.player_token, null);
    assert.equal(res.headers.get("set-cookie"), null);
    const me = await call("GET", "/me", { token: organizer.token });
    assert.deepEqual(
      me.body.slots.map((card) => card.name),
      ["Mercredi", "Jeudi"],
      "one device, both slots",
    );
  });

  test("with an unknown token: 401", async () => {
    assertProblem(
      await call("POST", "/slots", { token: "b".repeat(52), body: slotBody() }),
      401,
      "unauthorized",
    );
  });

  test("an outsider and a player can create their own slot", async () => {
    const w = await world(call);
    for (const caller of [w.outsider, w.player, w.organizer]) {
      const res = await call("POST", "/slots", { token: caller.token, body: slotBody() });
      assert.equal(res.status, 201, res.text);
      assert.equal(res.body.player_token, null, "the caller keeps their token");
    }
  });

  test("invalid bodies: 422 with a problem", async () => {
    const today = parisToday();
    for (const [changes, why] of [
      [{ court: "fronton" }, "court"],
      [{ interval_weeks: 3 }, "interval"],
      [{ anchor_date: addDays(today, -90) }, "anchor outside the season"],
      [{ season_end: addDays(today, 400) }, "season longer than a year"],
      [{ min_players: 6, places: 4 }, "places below the minimum"],
      [{ time_from: "18h00" }, "time"],
      [{ target: 50 }, "target"],
      [{ organizer_name: "  " }, "organizer name"],
      [{ name: "x".repeat(61) }, "name"],
      [{ unknown: 1 }, "unknown field"],
      [{ deadline_days_before: 7 }, "deadline"],
    ]) {
      const res = await call("POST", "/slots", { body: slotBody(changes) });
      assertProblem(res, 422, "invalid-request");
      assert.ok(res.body.detail, `422 says why: ${why}`);
    }
    assertProblem(await call("POST", "/slots", { body: "{" }), 422, "invalid-request");
  });

  test("Idempotency-Key: a replay returns the first slot with a fresh token", async () => {
    const key = randomUUID();
    const body = slotBody({ name: "Idempotent" });
    const first = await call("POST", "/slots", { body, headers: { "Idempotency-Key": key } });
    assert.equal(first.status, 201, first.text);
    // The same body with its fields in another order is the same request.
    const replay = await call("POST", "/slots", {
      body: Object.fromEntries(Object.entries(body).reverse()),
      headers: { "Idempotency-Key": key.toUpperCase() },
    });
    assert.equal(replay.status, 201, replay.text);
    assert.equal(replay.body.slot_id, first.body.slot_id, "the same slot");
    assert.equal(replay.body.member_id, first.body.member_id, "the same organizer");
    assert.notEqual(replay.body.player_token, first.body.player_token, "a fresh token");
    assert.equal(
      replay.headers.get("set-cookie")?.startsWith(`${COOKIE}=${replay.body.player_token};`),
      true,
    );
    assertProblem(await call("GET", "/me", { token: first.body.player_token }), 401, "unauthorized");
    const me = await call("GET", "/me", { token: replay.body.player_token });
    assert.deepEqual(me.body.slots.map((card) => card.id), [first.body.slot_id]);
    const { n } = await db
      .prepare("SELECT count(*) AS n FROM slots WHERE name = 'Idempotent'")
      .first();
    assert.equal(n, 1, "one slot created");

    // The key holds no token, only its hash and the slot id.
    const stored = await db.prepare("SELECT * FROM idempotency_keys").all();
    for (const row of stored.results) {
      const text = JSON.stringify(row);
      assert.ok(!text.includes(key), "the key is stored hashed");
      assert.ok(!text.includes(first.body.player_token), "no token in idempotency_keys");
      assert.ok(!text.includes(replay.body.player_token), "no token in idempotency_keys");
    }
    const row = await db
      .prepare("SELECT key, result_ref FROM idempotency_keys WHERE result_ref = ?")
      .bind(first.body.slot_id)
      .first();
    assert.equal(row.key, sha256(key).toString("hex"), "key is the SHA-256 of the UUID");
  });

  test("Idempotency-Key: a retry with the token the first response set keeps it", async () => {
    const key = randomUUID();
    const body = slotBody({ name: "Cookie retry" });
    const first = await call("POST", "/slots", { body, headers: { "Idempotency-Key": key } });
    assert.equal(first.status, 201, first.text);
    // The first response reached the browser, which stored the cookie, but not the page.
    const retry = await call("POST", "/slots", {
      cookie: first.body.player_token,
      body,
      headers: { "Idempotency-Key": key, Origin: "https://kancha.pilotariak.com" },
    });
    assert.equal(retry.status, 201, retry.text);
    assert.equal(retry.body.slot_id, first.body.slot_id, "the same slot");
    assert.equal(retry.body.player_token, null, "the player keeps its token");
    assert.equal((await call("GET", "/me", { token: first.body.player_token })).status, 200);
    const other = await createSlot(call);
    assertProblem(
      await call("POST", "/slots", {
        token: other.organizer.token,
        body,
        headers: { "Idempotency-Key": key },
      }),
      422,
      "idempotency-key-reused",
    );
  });

  test("Idempotency-Key: the same key with another body answers 422", async () => {
    const key = randomUUID();
    const first = await call("POST", "/slots", {
      body: slotBody({ name: "A" }),
      headers: { "Idempotency-Key": key },
    });
    assert.equal(first.status, 201);
    assertProblem(
      await call("POST", "/slots", {
        body: slotBody({ name: "B" }),
        headers: { "Idempotency-Key": key },
      }),
      422,
      "idempotency-key-reused",
    );
    assertProblem(
      await call("POST", "/slots", { body: slotBody(), headers: { "Idempotency-Key": "nope" } }),
      422,
      "invalid-request",
    );
  });

  test("rate limit: 429 with Retry-After past 5 creations a minute", async () => {
    const ip = "192.0.2.1";
    for (let i = 0; i < 5; i += 1) {
      assert.equal((await call("POST", "/slots", { ip, body: slotBody() })).status, 201);
    }
    const limited = await call("POST", "/slots", { ip, body: slotBody() });
    assertProblem(limited, 429, "rate-limited");
    assert.equal(limited.headers.get("retry-after"), "60");
    assert.equal(
      (await call("POST", "/slots", { ip: "192.0.2.2", body: slotBody() })).status,
      201,
      "another address is not limited",
    );
  });
});

describe("tokens", () => {
  test("only the SHA-256 of a player token is stored", async () => {
    const { organizer } = await createSlot(call);
    const row = await db
      .prepare(
        "SELECT typeof(token_hash) AS type, hex(token_hash) AS hash FROM players WHERE token_hash = ?",
      )
      .bind(sha256(organizer.token))
      .first();
    assert.equal(row.type, "blob");
    assert.equal(row.hash, sha256(organizer.token).toString("hex").toUpperCase());
    // No text column of any table holds the token.
    const { results: tables } = await db
      .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE '\\_%' ESCAPE '\\' AND name NOT LIKE 'sqlite_%'")
      .all();
    for (const { name } of tables) {
      const { results } = await db.prepare(`SELECT * FROM ${name}`).all();
      assert.ok(
        !JSON.stringify(results).includes(organizer.token),
        `no token in clear in ${name}`,
      );
    }
  });
});

describe("GET /me", () => {
  test("401 without a token; each caller sees only their own slots", async () => {
    assertProblem(await call("GET", "/me"), 401, "unauthorized");
    const w = await world(call);
    const card = (res) => res.body.slots.find((slot) => slot.id === w.slot);
    const organizer = await call("GET", "/me", { token: w.organizer.token });
    assert.equal(organizer.status, 200);
    assert.equal(card(organizer).role, "organizer");
    const player = await call("GET", "/me", { token: w.player.token });
    assert.equal(card(player).role, "player");
    assert.equal(card(player).member_id, w.player.member);
    const outsider = await call("GET", "/me", { token: w.outsider.token });
    assert.equal(card(outsider), undefined, "a member of another slot does not see it");
    assert.equal(outsider.body.slots.length, 1);
  });

  test("a card has the schedule and the next session with counts", async () => {
    const w = await world(call);
    await call("PUT", `/slots/${w.slot}/sessions/${w.session}/answer`, {
      token: w.player.token,
      body: { status: "ok" },
    });
    const res = await call("GET", "/me", { token: w.player.token });
    const card = res.body.slots[0];
    const weekday = new Date(`${w.session}T12:00:00Z`).getUTCDay() || 7;
    assert.deepEqual(card.schedule, {
      weekday,
      time_from: "18:00",
      time_to: "19:30",
      interval_weeks: 1,
    });
    assert.equal(card.court, "trinquet");
    assert.equal(card.next_session.date, w.session);
    assert.equal(card.next_session.state, "missing");
    assert.equal(card.next_session.missing, 1);
    assert.deepEqual(card.next_session.counts, { ok: 1, dispo: 0, ko: 0, none: 1 });
    assert.equal(card.next_session.my_answer, "ok");
    assert.equal(card.next_session.deadline_passed, false);
  });
});

describe("GET /slots/:id", () => {
  test("access: 401, 404 for another slot's member, 200 for members", async () => {
    await checkAccess(call, {
      request: (w) => ["GET", `/slots/${w.slot}`],
      organizerOnly: false,
      ok: 200,
    });
  });

  test("the slot, its members and upcoming sessions", async () => {
    const w = await world(call);
    const res = await call("GET", `/slots/${w.slot}`, { token: w.player.token });
    assert.equal(res.body.id, w.slot);
    assert.equal(res.body.invite_code, w.code);
    assert.deepEqual(res.body.me, { member_id: w.player.member, role: "player" });
    assert.deepEqual(
      res.body.members.map((m) => [m.name, m.role, m.claimed]),
      [
        ["Maite", "organizer", true],
        ["Peio", "player", true],
      ],
    );
    assert.deepEqual(
      res.body.upcoming_sessions.map((s) => s.date),
      [0, 7, 14, 21].map((days) => addDays(w.session, days)),
    );
    assert.deepEqual(res.body.deadline_rule, { days_before: 1, time: "20:00" });
    assert.equal(res.headers.get("cache-control"), "private, no-store");
  });

  test("an id that cannot exist is a 404, like a slot of someone else", async () => {
    const w = await world(call);
    assertProblem(await call("GET", "/slots/nope", { token: w.player.token }), 404, "not-found");
  });
});

describe("PATCH /slots/:id", () => {
  test("access: organizer only", async () => {
    await checkAccess(call, {
      request: (w) => ["PATCH", `/slots/${w.slot}`, { body: { name: "Mardi" } }],
      organizerOnly: true,
      ok: 200,
    });
  });

  test("changes name, times, minimum, places, target and deadline rule", async () => {
    const w = await world(call);
    const res = await call("PATCH", `/slots/${w.slot}`, {
      token: w.organizer.token,
      body: {
        name: "Mardi soir",
        venue: "Fronton",
        time_from: "19:00",
        time_to: null,
        min_players: 3,
        places: 6,
        target: 35,
        deadline_days_before: 2,
        deadline_time: "12:00",
      },
    });
    assert.equal(res.status, 200, res.text);
    assert.equal(res.body.name, "Mardi soir");
    assert.deepEqual(res.body.schedule.time_to, null, "null removes the end time");
    assert.deepEqual(res.body.deadline_rule, { days_before: 2, time: "12:00" });
    assert.equal(res.body.places, 6);
    const slot = await call("GET", `/slots/${w.slot}`, { token: w.player.token });
    assert.equal(slot.body.target, 35);
    assert.equal(slot.body.upcoming_sessions[0].min_players, 3);
  });

  test("interval_weeks and anchor_date answer 422", async () => {
    const w = await world(call);
    for (const body of [{ interval_weeks: 2 }, { anchor_date: addDays(w.session, 1) }, { name: "x", interval_weeks: 1 }]) {
      const res = await call("PATCH", `/slots/${w.slot}`, { token: w.organizer.token, body });
      assertProblem(res, 422, "invalid-request");
      assert.match(res.body.detail, /fixed for the season/);
    }
    const slot = await call("GET", `/slots/${w.slot}`, { token: w.organizer.token });
    assert.equal(slot.body.name, "Mercredi", "nothing changed");
  });

  test("a date with its own places must still fit a new minimum", async () => {
    const w = await world(call);
    const patched = await call("PATCH", `/slots/${w.slot}/sessions/${w.session}`, {
      token: w.organizer.token,
      body: { places: 3 },
    });
    assert.equal(patched.status, 200, patched.text);
    const res = await call("PATCH", `/slots/${w.slot}`, {
      token: w.organizer.token,
      body: { min_players: 4 },
    });
    assertProblem(res, 422, "invalid-request");
    assert.match(res.body.detail, new RegExp(w.session));
  });
});

describe("DELETE /slots/:id", () => {
  test("access: organizer only", async () => {
    await checkAccess(call, {
      request: (w) => ["DELETE", `/slots/${w.slot}`],
      organizerOnly: true,
      ok: 204,
    });
  });

  test("deletes the slot and all its data", async () => {
    const w = await world(call);
    await call("PUT", `/slots/${w.slot}/sessions/${w.session}/answer`, {
      token: w.player.token,
      body: { status: "ok" },
    });
    await call("POST", `/slots/${w.slot}/members/${w.player.member}/recovery`, {
      token: w.organizer.token,
    });
    assert.equal((await call("DELETE", `/slots/${w.slot}`, { token: w.organizer.token })).status, 204);
    for (const [table, column] of [
      ["slots", "id"],
      ["members", "slot_id"],
      ["answers", "slot_id"],
    ]) {
      const { n } = await db
        .prepare(`SELECT count(*) AS n FROM ${table} WHERE ${column} = ?`)
        .bind(w.slot)
        .first();
      assert.equal(n, 0, `no row left in ${table}`);
    }
    const { n } = await db
      .prepare("SELECT count(*) AS n FROM recoveries WHERE member_id = ?")
      .bind(w.player.member)
      .first();
    assert.equal(n, 0, "no recovery link left");
    assertProblem(await call("GET", `/slots/${w.slot}`, { token: w.organizer.token }), 404, "not-found");
    assertProblem(await call("GET", `/join/${w.code}`), 404, "not-found");
  });
});

describe("POST /slots/:id/invite-code", () => {
  test("access: organizer only", async () => {
    await checkAccess(call, {
      request: (w) => ["POST", `/slots/${w.slot}/invite-code`],
      organizerOnly: true,
      ok: 200,
    });
  });

  test("rotating the code does not break the calls of members who already joined", async () => {
    const w = await world(call);
    const res = await call("POST", `/slots/${w.slot}/invite-code`, { token: w.organizer.token });
    assert.equal(res.status, 200);
    assert.match(res.body.invite_code, /^[A-HJ-NP-Z2-9]{8}$/);
    assert.notEqual(res.body.invite_code, w.code);
    assertProblem(await call("GET", `/join/${w.code}`), 404, "not-found");
    assert.equal((await call("GET", `/join/${res.body.invite_code}`)).status, 200);
    // The player who joined with the old code keeps reading and answering.
    assert.equal((await call("GET", `/slots/${w.slot}`, { token: w.player.token })).status, 200);
    const answer = await call("PUT", `/slots/${w.slot}/sessions/${w.session}/answer`, {
      token: w.player.token,
      body: { status: "dispo" },
    });
    assert.equal(answer.status, 200, answer.text);
    await join(call, res.body.invite_code, "Unai");
  });
});

describe("PUT /slots/:id/list", () => {
  test("access: organizer only", async () => {
    await checkAccess(call, {
      request: (w) => ["PUT", `/slots/${w.slot}/list`, { body: { closed: true } }],
      organizerOnly: true,
      ok: 200,
    });
  });

  test("a closed list takes no new name, and reopens", async () => {
    const w = await world(call);
    const closed = await call("PUT", `/slots/${w.slot}/list`, {
      token: w.organizer.token,
      body: { closed: true },
    });
    assert.deepEqual(closed.body, { closed: true });
    assertProblem(
      await call("POST", `/join/${w.code}/members`, { body: { name: "Unai" } }),
      409,
      "list-closed",
    );
    assert.equal((await call("GET", `/join/${w.code}`)).body.list_closed, true);
    await call("PUT", `/slots/${w.slot}/list`, { token: w.organizer.token, body: { closed: false } });
    await join(call, w.code, "Unai");
  });
});
