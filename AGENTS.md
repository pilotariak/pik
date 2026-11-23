# AGENTS.md — pik

**pik** is the slots service behind the Créneaux tab of Kancha. A slot is a court that a group of friends books at a
fixed time, every week or every other week. For each session, each player answers whether they come, and the session
takes place once enough players answer OK.

pik is a Cloudflare Worker written in Rust with [workers-rs](https://github.com/cloudflare/workers-rs), and a D1
database, served at `https://pik.pilotariak.com`. Kancha calls pik for slot data: it does not go through Frontis.

## Decisions

- [Frontis ADR-0001](https://github.com/pilotariak/frontis/pull/113): why slot data lives in pik, outside Frontis.
- [Kancha ADR-0001](https://github.com/pilotariak/kancha/pull/154): how Kancha calls pik. The same pull request holds
  the proposed ADRs on player identity (ADR-0002) and on serving pik under the Kancha web origin (ADR-0004).

## Architecture

```text
pik/
├── src/
│   ├── lib.rs              # Worker entry: #[event(fetch)] wires the api and web routes
│   ├── api/
│   │   ├── mod.rs          # routes(): the /api/v1 prefix (v1! macro), one module per resource
│   │   └── health.rs       # GET /api/v1/health
│   ├── domain/
│   │   ├── mod.rs          # Pure logic and types, no worker import: unit tests run by cargo test on the host
│   │   └── calendar/
│   │       ├── mod.rs        # Europe/Paris rules embedded by jiff, local date and time to instant
│   │       ├── recurrence.rs # Session dates: anchor_date + k × interval, up to season_end
│   │       ├── deadline.rs   # Answer deadline of a session, Paris time
│   │       ├── session.rs    # Effective values (slot or session_overrides) and state of a session
│   │       └── call_order.rs # Call order, and sessions played from lineup_entries
│   └── web/
│       ├── mod.rs          # routes(): the public pages outside /api/
│       ├── landing.rs      # GET /: version, CSP and Cache-Control
│       └── landing.html    # The landing page, embedded with include_str!
├── database/
│   └── migrations/         # D1 migrations, applied in order by wrangler
├── docs/how-to/
│   └── howto-database.md   # Manage the D1 database: migrations, local and remote queries
├── tests/
│   ├── worker.mjs          # Starts build/ in Miniflare, shared by the test files
│   ├── health.test.mjs     # Worker tests, run with node:test
│   ├── landing.test.mjs
│   └── schema.test.mjs     # D1 tests: apply the migrations in Miniflare, check foreign keys and cascades
├── wrangler.jsonc          # Worker config: build command, Custom Domain, D1 binding
├── Cargo.toml              # Crate (version managed by release-please)
├── rust-toolchain.toml     # Pinned Rust toolchain and the wasm32-unknown-unknown target
└── package.json            # wrangler and miniflare (pinned), dev, deploy, migration and query scripts
```

`worker-build` compiles the crate to `wasm32-unknown-unknown` and writes `build/index.js` and `build/index_bg.wasm`.
`wrangler dev` and `wrangler deploy` run it through the `build` command of `wrangler.jsonc`.

## Module layout

One crate split into modules, in the `mod.rs` style. Modules are private or `pub(crate)`: the crate is a `cdylib`, so
nothing needs `pub`. New code goes here:

- **`api/`**: one file per resource, named like the resource cards of the landing page and the OpenAPI tags
  (`slots.rs`, `join.rs`, `members.rs`, `sessions.rs`, `lineups.rs`, `recovery.rs`). A file holds the resource's
  handlers, its request and response types, and its `#[utoipa::path]` annotations. `api/mod.rs` registers them with
  `v1!("/path")`, the only place the version appears; a breaking change adds `api/v2/` for the handlers that change
  only. Code shared by the routes gets its own file: `auth.rs` (player token, `Origin` check, organizer role),
  `problem.rs` (RFC 9457 errors), `idempotency.rs`, `openapi.rs`.
- **`domain/`**: pure logic and types: the calendar (`calendar/`: session dates, deadlines, effective values, session
  states, call order, sessions played), ids, invite code, player token. It never imports `worker`, so `cargo test` runs
  it on the host.
- **`db/`**: D1 queries, one file per table, returning `domain` types.
- **`web/`**: the public pages outside `/api/`.
- **`purge.rs`**: the daily Cron of #5, called from an `#[event(scheduled)]` handler in `lib.rs`.

Dependencies point one way: `api` uses `db` and `domain`, `db` uses `domain`, `domain` uses neither. workers-rs has
no middleware: what every response needs (security headers, `Cache-Control: private, no-store`, problem+json instead
of the Router's plain-text 404 and 405 on `/api/*`) goes after `router.run()` in `fetch`. Tests follow the same split:
`tests/<resource>.test.mjs`.

## Routes

Every API route lives under `/api/v1/`. Installed app versions stay in use for months, so a breaking change goes to
`/api/v2/` while `/api/v1/` keeps working. If Kancha web forwards `/api/*` to pik unchanged (kancha ADR-0004, still
proposed), a cookie with the path `/api` covers every version whether a client calls pik directly or through the
Kancha origin. Only the landing page lives outside `/api/`. The workers-rs `Router` answers 405 to any other method
on a registered path.

| Method | Path             | Description                                                         |
| ------ | ---------------- | ------------------------------------------------------------------- |
| `GET`  | `/`              | Landing page (HTML): what pik is, its resources, live health status |
| `GET`  | `/api/v1/health` | Liveness check: `{"status":"ok"}`                                   |

### Landing page

`src/web/landing.html` is one HTML document with inline CSS and script, no build step, in the style of the Frontis,
Azkena and Xilo landing pages (design tokens from frontis `DESIGN.md`).

- `__PIK_VERSION__` is replaced with `CARGO_PKG_VERSION` when the page is served.
- `GET /` sends `Content-Security-Policy` (Azkena's policy plus `connect-src 'self'` for the status script) and
  `Cache-Control: public, max-age=300`: the page holds no private data. A new origin on the page needs a CSP change.
- The resource cards follow the routes of #4. Keep them in step with the OpenAPI contract, and never put slot,
  member or answer data, or real player or venue names, on the page.

## Rules the next issues must keep

- **No account.** pik has no sign-up, no sign-in, no password, no email and no phone number. How players are identified
  without accounts is the subject of kancha ADR-0002, still proposed: follow it once it is accepted.
- **Private data.** Slot data is personal data of a group of friends: first names or nicknames, answers, lineups. Only
  the members of a slot read it. Never log a token or a player's data, store only a hash of a token, and delete a
  slot's data once it is no longer needed.
- **Dates in `Europe/Paris`.** Session dates, answer deadlines and the purge are computed in `Europe/Paris`. Cron
  Triggers run in UTC, and the offset is one hour in winter and two in summer.
- **API routes under `/api/v1/`**, as above. Only the landing page (`GET /`) lives outside it.

## Dev Setup

Requirements:

- [rustup](https://rustup.rs). `rust-toolchain.toml` pins the toolchain, rustfmt, clippy and the wasm target.
- [Node.js](https://nodejs.org) ≥ 22.12 (required by Wrangler).
- [worker-build](https://crates.io/crates/worker-build) 0.9.0, the same version as the `worker` crate.

```bash
make install         # rustup toolchain install, cargo install worker-build, npm ci
```

## Running Locally

```bash
make migrate-local   # npm run migrate:local: apply the migrations to the local D1
make dev             # npm run dev: wrangler dev --persist-to .wrangler/state  →  http://localhost:8787
```

```bash
curl http://localhost:8787/api/v1/health
```

## Database

Managed like the Frontis databases. See [How to manage the D1 database](docs/how-to/howto-database.md).

- Binding `DB`, database `pilotariak-pik` (named like the Frontis databases, `pilotariak-<name>`), migrations in
  `database/migrations/`. Commands call the database by its name.
- The database is restricted to the EU jurisdiction: its data is stored and processed in the EU.
- `wrangler dev`, the local migrations and the local queries share one local state, `.wrangler/state/`. The local
  database has no tables until `make migrate-local` runs.
- `make migrate-local`, `make migrate-remote`, `make migrate-list`, `make db-local SQL="..."` and
  `make db-remote SQL="..."` wrap the `package.json` scripts of the same names (`migrate:local`, …).
- Create a migration with `npx wrangler d1 migrations create pilotariak-pik "<description>"`, then add the `-- SPDX`
  header from `hack/header.txt` at the top of the file.
- Never edit a migration that has been applied remotely: add a new one. Never paste remote rows into an issue, a pull
  request or a log: they are private data.

## Testing

```bash
make check           # cargo fmt --check, cargo clippy on wasm32-unknown-unknown
make test            # worker-build, cargo test, then the Worker tests in Miniflare
make lint            # prek run -a: license headers, formatting, YAML/TOML/JSON, secrets
```

- Pure Rust logic (calendar, states) gets unit tests in its module, run by `cargo test` on the host.
- Behavior that needs the runtime (routes, D1, Cron, Rate Limiting) gets a test in `tests/`, which loads `build/` in
  Miniflare, as the workers-rs README recommends. Start the Worker with `startWorker()` from `tests/worker.mjs`, and
  declare a new binding there, so that every test file runs the same Worker.

CI (`.github/workflows/ci.yml`) runs all of this on every pull request, and applies the migrations to a local D1.

## Deployment

```bash
make deploy          # make migrate-remote, then npm run deploy (wrangler deploy)
```

`wrangler deploy` attaches the Custom Domain `pik.pilotariak.com` declared in `routes`, then
`https://pik.pilotariak.com/api/v1/health` answers.

## Platform features the next issues use

All are in workers-rs 0.9.0:

- **D1**: the `d1` feature of the `worker` crate, `env.d1("DB")`, prepared statements with `bind` and `batch`.
- **Cron Triggers**: a `#[event(scheduled)]` handler receives a `ScheduledEvent`, and `triggers.crons` goes in
  `wrangler.jsonc`.
- **Rate Limiting binding**: `env.rate_limiter("NAME")?.limit(key)` returns `success`. The binding goes in `ratelimits`
  in `wrangler.jsonc`.
- **Cookies**: workers-rs has no cookie helper. Read the `Cookie` request header, and add each `Set-Cookie` with
  `headers.append`, never `set`, so that one cookie does not replace another.

## Key Conventions

- Keep versions in step: the `worker` crate and `worker-build` (Cargo.toml, `wrangler.jsonc`, Makefile, CI); `wrangler`
  and `miniflare` at the versions wrangler bundles; `compatibility_date` no later than the workerd that wrangler
  bundles.
- Do not bump `version` in `Cargo.toml` by hand: release-please does it from conventional commits.
- `Cargo.toml` forbids `unsafe` code and turns on `clippy::pedantic`, and `make check` fails on any warning. Silence a
  lint with `#[expect(lint, reason = "...")]`, never `#[allow]`. In tests, import items by name (no `use super::*`) and
  give every assertion a message.
- Commits follow [Conventional Commits](https://www.conventionalcommits.org) and carry a DCO `Signed-off-by`
  (`git commit -s`, see CONTRIBUTING.md).
- License headers are required on every source and config file (checked by `licenserc.toml`): `//` for Rust, JSONC and
  JavaScript, `#` for TOML, YAML and the Makefile, `--` for SQL, `<!-- -->` after the doctype for HTML.
