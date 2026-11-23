# How to manage the D1 database

pik keeps its data in one [Cloudflare D1](https://developers.cloudflare.com/d1/) database (SQLite at the edge). It is
managed like the Frontis databases: the commands call the database by its name, the scripts live in `package.json`, and
every local command shares one local state.

## Database

| Binding | Database name    | Jurisdiction |
| ------- | ---------------- | ------------ |
| `DB`    | `pilotariak-pik` | `eu`         |

The database holds private data of groups of friends: first names or nicknames, answers and lineups. Its EU
jurisdiction keeps that data stored and processed in the EU. It was created once with:

```bash
npx wrangler d1 create pilotariak-pik --jurisdiction eu
```

---

## Directory layout

```text
database/
└── migrations/
    ├── 0001_init.sql          ← empty: proves the migrations folder applies
    └── 0002_slots_schema.sql  ← slots, players, members, answers, guests, lineups…
wrangler.jsonc                 ← D1 binding DB, migrations_dir = database/migrations
package.json                   ← migration and query scripts
```

Unlike Frontis, `database/` has no `wrangler.toml` of its own: pik is a single Worker, and its `wrangler.jsonc` already
binds the database.

---

## Applying migrations

### Local (wrangler dev)

Apply pending migrations to the local SQLite file used by `wrangler dev`:

```bash
make migrate-local       # npm run migrate:local
```

This runs:

```bash
wrangler d1 migrations apply pilotariak-pik --local --persist-to .wrangler/state
```

Run it once after cloning, and after each new migration. Until then the local database lacks the tables of the
pending migrations (`0002_slots_schema.sql` creates the slots schema), and a query on them fails with
`no such table`.

### Remote (production)

Apply pending migrations to the live database:

```bash
make migrate-remote      # npm run migrate:remote
```

`make deploy` runs it before `wrangler deploy`, so the schema is in place before the code that uses it.

### Check migration status

```bash
make migrate-list                                         # local
npx wrangler d1 migrations list pilotariak-pik --remote   # remote
```

---

## Querying the local database

`wrangler dev`, the migrations and the queries share a single local state at the project root, `.wrangler/state/`.
Every script passes `--persist-to .wrangler/state`: it is wrangler's default for this repository, written out as in
Frontis. Data written by `make dev` is immediately visible to `make db-local`.

```bash
make db-local SQL="SELECT name FROM sqlite_master WHERE type = 'table'"
npm run db:local -- "SELECT name FROM sqlite_master WHERE type = 'table'"
```

For scripts, ask for JSON and use `jq`, or put a long query in a file:

```bash
npx wrangler d1 execute pilotariak-pik --local --persist-to .wrangler/state --json \
  --command "SELECT name FROM sqlite_master" | jq '.[0].results'

npx wrangler d1 execute pilotariak-pik --local --persist-to .wrangler/state --file query.sql
```

### Interactive shell with sqlite3

wrangler has no interactive SQL shell. The local database is a plain SQLite file under
`.wrangler/state/v3/d1/miniflare-D1DatabaseObject/`, named after a hash, not after the database. A file left by an
earlier `database_id` can remain next to it: compare with what `make db-local` returns. Open it read-only:

```bash
sqlite3 -readonly -box .wrangler/state/v3/d1/miniflare-D1DatabaseObject/<hash>.sqlite
```

Never write to this file directly, and avoid opening it while `make dev` runs: several processes on the same SQLite
file can fail with `database is locked`.

The Worker tests (`make test`) do not use this state: they run in Miniflare with an in-memory D1.

---

## Querying the remote database

The production database is queried the same way, with `--remote`. Every query runs on Cloudflare D1.

```bash
npx wrangler whoami      # logged in? Otherwise: npx wrangler login
make db-remote SQL="SELECT count(*) FROM d1_migrations"
npx wrangler d1 execute pilotariak-pik --remote --json --command "SELECT ..." | jq '.[0].results'
```

- The rows are private data. Read only what you need, and never paste them into an issue, a pull request or a log.
- A `DELETE` or `UPDATE` changes production at once. To undo a mistake, D1 Time Travel restores the database to a point
  of the last 30 days:
  `npx wrangler d1 time-travel restore pilotariak-pik --timestamp=<unix or RFC 3339>`.
- For an interactive console, open the Cloudflare dashboard: Storage & Databases → D1 → `pilotariak-pik` → Console.

---

## Creating a new migration

```bash
npx wrangler d1 migrations create pilotariak-pik "<description>"
```

This creates the next numbered file in `database/migrations/`. Add the `-- SPDX` header from `hack/header.txt` at the
top, write the SQL, then:

```bash
make migrate-local       # apply it locally
make test                # CI also applies every migration to a local D1
```

The remote database gets it with `make migrate-remote` or `make deploy`. Never edit a migration that has been applied
remotely: add a new one.
