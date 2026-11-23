// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

// Runs the Worker built by `worker-build --release` (build/) in Miniflare, as the workers-rs
// README recommends for testing a Rust Worker. Run `make build` first. Shared by the
// *.test.mjs files, so that each binding is declared once.

import { readdirSync, readFileSync } from "node:fs";
import { Miniflare } from "miniflare";
import { experimental_readRawConfig as readRawConfig, unstable_splitSqlQuery as splitSqlQuery } from "wrangler";

const { rawConfig } = readRawConfig({ config: "wrangler.toml" });

// Same compatibility date as the deployed Worker.
const compatibilityDate = rawConfig.compatibility_date;

// The rate limiters of wrangler.toml, with the same limits and periods.
const rateLimiters = Object.fromEntries(
  rawConfig.ratelimits.map(({ name, namespace_id: namespace, simple }) => [
    name,
    { type: "rate-limit", namespace, simple },
  ]),
);

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
            ...rateLimiters,
          },
        },
      },
    ],
  });
}

const migrations = "database/migrations";

// Applies database/migrations/*.sql to a D1 binding the way `wrangler d1 migrations apply
// --local` does: wrangler's SQL splitter, then one batch per file, without the d1_migrations
// bookkeeping.
export async function applyMigrations(db) {
  const files = readdirSync(migrations)
    .filter((file) => file.endsWith(".sql"))
    .sort();
  for (const file of files) {
    const statements = splitSqlQuery(readFileSync(`${migrations}/${file}`, "utf8")).filter(
      // 0001 holds comments only.
      (statement) => statement.replace(/--.*$/gm, "").trim() !== "",
    );
    if (statements.length > 0) {
      await db.batch(statements.map((statement) => db.prepare(statement)));
    }
  }
}
