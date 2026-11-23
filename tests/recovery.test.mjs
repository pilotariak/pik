// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

// Recovery links: POST /slots/:id/members/:member/recovery and POST /recover/:token.

import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { after, before, describe, test } from "node:test";
import { COOKIE, assertProblem, checkAccess, sha256, startApi, world } from "./api.mjs";

let mf;
let db;
let call;

before(async () => {
  ({ mf, db, call } = await startApi());
});

after(async () => {
  await mf.dispose();
});

async function link(w, member = w.player.member, headers = {}) {
  const res = await call("POST", `/slots/${w.slot}/members/${member}/recovery`, {
    token: w.organizer.token,
    headers,
  });
  assert.equal(res.status, 201, res.text);
  return res.body;
}

describe("POST /slots/:id/members/:member/recovery", () => {
  test("access: organizer only", async () => {
    await checkAccess(call, {
      request: (w) => ["POST", `/slots/${w.slot}/members/${w.player.member}/recovery`],
      organizerOnly: true,
      ok: 201,
    });
  });

  test("a link valid 24 hours, stored as the SHA-256 of its token", async () => {
    const w = await world(call);
    const before = Date.now();
    const { token, expires_at: expiresAt } = await link(w);
    assert.match(token, /^[a-z2-7]{52}$/);
    const expires = new Date(expiresAt).getTime();
    assert.ok(expires >= before + 24 * 3_600_000 && expires <= Date.now() + 24 * 3_600_000);
    const row = await db
      .prepare("SELECT member_id, created_by, typeof(token_hash) AS type FROM recoveries WHERE token_hash = ?")
      .bind(sha256(token))
      .first();
    assert.deepEqual(row, { member_id: w.player.member, created_by: w.organizer.member, type: "blob" });
    assertProblem(
      await call("POST", `/slots/${w.slot}/members/${w.outsider.member}/recovery`, { token: w.organizer.token }),
      404,
      "not-found",
    );
  });

  test("Idempotency-Key: a replay returns the first link with a fresh token", async () => {
    const w = await world(call);
    const key = randomUUID();
    const first = await link(w, w.player.member, { "Idempotency-Key": key });
    const replay = await link(w, w.player.member, { "Idempotency-Key": key });
    assert.notEqual(replay.token, first.token, "a fresh token");
    assert.equal(replay.expires_at, first.expires_at, "the same link");
    const { n } = await db
      .prepare("SELECT count(*) AS n FROM recoveries WHERE member_id = ?")
      .bind(w.player.member)
      .first();
    assert.equal(n, 1, "one link");
    assertProblem(await call("POST", `/recover/${first.token}`), 404, "not-found");
    assert.equal((await call("POST", `/recover/${replay.token}`)).status, 200);
    const keys = await db.prepare("SELECT * FROM idempotency_keys").all();
    for (const token of [first.token, replay.token]) {
      assert.ok(!JSON.stringify(keys.results).includes(token), "no token in idempotency_keys");
    }
    // Another member with the same key is another request.
    assertProblem(
      await call("POST", `/slots/${w.slot}/members/${w.organizer.member}/recovery`, {
        token: w.organizer.token,
        headers: { "Idempotency-Key": key },
      }),
      422,
      "idempotency-key-reused",
    );
  });
});

describe("POST /recover/:token", () => {
  test("a new device gets the member, a new token and the member's answers", async () => {
    const w = await world(call);
    await call("PUT", `/slots/${w.slot}/sessions/${w.session}/answer`, {
      token: w.player.token,
      body: { status: "ok" },
    });
    const { token } = await link(w);
    const res = await call("POST", `/recover/${token}`);
    assert.equal(res.status, 200, res.text);
    assert.deepEqual(
      [res.body.slot_id, res.body.member_id],
      [w.slot, w.player.member],
    );
    assert.match(res.body.player_token, /^[a-z2-7]{52}$/);
    assert.ok(res.headers.get("set-cookie").startsWith(`${COOKIE}=${res.body.player_token};`));
    const session = await call("GET", `/slots/${w.slot}/sessions/${w.session}`, {
      token: res.body.player_token,
    });
    assert.equal(session.body.my_answer, "ok", "the answers stay");
    assertProblem(
      await call("GET", `/slots/${w.slot}`, { token: w.player.token }),
      404,
      "not-found",
    );
  });

  test("a link works once", async () => {
    const w = await world(call);
    const { token } = await link(w);
    assert.equal((await call("POST", `/recover/${token}`)).status, 200);
    assertProblem(await call("POST", `/recover/${token}`), 410, "recovery-link-gone");
  });

  test("a link expires after 24 hours", async () => {
    const w = await world(call);
    const { token } = await link(w);
    await db
      .prepare("UPDATE recoveries SET expires_at = ? WHERE token_hash = ?")
      .bind(Date.now() - 1, sha256(token))
      .run();
    assertProblem(await call("POST", `/recover/${token}`), 410, "recovery-link-gone");
  });

  test("the device of a member of another slot links the member to its player", async () => {
    const w = await world(call);
    const { token } = await link(w);
    const res = await call("POST", `/recover/${token}`, { token: w.outsider.token });
    assert.equal(res.status, 200, res.text);
    assert.equal(res.body.player_token, null, "the device keeps its token");
    const me = await call("GET", "/me", { token: w.outsider.token });
    assert.deepEqual(me.body.slots.map((card) => card.id).sort(), [w.outsider.slot, w.slot].sort());
  });

  test("a device that holds another member of the slot: 409", async () => {
    const w = await world(call);
    const { token } = await link(w, w.player.member);
    assertProblem(await call("POST", `/recover/${token}`, { token: w.organizer.token }), 409, "already-member");
    const own = await link(w, w.player.member);
    assert.equal(
      (await call("POST", `/recover/${own.token}`, { token: w.player.token })).status,
      200,
      "the member's own device may redeem it",
    );
  });

  test("an unknown link: 404; an unknown player token: 401", async () => {
    const w = await world(call);
    assertProblem(await call("POST", `/recover/${"a".repeat(52)}`), 404, "not-found");
    assertProblem(await call("POST", "/recover/nope"), 404, "not-found");
    const { token } = await link(w);
    assertProblem(await call("POST", `/recover/${token}`, { token: "d".repeat(52) }), 401, "unauthorized");
  });

  test("rate limit: 429 with Retry-After past 5 redemptions a minute", async () => {
    const ip = "203.0.113.9";
    for (let i = 0; i < 5; i += 1) {
      assertProblem(await call("POST", `/recover/${"e".repeat(52)}`, { ip }), 404, "not-found");
    }
    const limited = await call("POST", `/recover/${"e".repeat(52)}`, { ip });
    assertProblem(limited, 429, "rate-limited");
    assert.equal(limited.headers.get("retry-after"), "60");
  });
});
