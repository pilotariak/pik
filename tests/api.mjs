// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

// Shared by the API tests (*.test.mjs): a Worker with a migrated D1, a request helper, Paris
// dates, fixtures, and the access check that every /slots/:id route runs for an organizer, a
// player, a member of another slot and a caller without a token.
//
// Time-dependent rules are tested by writing past instants in D1 through the binding (deadlines,
// expiries), never by waiting: the Worker has no way to change its clock.

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { applyMigrations, startWorker } from "./worker.mjs";

export const ORIGIN = "https://kancha.pilotariak.com";
export const COOKIE = "__Secure-pik_token";
export const PROBLEM = "https://pik.pilotariak.com/problems/";

// Each request gets its own client address unless a test sets one, so that the rate limits of
// one test do not reach another.
let lastIp = 0;
export function nextIp() {
  lastIp += 1;
  return `10.${(lastIp >> 16) & 255}.${(lastIp >> 8) & 255}.${lastIp & 255}`;
}

export function sha256(text) {
  return createHash("sha256").update(text).digest();
}

// Today in Europe/Paris, YYYY-MM-DD.
export function parisToday() {
  return new Intl.DateTimeFormat("en-CA", {
    timeZone: "Europe/Paris",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).format(new Date());
}

export function addDays(date, days) {
  const day = new Date(`${date}T00:00:00Z`);
  day.setUTCDate(day.getUTCDate() + days);
  return day.toISOString().slice(0, 10);
}

export async function startApi() {
  const mf = startWorker();
  const db = await mf.getD1Database("DB");
  await applyMigrations(db);

  // Sends a request to /api/v1 and reads the response. `token` goes in Authorization, `cookie`
  // in Cookie.
  async function call(method, path, { token, cookie, body, headers = {}, ip } = {}) {
    const sent = { "CF-Connecting-IP": ip ?? nextIp(), ...headers };
    if (token) sent.Authorization = `Bearer ${token}`;
    if (cookie) sent.Cookie = `${COOKIE}=${cookie}`;
    if (body !== undefined) sent["Content-Type"] = "application/json";
    const res = await mf.dispatchFetch(`http://localhost/api/v1${path}`, {
      method,
      headers: sent,
      body: body === undefined ? undefined : typeof body === "string" ? body : JSON.stringify(body),
    });
    const text = await res.text();
    let json = null;
    try {
      json = text ? JSON.parse(text) : null;
    } catch {
      json = null;
    }
    return { status: res.status, headers: res.headers, body: json, text };
  }

  return { mf, db, call };
}

// A weekly slot whose first session is in a week, in a season around today.
export function slotBody(changes = {}) {
  const today = parisToday();
  return {
    name: "Mercredi",
    venue: "Trinquet",
    court: "trinquet",
    specialty: "gomme_creuse",
    anchor_date: addDays(today, 7),
    interval_weeks: 1,
    time_from: "18:00",
    time_to: "19:30",
    season_start: addDays(today, -60),
    season_end: addDays(today, 200),
    min_players: 2,
    places: 4,
    organizer_name: "Maite",
    ...changes,
  };
}

// Creates a slot without a token: its creator gets a new player.
export async function createSlot(call, changes = {}) {
  const res = await call("POST", "/slots", { body: slotBody(changes) });
  assert.equal(res.status, 201, `slot created: ${res.text}`);
  return {
    slot: res.body.slot_id,
    code: res.body.invite_code,
    organizer: { token: res.body.player_token, member: res.body.member_id },
  };
}

// Joins a slot with a new name, from a new device unless a token is given.
export async function join(call, code, name, token) {
  const res = await call("POST", `/join/${code}/members`, { token, body: { name } });
  assert.equal(res.status, 201, `${name} joined: ${res.text}`);
  return { token: res.body.player_token ?? token, member: res.body.member_id };
}

// A slot with an organizer and a player, and an outsider: the organizer of another slot.
export async function world(call, changes = {}) {
  const { slot, code, organizer } = await createSlot(call, changes);
  const player = await join(call, code, "Peio");
  const other = await createSlot(call, { name: "Jeudi", organizer_name: "Ana" });
  return {
    slot,
    code,
    organizer,
    player,
    outsider: { ...other.organizer, slot: other.slot },
    session: changes.anchor_date ?? slotBody().anchor_date,
  };
}

export function assertProblem(res, status, type) {
  assert.equal(res.status, status, `status ${status}: ${res.text}`);
  assert.match(res.headers.get("content-type") ?? "", /^application\/problem\+json/);
  assert.equal(res.body?.type, `${PROBLEM}${type}`, `problem type ${type}`);
  assert.equal(res.body?.status, status, "the problem repeats the status");
}

// The access rules of a /slots/:id route: 401 without a token, 404 for a member of another
// slot, 403 for a player on an organizer route. `request(w)` returns [method, path, options]
// for a fresh world `w`; the organizer's (and, on member routes, the player's) call must
// answer `ok`.
export async function checkAccess(call, { request, organizerOnly, ok, changes }) {
  const w = await world(call, changes);
  const [method, path, options = {}] = request(w);

  assertProblem(await call(method, path, options), 401, "unauthorized");
  assertProblem(
    await call(method, path, { ...options, token: "a".repeat(52) }),
    401,
    "unauthorized",
  );
  assertProblem(await call(method, path, { ...options, token: w.outsider.token }), 404, "not-found");
  const asPlayer = await call(method, path, { ...options, token: w.player.token });
  if (organizerOnly) {
    assertProblem(asPlayer, 403, "organizer-only");
  } else {
    assert.equal(asPlayer.status, ok, `the player gets ${ok}: ${asPlayer.text}`);
  }
  const fresh = organizerOnly ? w : await world(call, changes);
  const [m, p, o = {}] = organizerOnly ? [method, path, options] : request(fresh);
  const asOrganizer = await call(m, p, { ...o, token: fresh.organizer.token });
  assert.equal(asOrganizer.status, ok, `the organizer gets ${ok}: ${asOrganizer.text}`);
  return { world: w, response: asOrganizer };
}
