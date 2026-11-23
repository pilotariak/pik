// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

// Members: POST /slots/:id/members, PATCH /slots/:id/members/:member,
// DELETE /slots/:id/members/:member/claim and DELETE /slots/:id/members/me.

import assert from "node:assert/strict";
import { after, before, describe, test } from "node:test";
import { addDays, assertProblem, checkAccess, join, parisToday, startApi, world } from "./api.mjs";

let mf;
let db;
let call;

before(async () => {
  ({ mf, db, call } = await startApi());
});

after(async () => {
  await mf.dispose();
});

describe("POST /slots/:id/members", () => {
  test("access: organizer only", async () => {
    await checkAccess(call, {
      request: (w) => ["POST", `/slots/${w.slot}/members`, { body: { name: "Ana" } }],
      organizerOnly: true,
      ok: 201,
    });
  });

  test("adds a free name; the same name in another case is taken", async () => {
    const w = await world(call);
    const res = await call("POST", `/slots/${w.slot}/members`, {
      token: w.organizer.token,
      body: { name: "Ana" },
    });
    assert.equal(res.status, 201, res.text);
    assert.deepEqual(
      { name: res.body.name, role: res.body.role, claimed: res.body.claimed },
      { name: "Ana", role: "player", claimed: false },
    );
    assertProblem(
      await call("POST", `/slots/${w.slot}/members`, { token: w.organizer.token, body: { name: "ANA" } }),
      409,
      "name-taken",
    );
  });
});

describe("PATCH /slots/:id/members/:member", () => {
  test("access: organizer only", async () => {
    await checkAccess(call, {
      request: (w) => ["PATCH", `/slots/${w.slot}/members/${w.player.member}`, { body: { name: "Peio B" } }],
      organizerOnly: true,
      ok: 200,
    });
  });

  test("makes a co-organizer, who can then use organizer routes", async () => {
    const w = await world(call);
    const res = await call("PATCH", `/slots/${w.slot}/members/${w.player.member}`, {
      token: w.organizer.token,
      body: { role: "organizer" },
    });
    assert.equal(res.status, 200, res.text);
    assert.equal(res.body.role, "organizer");
    const rotated = await call("POST", `/slots/${w.slot}/invite-code`, { token: w.player.token });
    assert.equal(rotated.status, 200, "the co-organizer rotates the code");
    // With two organizers, one can step down.
    const down = await call("PATCH", `/slots/${w.slot}/members/${w.organizer.member}`, {
      token: w.organizer.token,
      body: { role: "player" },
    });
    assert.equal(down.status, 200, down.text);
  });

  test("the last organizer cannot lose the role: 409", async () => {
    const w = await world(call);
    assertProblem(
      await call("PATCH", `/slots/${w.slot}/members/${w.organizer.member}`, {
        token: w.organizer.token,
        body: { role: "player" },
      }),
      409,
      "last-organizer",
    );
    const slot = await call("GET", `/slots/${w.slot}`, { token: w.organizer.token });
    assert.equal(slot.body.me.role, "organizer", "the role is unchanged");
  });

  test("fixes a misspelled name, unless another member has it", async () => {
    const w = await world(call);
    const res = await call("PATCH", `/slots/${w.slot}/members/${w.player.member}`, {
      token: w.organizer.token,
      body: { name: "Peyo" },
    });
    assert.equal(res.body.name, "Peyo");
    assertProblem(
      await call("PATCH", `/slots/${w.slot}/members/${w.player.member}`, {
        token: w.organizer.token,
        body: { name: "maite" },
      }),
      409,
      "name-taken",
    );
    assertProblem(
      await call("PATCH", `/slots/${w.slot}/members/${w.outsider.member}`, {
        token: w.organizer.token,
        body: { name: "X" },
      }),
      404,
      "not-found",
    );
    assertProblem(
      await call("PATCH", `/slots/${w.slot}/members/${w.player.member}`, {
        token: w.organizer.token,
        body: { role: "admin" },
      }),
      422,
      "invalid-request",
    );
  });
});

