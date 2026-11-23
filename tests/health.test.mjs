// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { startWorker } from "./worker.mjs";

let mf;

before(() => {
  mf = startWorker();
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
