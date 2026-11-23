-- SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
-- SPDX-License-Identifier: Apache-2.0

-- Migration number: 0002 	 2026-11-23T12:59:33.680Z

-- Slots schema (pilotariak/pik#3).
--
-- Ids are random TEXT (128 bits, base32), never sequential: they appear in URLs. Local dates
-- are YYYY-MM-DD and local times HH:MM, in Europe/Paris. Instants are INTEGER milliseconds
-- since the epoch, UTC.
--
-- Sessions are not stored: their dates follow from the slot's recurrence
-- (src/domain/calendar/), and only per-date overrides are. D1 enforces foreign keys, so deleting
-- a slot deletes every row attached to it.

CREATE TABLE slots (
  id                   TEXT PRIMARY KEY,
  name                 TEXT NOT NULL,
  venue                TEXT NOT NULL,
  court                TEXT NOT NULL CHECK (court IN ('trinquet','mur_a_gauche','place_libre')),
  specialty            TEXT NOT NULL,            -- 'gomme_creuse', 'gomme_pleine'… or a Frontis specialty id
  interval_weeks       INTEGER NOT NULL CHECK (interval_weeks IN (1,2)),
  anchor_date          TEXT NOT NULL,            -- first session; also gives the weekday
  time_from            TEXT NOT NULL,
  time_to              TEXT,
  timezone             TEXT NOT NULL DEFAULT 'Europe/Paris',
  min_players          INTEGER NOT NULL DEFAULT 4,
  places               INTEGER NOT NULL DEFAULT 4,
  target               INTEGER NOT NULL DEFAULT 40 CHECK (target IN (30,35,40)),
  deadline_days_before INTEGER NOT NULL DEFAULT 1 CHECK (deadline_days_before BETWEEN 0 AND 6),
  deadline_time        TEXT NOT NULL DEFAULT '20:00',
  season_start         TEXT NOT NULL,
  season_end           TEXT NOT NULL,
  invite_code          TEXT NOT NULL UNIQUE,
  list_closed          INTEGER NOT NULL DEFAULT 0,
  created_at           INTEGER NOT NULL,
  CHECK (places >= min_players),
  CHECK (anchor_date BETWEEN season_start AND season_end)
);
CREATE INDEX slots_season_end ON slots(season_end);             -- purge (#5)

CREATE TABLE players (                                          -- one per device (ADR-0002)
  id           TEXT PRIMARY KEY,
  token_hash   BLOB NOT NULL UNIQUE,                            -- SHA-256, never the token
  created_at   INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL
);

CREATE TABLE members (
  id           TEXT PRIMARY KEY,
  slot_id      TEXT NOT NULL REFERENCES slots(id) ON DELETE CASCADE,
  player_id    TEXT REFERENCES players(id) ON DELETE SET NULL,  -- NULL: a name nobody has claimed (yet)
  display_name TEXT,                                            -- NULL once the member left: "Ancien joueur"
  name_key     TEXT,                                            -- normalized name (NFC, Unicode lowercase), set by the Worker
  role         TEXT NOT NULL DEFAULT 'player' CHECK (role IN ('organizer','player')),
  claimed_at   INTEGER,
  left_at      INTEGER,
  created_at   INTEGER NOT NULL,
  CHECK ((left_at IS NULL) = (display_name IS NOT NULL))
);
CREATE UNIQUE INDEX members_active_name ON members(slot_id, name_key) WHERE left_at IS NULL;
CREATE UNIQUE INDEX members_one_per_device ON members(slot_id, player_id) WHERE player_id IS NOT NULL;
CREATE INDEX members_by_player ON members(player_id);

CREATE TABLE session_overrides (                                -- replaces exceptions(kind)
  slot_id      TEXT NOT NULL REFERENCES slots(id) ON DELETE CASCADE,
  date         TEXT NOT NULL,
  cancelled_at INTEGER,                                         -- back to NULL: cancellation undone
  note         TEXT,
  min_players  INTEGER,                                         -- NULL: the slot's value
  places       INTEGER,
  deadline_at  INTEGER,
  PRIMARY KEY (slot_id, date)
) WITHOUT ROWID;

CREATE TABLE answers (
  slot_id     TEXT NOT NULL REFERENCES slots(id) ON DELETE CASCADE,
  date        TEXT NOT NULL,
  member_id   TEXT NOT NULL REFERENCES members(id) ON DELETE CASCADE,
  status      TEXT NOT NULL CHECK (status IN ('ok','dispo','ko')),
  answered_at INTEGER NOT NULL,                                 -- call order; changes only when the status changes
  set_by      TEXT REFERENCES members(id) ON DELETE SET NULL,   -- an organizer who answered for the member
  PRIMARY KEY (slot_id, date, member_id)
) WITHOUT ROWID;

CREATE TABLE guests (                                           -- a replacement for one session
  id           TEXT PRIMARY KEY,
  slot_id      TEXT NOT NULL REFERENCES slots(id) ON DELETE CASCADE,
  date         TEXT NOT NULL,
  display_name TEXT NOT NULL
);
CREATE INDEX guests_session ON guests(slot_id, date);

CREATE TABLE lineups (                                          -- header; its version is the ETag (#4)
  slot_id    TEXT NOT NULL REFERENCES slots(id) ON DELETE CASCADE,
  date       TEXT NOT NULL,
  version    INTEGER NOT NULL DEFAULT 1,
  updated_at INTEGER NOT NULL,
  updated_by TEXT REFERENCES members(id) ON DELETE SET NULL,
  PRIMARY KEY (slot_id, date)
) WITHOUT ROWID;

CREATE TABLE lineup_entries (
  slot_id   TEXT NOT NULL,
  date      TEXT NOT NULL,
  position  INTEGER NOT NULL,                                   -- rank in the selection
  member_id TEXT REFERENCES members(id) ON DELETE CASCADE,
  guest_id  TEXT REFERENCES guests(id) ON DELETE CASCADE,
  game      INTEGER,                                            -- NULL: selected, waits for the next game
  team      TEXT CHECK (team IN ('a','b')),
  PRIMARY KEY (slot_id, date, position),
  FOREIGN KEY (slot_id, date) REFERENCES lineups(slot_id, date) ON DELETE CASCADE,
  UNIQUE (slot_id, date, member_id),
  UNIQUE (slot_id, date, guest_id),
  CHECK ((member_id IS NULL) <> (guest_id IS NULL)),
  CHECK ((game IS NULL) = (team IS NULL))
);
CREATE INDEX lineup_entries_member ON lineup_entries(member_id); -- sessions played

CREATE TABLE recoveries (                                       -- single-use recovery links (#4)
  token_hash BLOB PRIMARY KEY,
  member_id  TEXT NOT NULL REFERENCES members(id) ON DELETE CASCADE,
  created_by TEXT REFERENCES members(id) ON DELETE SET NULL,
  expires_at INTEGER NOT NULL,                                  -- created + 24 h
  used_at    INTEGER
) WITHOUT ROWID;

CREATE TABLE idempotency_keys (                                 -- Idempotency-Key header (#4)
  key          TEXT PRIMARY KEY,                                -- UUID v4 drawn by the client
  route        TEXT NOT NULL,
  request_hash BLOB NOT NULL,                                   -- same key, other body: 422
  result_ref   TEXT NOT NULL,                                   -- id of the slot or link created, never the response
  created_at   INTEGER NOT NULL                                 -- purged after 24 h by the Cron (#5)
) WITHOUT ROWID;
