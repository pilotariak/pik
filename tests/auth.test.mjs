// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

// What every route shares: the token from Authorization or the cookie, the Origin check of
// cookie writes, problem+json errors, Cache-Control and the security headers.

import assert from "node:assert/strict";
import { after, before, describe, test } from "node:test";
import { COOKIE, ORIGIN, PROBLEM, assertProblem, startApi, world } from "./api.mjs";

let mf;
let call;

before(async () => {
  ({ mf, call } = await startApi());
});

after(async () => {
  await mf.dispose();
});

describe("token transport", () => {
  test("the cookie works for reads, without an Origin", async () => {
    const w = await world(call);
    const res = await call("GET", `/slots/${w.slot}`, { cookie: w.player.token });
    assert.equal(res.status, 200, res.text);
  });

  test("a write with the cookie needs Kancha's Origin", async () => {
    const w = await world(call);
    const path = `/slots/${w.slot}/sessions/${w.session}/answer`;
    const body = { status: "ok" };
    assertProblem(await call("PUT", path, { cookie: w.player.token, body }), 403, "origin-not-allowed");
    for (const origin of ["https://evil.example", "null", "https://kancha.pilotariak.com.evil.example"]) {
      assertProblem(
        await call("PUT", path, { cookie: w.player.token, body, headers: { Origin: origin } }),
        403,
        "origin-not-allowed",
      );
    }
    const ok = await call("PUT", path, { cookie: w.player.token, body, headers: { Origin: ORIGIN } });
    assert.equal(ok.status, 200, ok.text);
  });

  test("Authorization wins over the cookie and needs no Origin", async () => {
    const w = await world(call);
    const res = await call("PUT", `/slots/${w.slot}/sessions/${w.session}/answer`, {
      token: w.player.token,
      cookie: "x",
      body: { status: "dispo" },
      headers: { Origin: "https://evil.example" },
    });
    assert.equal(res.status, 200, res.text);
  });

  test("an unknown token in the cookie: 401, and the cookie is deleted", async () => {
    const res = await call("GET", "/me", { cookie: "f".repeat(52) });
    assertProblem(res, 401, "unauthorized");
    assert.equal(
      res.headers.get("set-cookie"),
      `${COOKIE}=; Path=/api; Max-Age=0; HttpOnly; Secure; SameSite=Lax`,
    );
    const bearer = await call("GET", "/me", { token: "f".repeat(52) });
    assert.equal(bearer.headers.get("set-cookie"), null, "a bearer token has no cookie to delete");
  });

  test("a join route takes an unknown cookie as no token, and replaces it", async () => {
    const w = await world(call);
    const res = await call("POST", `/join/${w.code}/members`, {
      cookie: "f".repeat(52),
      headers: { Origin: ORIGIN },
      body: { name: "Unai" },
    });
    assert.equal(res.status, 201, res.text);
    assert.ok(res.headers.get("set-cookie").startsWith(`${COOKIE}=${res.body.player_token};`));
  });

  test("a malformed Authorization header is an unknown token", async () => {
    for (const authorization of ["Basic abc", "Bearer", "Bearer short"]) {
      const res = await mf.dispatchFetch("http://localhost/api/v1/me", {
        headers: { Authorization: authorization },
      });
      assert.equal(res.status, 401, authorization);
      await res.body?.cancel();
    }
  });
});

describe("every API response", () => {
  test("sends Cache-Control: private, no-store and the security headers", async () => {
    const w = await world(call);
    for (const res of [
      await call("GET", `/slots/${w.slot}`, { token: w.player.token }),
      await call("GET", "/me"),
      await call("GET", "/health"),
    ]) {
      assert.equal(res.headers.get("cache-control"), "private, no-store");
      assert.equal(res.headers.get("x-content-type-options"), "nosniff");
      assert.equal(res.headers.get("x-frame-options"), "DENY");
      assert.equal(res.headers.get("referrer-policy"), "no-referrer");
      assert.equal(res.headers.get("cross-origin-resource-policy"), "same-origin");
      assert.match(res.headers.get("strict-transport-security"), /^max-age=\d+; includeSubDomains$/);
      assert.match(res.headers.get("content-security-policy"), /^default-src 'none'; frame-ancestors 'none'/);
    }
  });

  test("the router's 404 and 405 become problems under /api/", async () => {
    const notFound = await call("GET", "/nope");
    assertProblem(notFound, 404, "not-found");
    assert.equal(notFound.body.title, "Not found");
    const notAllowed = await call("POST", "/me");
    assertProblem(notAllowed, 405, "method-not-allowed");
    assert.equal(notAllowed.body.type, `${PROBLEM}method-not-allowed`);
  });

  test("outside /api/, the landing page keeps its own headers", async () => {
    const res = await mf.dispatchFetch("http://localhost/");
    assert.equal(res.headers.get("cache-control"), "public, max-age=300");
    assert.match(res.headers.get("content-security-policy"), /connect-src 'self'/);
    assert.equal(res.headers.get("x-content-type-options"), "nosniff");
    await res.body?.cancel();
  });
});
