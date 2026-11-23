// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

// Runs the Worker built by `worker-build --release` (build/) in Miniflare, as the workers-rs
// README recommends for testing a Rust Worker. Run `make build` first. Shared by the
// *.test.mjs files, so that each binding is declared once.

import { readFileSync } from "node:fs";
import { Miniflare } from "miniflare";

// Same compatibility date as the deployed Worker.
const compatibilityDate = readFileSync("wrangler.toml", "utf8").match(
  /^compatibility_date\s*=\s*"([^"]+)"/m,
)[1];

export function startWorker() {
  return new Miniflare({
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
}
