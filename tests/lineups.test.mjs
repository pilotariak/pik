// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

// Lineups: PUT /slots/:id/sessions/:date/lineup, guarded by If-Match.

import assert from "node:assert/strict";
import { after, before, describe, test } from "node:test";
import { addDays, assertProblem, checkAccess, join, startApi, world } from "./api.mjs";

let mf;
let call;

before(async () => {
  ({ mf, call } = await startApi());
});

after(async () => {
  await mf.dispose();
});

// A slot with six members and one guest on its first session, 6 places.
async function team() {
  const w = await world(call, { min_players: 4, places: 6 });
  const members = [w.organizer.member, w.player.member];
  for (const name of ["Ana", "Bixente", "Carmen", "Dani"]) {
    members.push((await join(call, w.code, name)).member);
  }
  const base = `/slots/${w.slot}/sessions/${w.session}`;
  const guests = await call("PUT", `${base}/guests`, {
    token: w.organizer.token,
    body: { guests: [{ name: "Unai" }] },
  });
  return { ...w, base, members, guest: guests.body.guests[0].id };
}

const m = (id) => ({ member: id });

describe("PUT /slots/:id/sessions/:date/lineup", () => {
  test("access: organizer only", async () => {
    await checkAccess(call, {
      request: (w) => [
        "PUT",
        `/slots/${w.slot}/sessions/${w.session}/lineup`,
        { headers: { "If-Match": '"0"' }, body: { selection: [m(w.player.member)], games: [] } },
      ],
      organizerOnly: true,
      ok: 200,
    });
  });

  test("without If-Match: 428; with a stale one: 412", async () => {
    const t = await team();
    const body = { selection: [m(t.members[0])], games: [] };
    assertProblem(
      await call("PUT", `${t.base}/lineup`, { token: t.organizer.token, body }),
      428,
      "if-match-required",
    );
    const first = await call("PUT", `${t.base}/lineup`, {
      token: t.organizer.token,
      headers: { "If-Match": '"0"' },
      body,
    });
    assert.equal(first.status, 200, first.text);
    assert.equal(first.headers.get("etag"), '"1"');
    // A second organizer still holds the ETag "0".
    for (const etag of ['"0"', '"7"', 'W/"1"', "1"]) {
      assertProblem(
        await call("PUT", `${t.base}/lineup`, {
          token: t.organizer.token,
          headers: { "If-Match": etag },
          body,
        }),
        412,
        "lineup-changed",
      );
    }
    const second = await call("PUT", `${t.base}/lineup`, {
      token: t.organizer.token,
      headers: { "If-Match": '"1"' },
      body: { selection: [], games: [] },
    });
    assert.equal(second.headers.get("etag"), '"2"');
  });

  test("selection and pairs, read back with the session's ETag", async () => {
    const t = await team();
    const [a, b, c, d, e] = t.members;
    const body = {
      selection: [m(a), m(b), m(c), m(d), { guest: t.guest }, m(e)],
      games: [{ game: 1, a: [m(a), m(c)], b: [m(b), { guest: t.guest }] }],
    };
    const res = await call("PUT", `${t.base}/lineup`, {
      token: t.organizer.token,
      headers: { "If-Match": '"0"' },
      body,
    });
    assert.equal(res.status, 200, res.text);
    assert.deepEqual(res.body, { version: 1, ...body });
    const session = await call("GET", t.base, { token: t.player.token });
    assert.equal(session.headers.get("etag"), '"1"');
    assert.deepEqual(session.body.lineup, { version: 1, ...body });
  });

  test("422: a team without two players, a player outside the selection, too many selected", async () => {
    const t = await team();
    const [a, b, c, d] = t.members;
    for (const [body, why] of [
      [{ selection: [m(a), m(b), m(c)], games: [{ game: 1, a: [m(a)], b: [m(b), m(c)] }] }, /exactly 2/],
      [{ selection: [m(a), m(b), m(c)], games: [{ game: 1, a: [m(a), m(b)], b: [m(c), m(d)] }] }, /not in the selection/],
      [{ selection: [...t.members, { guest: t.guest }].map((s) => (typeof s === "string" ? m(s) : s)), games: [] }, /exceeds the 6 places/],
      [{ selection: [m(a), m(a)], games: [] }, /twice/],
      [{ selection: [m(t.outsider.member)], games: [] }, /not a member of the slot/],
      [{ selection: [{ guest: "g_aaaaaaaaaaaaaaaaaaaaaaaaaa" }], games: [] }, /not a guest/],
      [{ selection: [m(a), m(b), m(c), m(d)], games: [{ game: 1, a: [m(a), m(b)], b: [m(c), m(d)] }, { game: 2, a: [m(a), m(b)], b: [m(c), m(d)] }] }, /another game/],
      [{ selection: [], games: [], extra: true }, /unknown field/],
    ]) {
      const res = await call("PUT", `${t.base}/lineup`, {
        token: t.organizer.token,
        headers: { "If-Match": '"0"' },
        body,
      });
      assertProblem(res, 422, "invalid-request");
      assert.match(res.body.detail, why);
    }
    const session = await call("GET", t.base, { token: t.organizer.token });
    assert.equal(session.headers.get("etag"), '"0"', "no rejected write changed the lineup");
  });

  test("the effective places of the date count, and a cancelled date takes no lineup", async () => {
    const t = await team();
    await call("PATCH", t.base, { token: t.organizer.token, body: { places: 4 } });
    const five = t.members.slice(0, 5).map(m);
    assertProblem(
      await call("PUT", `${t.base}/lineup`, {
        token: t.organizer.token,
        headers: { "If-Match": '"0"' },
        body: { selection: five, games: [] },
      }),
      422,
      "invalid-request",
    );
    await call("PATCH", t.base, { token: t.organizer.token, body: { cancelled: true } });
    assertProblem(
      await call("PUT", `${t.base}/lineup`, {
        token: t.organizer.token,
        headers: { "If-Match": '"0"' },
        body: { selection: [], games: [] },
      }),
      409,
      "session-cancelled",
    );
    assertProblem(
      await call("PUT", `/slots/${t.slot}/sessions/${addDays(t.session, 2)}/lineup`, {
        token: t.organizer.token,
        headers: { "If-Match": '"0"' },
        body: { selection: [], games: [] },
      }),
      422,
      "not-a-session",
    );
  });
});
