// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

// CORS of the API for the Kancha website (src/api/cors.rs). The Worker runs with the [vars] of
// wrangler.toml, the production list, so the development origin is an unlisted one here.

import assert from "node:assert/strict";
import { after, before, describe, test } from "node:test";
import { ORIGIN, assertProblem, nextIp, slotBody, startApi, world } from "./api.mjs";
import { allowedOrigins } from "./worker.mjs";

const DEV_ORIGIN = "http://localhost:8081";
const UNLISTED = [
  "https://evil.example",
  "null",
  DEV_ORIGIN,
  `${ORIGIN}.evil.example`,
  "http://kancha.pilotariak.com",
];
const CORS_HEADERS = [
  "access-control-allow-origin",
  "access-control-allow-credentials",
  "access-control-allow-methods",
  "access-control-allow-headers",
  "access-control-expose-headers",
  "access-control-max-age",
];

let mf;
let call;

before(async () => {
  ({ mf, call } = await startApi());
});

after(async () => {
  await mf.dispose();
});

function assertVary(res) {
  assert.match(res.headers.get("vary") ?? "", /\bOrigin\b/, "the response varies on Origin");
}

function assertCors(res, origin) {
  assert.equal(res.headers.get("access-control-allow-origin"), origin, "the origin is echoed, never *");
  assert.equal(res.headers.get("access-control-allow-credentials"), "true", "credentials are allowed");
  assertVary(res);
}

function assertNoCors(res, why) {
  for (const name of CORS_HEADERS) {
    assert.equal(res.headers.get(name), null, `${why}: no ${name}`);
  }
}

// A preflight as a browser sends it.
function preflight(path, origin, method = "POST") {
  return call("OPTIONS", path, {
    headers: {
      Origin: origin,
      "Access-Control-Request-Method": method,
      "Access-Control-Request-Headers": "content-type,idempotency-key",
    },
  });
}

describe("allowed origins", () => {
  test("production allows only https://kancha.pilotariak.com", () => {
    assert.deepEqual(allowedOrigins, ["https://kancha.pilotariak.com"], "ALLOWED_ORIGINS of wrangler.toml");
  });
});

describe("responses", () => {
  test("an allowed origin is echoed, with credentials and the headers Kancha reads", async () => {
    const w = await world(call);
    for (const res of [
      await call("GET", "/health", { headers: { Origin: ORIGIN } }),
      await call("GET", `/slots/${w.slot}`, { token: w.player.token, headers: { Origin: ORIGIN } }),
      await call("PUT", `/slots/${w.slot}/sessions/${w.session}/answer`, {
        cookie: w.player.token,
        body: { status: "ok" },
        headers: { Origin: ORIGIN },
      }),
    ]) {
      assert.equal(res.status, 200, res.text);
      assertCors(res, ORIGIN);
      assert.equal(res.headers.get("access-control-expose-headers"), "Retry-After, ETag");
      assert.equal(res.headers.get("access-control-allow-methods"), null, "preflight headers only on a preflight");
    }
  });

  test("an unlisted origin gets no CORS header", async () => {
    for (const origin of UNLISTED) {
      const res = await call("GET", "/health", { headers: { Origin: origin } });
      assert.equal(res.status, 200, res.text);
      assertNoCors(res, origin);
      assertVary(res);
    }
  });

  test("a request without Origin, like the native app's, gets no CORS header", async () => {
    const res = await call("GET", "/health");
    assert.equal(res.status, 200, res.text);
    assertNoCors(res, "no Origin");
    assertVary(res);
  });

  test("a cookie write from the development origin gets 403 in production, without CORS headers", async () => {
    const w = await world(call);
    const res = await call("PUT", `/slots/${w.slot}/sessions/${w.session}/answer`, {
      cookie: w.player.token,
      body: { status: "ok" },
      headers: { Origin: DEV_ORIGIN },
    });
    assertProblem(res, 403, "origin-not-allowed");
    assertNoCors(res, DEV_ORIGIN);
  });

  test("outside /api/, the landing page sends no CORS header", async () => {
    const res = await mf.dispatchFetch("http://localhost/", { headers: { Origin: ORIGIN } });
    assert.equal(res.status, 200);
    assertNoCors(res, "landing page");
    await res.body?.cancel();
  });
});

describe("problems", () => {
  test("a 422 carries the CORS headers, so the page can read the problem", async () => {
    const res = await call("POST", "/slots", { body: "not json", headers: { Origin: ORIGIN } });
    assertProblem(res, 422, "invalid-request");
    assertCors(res, ORIGIN);
  });

  test("the router's 404 and 405 carry them too", async () => {
    const notFound = await call("GET", "/nope", { headers: { Origin: ORIGIN } });
    assertProblem(notFound, 404, "not-found");
    assertCors(notFound, ORIGIN);
    const notAllowed = await call("POST", "/me", { headers: { Origin: ORIGIN } });
    assertProblem(notAllowed, 405, "method-not-allowed");
    assertCors(notAllowed, ORIGIN);
  });

  test("a 429 exposes Retry-After", async () => {
    const ip = nextIp();
    const headers = { Origin: ORIGIN };
    for (let i = 0; i < 5; i += 1) {
      assert.equal((await call("POST", "/slots", { ip, body: slotBody(), headers })).status, 201);
    }
    const limited = await call("POST", "/slots", { ip, body: slotBody(), headers });
    assertProblem(limited, 429, "rate-limited");
    assertCors(limited, ORIGIN);
    assert.match(limited.headers.get("access-control-expose-headers"), /\bRetry-After\b/);
    assert.equal(limited.headers.get("retry-after"), "60");
  });
});

describe("preflight", () => {
  test("a preflight from an allowed origin answers 204 before the router's 405", async () => {
    const w = await world(call);
    for (const [path, method] of [
      ["/slots", "POST"],
      // GET only: the router alone would answer 405.
      ["/me", "GET"],
      [`/slots/${w.slot}`, "PATCH"],
      [`/slots/${w.slot}/sessions/${w.session}/lineup`, "PUT"],
      [`/slots/${w.slot}/members/me`, "DELETE"],
    ]) {
      const res = await preflight(path, ORIGIN, method);
      assert.equal(res.status, 204, `${method} ${path}: ${res.text}`);
      assert.equal(res.text, "", "a preflight has no body");
      assertCors(res, ORIGIN);
      assert.equal(res.headers.get("access-control-allow-methods"), "GET, POST, PUT, PATCH, DELETE");
      assert.equal(
        res.headers.get("access-control-allow-headers"),
        "Content-Type, Idempotency-Key, Authorization, If-Match",
      );
      assert.equal(res.headers.get("access-control-max-age"), "7200");
      assert.equal(res.headers.get("cache-control"), "private, no-store");
      assert.equal(res.headers.get("x-content-type-options"), "nosniff");
    }
  });

  test("a preflight from an unlisted origin, or without one, gets the router's 405 without CORS headers", async () => {
    for (const origin of UNLISTED) {
      const res = await preflight("/slots", origin);
      assertProblem(res, 405, "method-not-allowed");
      assertNoCors(res, origin);
      assertVary(res);
    }
    const bare = await call("OPTIONS", "/slots");
    assertProblem(bare, 405, "method-not-allowed");
    assertNoCors(bare, "no Origin");
  });
});
