// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

// Sessions and answers: GET /slots/:id/sessions, GET|PATCH /slots/:id/sessions/:date,
// PUT …/answer and PUT …/guests.

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

const HOUR = 3_600_000;

// Moves the answer deadline of a session to an instant in the past, through D1.
async function deadlinePassed(slot, date) {
  await db
    .prepare(
      `INSERT INTO session_overrides (slot_id, date, deadline_at) VALUES (?, ?, ?)
       ON CONFLICT (slot_id, date) DO UPDATE SET deadline_at = excluded.deadline_at`,
    )
    .bind(slot, date, Date.now() - HOUR)
    .run();
}

describe("GET /slots/:id/sessions", () => {
  test("access: members only", async () => {
    await checkAccess(call, {
      request: (w) => ["GET", `/slots/${w.slot}/sessions`],
      organizerOnly: false,
      ok: 200,
    });
  });

  test("sessions of a range, by date, with their states", async () => {
    const w = await world(call);
    const from = w.session;
    const to = addDays(w.session, 20);
    const res = await call("GET", `/slots/${w.slot}/sessions?from=${from}&to=${to}`, {
      token: w.player.token,
    });
    assert.equal(res.status, 200, res.text);
    assert.deepEqual(
      res.body.sessions.map((session) => [session.date, session.state, session.missing]),
      [0, 7, 14].map((days) => [addDays(w.session, days), "missing", 2]),
    );
    const defaults = await call("GET", `/slots/${w.slot}/sessions`, { token: w.player.token });
    assert.equal(defaults.body.sessions[0].date, w.session, "from today to the season end");
    assert.ok(defaults.body.sessions.length > 20);
  });

  test("the default range is valid after the season and before a long one", async () => {
    const today = parisToday();
    const over = await world(call, {
      season_start: addDays(today, -200),
      anchor_date: addDays(today, -190),
      season_end: addDays(today, -10),
    });
    const ended = await call("GET", `/slots/${over.slot}/sessions`, { token: over.player.token });
    assert.equal(ended.status, 200, ended.text);
    assert.deepEqual(ended.body.sessions, [], "no session after the season");
    const later = await world(call, {
      season_start: addDays(today, 30),
      anchor_date: addDays(today, 30),
      season_end: addDays(today, 396),
    });
    const upcoming = await call("GET", `/slots/${later.slot}/sessions`, { token: later.player.token });
    assert.equal(upcoming.status, 200, upcoming.text);
    assert.equal(upcoming.body.sessions.at(-1).date <= addDays(today, 366), true, "366 days at most");
  });

  test("an invalid range: 422", async () => {
    const w = await world(call);
    for (const query of [
      `from=${w.session}&to=${addDays(w.session, -1)}`,
      `from=${w.session}&to=${addDays(w.session, 400)}`,
      "from=tomorrow",
      "foo=1",
    ]) {
      assertProblem(
        await call("GET", `/slots/${w.slot}/sessions?${query}`, { token: w.player.token }),
        422,
        "invalid-request",
      );
    }
  });
});