describe("DELETE /slots/:id/members/:member/claim", () => {
  test("access: organizer only", async () => {
    await checkAccess(call, {
      request: (w) => ["DELETE", `/slots/${w.slot}/members/${w.player.member}/claim`],
      organizerOnly: true,
      ok: 204,
    });
  });

  test("frees a name claimed by mistake, which keeps its answers", async () => {
    const w = await world(call);
    await call("PUT", `/slots/${w.slot}/sessions/${w.session}/answer`, {
      token: w.player.token,
      body: { status: "ok" },
    });
    const res = await call("DELETE", `/slots/${w.slot}/members/${w.player.member}/claim`, {
      token: w.organizer.token,
    });
    assert.equal(res.status, 204);
    assertProblem(await call("GET", `/slots/${w.slot}`, { token: w.player.token }), 404, "not-found");
    const preview = await call("GET", `/join/${w.code}`);
    assert.deepEqual(preview.body.free_names, [{ member_id: w.player.member, name: "Peio" }]);
    const claimed = await call("POST", `/join/${w.code}/claim`, { body: { member_id: w.player.member } });
    assert.equal(claimed.status, 200, claimed.text);
    const session = await call("GET", `/slots/${w.slot}/sessions/${w.session}`, {
      token: claimed.body.player_token,
    });
    assert.equal(session.body.my_answer, "ok", "the answer stayed with the member");
  });

  test("the last organizer's name cannot be released: 409", async () => {
    const w = await world(call);
    assertProblem(
      await call("DELETE", `/slots/${w.slot}/members/${w.organizer.member}/claim`, {
        token: w.organizer.token,
      }),
      409,
      "last-organizer",
    );
  });
});

describe("DELETE /slots/:id/members/me", () => {
  test("access: 401 without a token, 404 for another slot's member, 204 for a player", async () => {
    const w = await world(call);
    const path = `/slots/${w.slot}/members/me`;
    assertProblem(await call("DELETE", path), 401, "unauthorized");
    assertProblem(await call("DELETE", path, { token: w.outsider.token }), 404, "not-found");
    assert.equal((await call("DELETE", path, { token: w.player.token })).status, 204);
    assertProblem(await call("DELETE", path, { token: w.player.token }), 404, "not-found");
    // The organizer is the last one: see the next test.
    assertProblem(await call("DELETE", path, { token: w.organizer.token }), 409, "last-organizer");
  });

  test("the last organizer cannot leave: 409; with a co-organizer they can", async () => {
    const w = await world(call);
    assertProblem(
      await call("DELETE", `/slots/${w.slot}/members/me`, { token: w.organizer.token }),
      409,
      "last-organizer",
    );
    await call("PATCH", `/slots/${w.slot}/members/${w.player.member}`, {
      token: w.organizer.token,
      body: { role: "organizer" },
    });
    assert.equal(
      (await call("DELETE", `/slots/${w.slot}/members/me`, { token: w.organizer.token })).status,
      204,
    );
    assertProblem(
      await call("DELETE", `/slots/${w.slot}/members/me`, { token: w.player.token }),
      409,
      "last-organizer",
    );
  });

  test("a member who left is an 'Ancien joueur' with past answers; future ones go", async () => {
    const today = parisToday();
    // Sessions a week ago, today and next week.
    const w = await world(call, { anchor_date: addDays(today, -7) });
    const past = addDays(today, -7);
    const next = addDays(today, 7);
    // A past answer, written in D1: the API refuses answers after the deadline.
    await db
      .prepare("INSERT INTO answers (slot_id, date, member_id, status, answered_at) VALUES (?, ?, ?, 'ok', ?)")
      .bind(w.slot, past, w.player.member, Date.now() - 10 * 86_400_000)
      .run();
    await call("PUT", `/slots/${w.slot}/sessions/${next}/answer`, {
      token: w.player.token,
      body: { status: "ok" },
    });
    assert.equal(
      (await call("DELETE", `/slots/${w.slot}/members/me`, { token: w.player.token })).status,
      204,
    );
    assertProblem(await call("GET", `/slots/${w.slot}`, { token: w.player.token }), 404, "not-found");
    const pastSession = await call("GET", `/slots/${w.slot}/sessions/${past}`, {
      token: w.organizer.token,
    });
    assert.deepEqual(
      pastSession.body.answers.map((answer) => [answer.member_id, answer.name, answer.status]),
      [[w.player.member, null, "ok"]],
      "the past answer stays, without a name",
    );
    const nextSession = await call("GET", `/slots/${w.slot}/sessions/${next}`, {
      token: w.organizer.token,
    });
    assert.deepEqual(nextSession.body.answers, [], "the answers to come are gone");
    assert.deepEqual(nextSession.body.counts, { ok: 0, dispo: 0, ko: 0, none: 1 });
    // The name and the device are free again.
    await join(call, w.code, "Peio", w.player.token);
  });
});
