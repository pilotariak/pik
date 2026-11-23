// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

// Runs the Worker built by `worker-build --release` (build/) in Miniflare, as the workers-rs
// README recommends for testing a Rust Worker. Run `make build` first.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { after, before, test } from "node:test";
import { Miniflare } from "miniflare";

// Same compatibility date as the deployed Worker.
const compatibilityDate = readFileSync("wrangler.jsonc", "utf8").match(
  /"compatibility_date":\s*"([^"]+)"/,
)[1];

let mf;

before(() => {
  mf = new Miniflare({
    workers: [
      {
        config: {
          name: "pik",
          compatibilityDate,
          manifest: {
            mainModule: "build/index.js",
            modules: {
              "build/index.js": {
                type: "esm",
                contents: readFileSync("build/index.js", "utf8"),
              },
              "build/index_bg.wasm": {
                type: "wasm",
                contents: new Uint8Array(readFileSync("build/index_bg.wasm")),
              },
            },
          },
          env: {
            DB: { type: "d1", id: "DB" },
          },
        },
      },
    ],
  });
});

after(async () => {
  await mf.dispose();
});

test("GET /api/v1/health answers 200 with status ok", async () => {
  const res = await mf.dispatchFetch("http://localhost/api/v1/health");
  assert.equal(res.status, 200);
  assert.match(res.headers.get("content-type"), /^application\/json/);
  assert.deepEqual(await res.json(), { status: "ok" });
});

for (const path of ["/health", "/api/health"]) {
  test(`GET ${path} (outside /api/v1/) answers 404`, async () => {
    const res = await mf.dispatchFetch(`http://localhost${path}`);
    assert.equal(res.status, 404);
    await res.body?.cancel();
  });
}