describe("GET /slots/:id/sessions/:date", () => {
  test("access: members only", async () => {
    await checkAccess(call, {
      request: (w) => ["GET", `/slots/${w.slot}/sessions/${w.session}`],
      organizerOnly: false,
      ok: 200,
    });
  });

  test("a session without a lineup has the ETag \"0\"", async () => {
    const w = await world(call);
    const res = await call("GET", `/slots/${w.slot}/sessions/${w.session}`, { token: w.player.token });
    assert.equal(res.headers.get("etag"), '"0"');
    assert.deepEqual(res.body.lineup, { version: 0, selection: [], games: [] });
    assert.equal(res.body.date, w.session);
    assert.equal(res.body.cancelled, false);
    assert.equal(res.body.note, null);
    assert.deepEqual(
      res.body.no_answer.map((member) => [member.name, member.me]),
      [
        ["Maite", false],
        ["Peio", true],
      ],
    );
  });

  test("a date that is not a session of the slot: 422, after the access checks", async () => {
    const w = await world(call);
    const today = parisToday();
    for (const date of [addDays(w.session, 1), addDays(today, 210), addDays(today, -70), "2026-02-30", "tomorrow"]) {
      assertProblem(
        await call("GET", `/slots/${w.slot}/sessions/${date}`, { token: w.player.token }),
        422,
        "not-a-session",
      );
    }
    assertProblem(
      await call("GET", `/slots/${w.slot}/sessions/tomorrow`, { token: w.outsider.token }),
      404,
      "not-found",
    );
    assertProblem(await call("GET", `/slots/${w.slot}/sessions/tomorrow`), 401, "unauthorized");
  });

  test("answers by status, call order by sessions played, and the deadline in UTC", async () => {
    const today = parisToday();
    // Sessions 14 and 7 days ago, today, in 7 days…
    const w = await world(call, { anchor_date: addDays(today, -14), min_players: 2, places: 3 });
    const next = addDays(today, 7);
    const ana = await join(call, w.code, "Ana");
    const bixente = await join(call, w.code, "Bixente");
    // Ana played the two past sessions, Bixente none: written in D1, as the lineups of dates
    // whose deadline has passed.
    for (const date of [addDays(today, -14), addDays(today, -7)]) {
      await db.batch([
        db.prepare("INSERT INTO lineups (slot_id, date, updated_at) VALUES (?, ?, ?)").bind(w.slot, date, Date.now()),
        db
          .prepare("INSERT INTO lineup_entries (slot_id, date, position, member_id) VALUES (?, ?, 1, ?)")
          .bind(w.slot, date, ana.member),
      ]);
    }
    const answer = (token, status) =>
      call("PUT", `/slots/${w.slot}/sessions/${next}/answer`, { token, body: { status } });
    assert.equal((await answer(ana.token, "dispo")).status, 200);
    assert.equal((await answer(w.player.token, "ok")).status, 200);
    assert.equal((await answer(bixente.token, "dispo")).status, 200);
    assert.equal((await answer(w.organizer.token, "ko")).status, 200);

    const res = await call("GET", `/slots/${w.slot}/sessions/${next}`, { token: w.organizer.token });
    assert.equal(res.status, 200, res.text);
    assert.deepEqual(
      res.body.answers.map((a) => [a.name, a.status, a.played, a.me]),
      [
        ["Peio", "ok", 0, false],
        ["Ana", "dispo", 2, false],
        ["Bixente", "dispo", 0, false],
        ["Maite", "ko", 0, true],
      ],
    );
    assert.deepEqual(
      res.body.call_order,
      [w.player.member, bixente.member, ana.member],
      "OK first, then Dispo with the fewest sessions played",
    );
    assert.deepEqual(res.body.no_answer, []);
    assert.deepEqual(res.body.counts, { ok: 1, dispo: 2, ko: 1, none: 0 });
    assert.equal(res.body.state, "missing");
    assert.equal(res.body.missing, 1);
    // The deadline is the day before at 20:00, Paris time, sent in UTC.
    const deadline = new Date(res.body.deadline);
    const paris = new Intl.DateTimeFormat("en-GB", {
      timeZone: "Europe/Paris",
      hour: "2-digit",
      minute: "2-digit",
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    }).format(deadline);
    const [d, m, y] = addDays(next, -1).split("-").reverse();
    assert.equal(paris, `${d}/${m}/${y}, 20:00`);
    assert.match(res.body.deadline, /Z$/, "RFC 3339 in UTC");
  });
});

describe("PATCH /slots/:id/sessions/:date", () => {
  test("access: organizer only", async () => {
    await checkAccess(call, {
      request: (w) => ["PATCH", `/slots/${w.slot}/sessions/${w.session}`, { body: { note: "Fronton" } }],
      organizerOnly: true,
      ok: 200,
    });
  });

  test("cancel, then uncancel a date", async () => {
    const w = await world(call);
    const path = `/slots/${w.slot}/sessions/${w.session}`;
    const cancelled = await call("PATCH", path, {
      token: w.organizer.token,
      body: { cancelled: true, note: "Trinquet fermé" },
    });
    assert.equal(cancelled.status, 200, cancelled.text);
    assert.equal(cancelled.body.state, "cancelled");
    assert.equal(cancelled.body.note, "Trinquet fermé");
    assertProblem(
      await call("PUT", `${path}/answer`, { token: w.player.token, body: { status: "ok" } }),
      409,
      "session-cancelled",
    );
    const back = await call("PATCH", path, { token: w.organizer.token, body: { cancelled: false } });
    assert.equal(back.body.state, "missing");
    assert.equal(back.body.note, "Trinquet fermé", "the note stays");
    const reset = await call("PATCH", path, { token: w.organizer.token, body: { note: null } });
    assert.equal(reset.body.note, null);
  });

  test("minimum, places and deadline for one date; null goes back to the slot", async () => {
    const w = await world(call);
    const path = `/slots/${w.slot}/sessions/${w.session}`;
    const deadline = new Date(Date.now() + 48 * HOUR).toISOString();
    const res = await call("PATCH", path, {
      token: w.organizer.token,
      body: { min_players: 3, places: 6, deadline_at: deadline },
    });
    assert.equal(res.status, 200, res.text);
    assert.deepEqual([res.body.min_players, res.body.places, res.body.missing], [3, 6, 3]);
    assert.equal(new Date(res.body.deadline).getTime(), new Date(deadline).getTime());
    const reset = await call("PATCH", path, {
      token: w.organizer.token,
      body: { min_players: null, places: null, deadline_at: null },
    });
    assert.deepEqual([reset.body.min_players, reset.body.places], [2, 4], "the slot's values");
    const other = await call("GET", `/slots/${w.slot}/sessions/${addDays(w.session, 7)}`, {
      token: w.organizer.token,
    });
    assert.equal(other.body.min_players, 2, "other dates keep the slot's values");
  });

  test("invalid changes: 422", async () => {
    const w = await world(call);
    const path = `/slots/${w.slot}/sessions/${w.session}`;
    for (const body of [{ min_players: 5 }, { places: 0 }, { deadline_at: "demain" }, { interval_weeks: 2 }, { note: "x".repeat(201) }]) {
      assertProblem(await call("PATCH", path, { token: w.organizer.token, body }), 422, "invalid-request");
    }
    assertProblem(
      await call("PATCH", `/slots/${w.slot}/sessions/${addDays(w.session, 3)}`, {
        token: w.organizer.token,
        body: { cancelled: true },
      }),
      422,
      "not-a-session",
    );
  });
});

describe("PUT /slots/:id/sessions/:date/answer", () => {
  test("access: members answer for themselves", async () => {
    await checkAccess(call, {
      request: (w) => ["PUT", `/slots/${w.slot}/sessions/${w.session}/answer`, { body: { status: "ok" } }],
      organizerOnly: false,
      ok: 200,
    });
  });

  test("the response carries the recomputed counts and state", async () => {
    const w = await world(call);
    const path = `/slots/${w.slot}/sessions/${w.session}/answer`;
    const first = await call("PUT", path, { token: w.player.token, body: { status: "ok" } });
    assert.deepEqual(
      [first.body.member_id, first.body.status, first.body.session.state, first.body.session.missing],
      [w.player.member, "ok", "missing", 1],
    );
    const second = await call("PUT", path, { token: w.organizer.token, body: { status: "ok" } });
    assert.equal(second.body.session.state, "on", "two OK answers reach the minimum of 2");
    assert.deepEqual(second.body.session.counts, { ok: 2, dispo: 0, ko: 0, none: 0 });
    assert.equal(second.body.session.my_answer, "ok");
  });

  test("the same answer again keeps its place in the call order", async () => {
    const w = await world(call);
    const path = `/slots/${w.slot}/sessions/${w.session}/answer`;
    await call("PUT", path, { token: w.player.token, body: { status: "ok" } });
    const read = async () =>
      (await db.prepare("SELECT answered_at FROM answers WHERE member_id = ?").bind(w.player.member).first())
        .answered_at;
    const before = await read();
    await new Promise((resolve) => setTimeout(resolve, 5));
    await call("PUT", path, { token: w.player.token, body: { status: "ok" } });
    assert.equal(await read(), before, "unchanged status: same answered_at");
    await call("PUT", path, { token: w.player.token, body: { status: "dispo" } });
    assert.ok((await read()) > before, "a new status moves answered_at");
  });

  test("an organizer answers for a member who never joined; a player cannot", async () => {
    const w = await world(call);
    const path = `/slots/${w.slot}/sessions/${w.session}/answer`;
    const ana = await call("POST", `/slots/${w.slot}/members`, { token: w.organizer.token, body: { name: "Ana" } });
    const res = await call("PUT", path, {
      token: w.organizer.token,
      body: { status: "dispo", member_id: ana.body.id },
    });
    assert.equal(res.status, 200, res.text);
    assert.equal(res.body.member_id, ana.body.id);
    const row = await db
      .prepare("SELECT status, set_by FROM answers WHERE member_id = ?")
      .bind(ana.body.id)
      .first();
    assert.deepEqual(row, { status: "dispo", set_by: w.organizer.member });
    assertProblem(
      await call("PUT", path, { token: w.player.token, body: { status: "ko", member_id: ana.body.id } }),
      403,
      "organizer-only",
    );
    const own = await call("PUT", path, {
      token: w.player.token,
      body: { status: "ko", member_id: w.player.member },
    });
    assert.equal(own.status, 200, "a player may name themselves");
    assertProblem(
      await call("PUT", path, {
        token: w.organizer.token,
        body: { status: "ok", member_id: w.outsider.member },
      }),
      422,
      "invalid-request",
    );
  });

  test("after the deadline only ko is accepted", async () => {
    const w = await world(call);
    const path = `/slots/${w.slot}/sessions/${w.session}/answer`;
    await call("PUT", path, { token: w.player.token, body: { status: "ok" } });
    await deadlinePassed(w.slot, w.session);
    for (const status of ["ok", "dispo"]) {
      assertProblem(
        await call("PUT", path, { token: w.player.token, body: { status } }),
        409,
        "deadline-passed",
      );
      assertProblem(
        await call("PUT", path, { token: w.organizer.token, body: { status, member_id: w.player.member } }),
        409,
        "deadline-passed",
      );
    }
    const ko = await call("PUT", path, { token: w.player.token, body: { status: "ko" } });
    assert.equal(ko.status, 200, ko.text);
    assert.equal(ko.body.session.deadline_passed, true);
    assert.equal(ko.body.session.state, "cancelled_short", "nobody left at the deadline");
  });

  test("at the deadline, Dispo answers make a session to complete", async () => {
    const w = await world(call, { min_players: 2 });
    const path = `/slots/${w.slot}/sessions/${w.session}/answer`;
    await call("PUT", path, { token: w.player.token, body: { status: "ok" } });
    await call("PUT", path, { token: w.organizer.token, body: { status: "dispo" } });
    await deadlinePassed(w.slot, w.session);
    const res = await call("GET", `/slots/${w.slot}/sessions/${w.session}`, { token: w.player.token });
    assert.equal(res.body.state, "to_complete");
  });

  test("a date that is not a session, or an invalid status: 422", async () => {
    const w = await world(call);
    assertProblem(
      await call("PUT", `/slots/${w.slot}/sessions/${addDays(w.session, 1)}/answer`, {
        token: w.player.token,
        body: { status: "ok" },
      }),
      422,
      "not-a-session",
    );
    assertProblem(
      await call("PUT", `/slots/${w.slot}/sessions/${w.session}/answer`, {
        token: w.player.token,
        body: { status: "maybe" },
      }),
      422,
      "invalid-request",
    );
  });
});

describe("PUT /slots/:id/sessions/:date/guests", () => {
  test("access: organizer only", async () => {
    await checkAccess(call, {
      request: (w) => ["PUT", `/slots/${w.slot}/sessions/${w.session}/guests`, { body: { guests: [{ name: "Unai" }] } }],
      organizerOnly: true,
      ok: 200,
    });
  });

  test("replaces the guests; a kept guest keeps their id, so a retry changes nothing", async () => {
    const w = await world(call);
    const path = `/slots/${w.slot}/sessions/${w.session}/guests`;
    const first = await call("PUT", path, {
      token: w.organizer.token,
      body: { guests: [{ name: "Unai" }, { name: "Txomin" }] },
    });
    assert.equal(first.status, 200, first.text);
    assert.deepEqual(first.body.guests.map((g) => g.name), ["Unai", "Txomin"]);
    const retry = await call("PUT", path, {
      token: w.organizer.token,
      body: { guests: [{ name: "Unai" }, { name: "Txomin" }] },
    });
    assert.deepEqual(retry.body, first.body, "same ids");
    const second = await call("PUT", path, { token: w.organizer.token, body: { guests: [{ name: "unai" }] } });
    assert.deepEqual(second.body.guests, [first.body.guests[0]], "Unai kept, Txomin removed");
    const session = await call("GET", `/slots/${w.slot}/sessions/${w.session}`, { token: w.player.token });
    assert.deepEqual(session.body.guests, second.body.guests);
    assertProblem(
      await call("PUT", path, { token: w.organizer.token, body: { guests: [{ name: "A" }, { name: "a" }] } }),
      422,
      "invalid-request",
    );
  });

  test("removing a guest who is in the lineup moves its ETag", async () => {
    const w = await world(call);
    const base = `/slots/${w.slot}/sessions/${w.session}`;
    const guests = await call("PUT", `${base}/guests`, { token: w.organizer.token, body: { guests: [{ name: "Unai" }] } });
    const lineup = await call("PUT", `${base}/lineup`, {
      token: w.organizer.token,
      headers: { "If-Match": '"0"' },
      body: { selection: [{ guest: guests.body.guests[0].id }, { member: w.player.member }], games: [] },
    });
    assert.equal(lineup.headers.get("etag"), '"1"');
    await call("PUT", `${base}/guests`, { token: w.organizer.token, body: { guests: [] } });
    const session = await call("GET", base, { token: w.organizer.token });
    assert.equal(session.headers.get("etag"), '"2"');
    assert.deepEqual(session.body.lineup.selection, [{ member: w.player.member }]);
  });

  test("a cancelled session takes no guest: 409", async () => {
    const w = await world(call);
    const base = `/slots/${w.slot}/sessions/${w.session}`;
    await call("PATCH", base, { token: w.organizer.token, body: { cancelled: true } });
    assertProblem(
      await call("PUT", `${base}/guests`, { token: w.organizer.token, body: { guests: [{ name: "Unai" }] } }),
      409,
      "session-cancelled",
    );
  });
});
