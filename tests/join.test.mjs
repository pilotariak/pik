// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

// Joining with an invite code: GET /join/:code, POST /join/:code/claim and
// POST /join/:code/members.

import assert from "node:assert/strict";
import { after, before, describe, test } from "node:test";
import { COOKIE, assertProblem, createSlot, startApi, world } from "./api.mjs";

let mf;
let call;

before(async () => {
  ({ mf, call } = await startApi());
});

after(async () => {
  await mf.dispose();
});

// A slot with two names added by the organizer that nobody holds yet.
async function slotWithFreeNames() {
  const w = await world(call);
  const ids = {};
  for (const name of ["Ana", "Bixente"]) {
    const res = await call("POST", `/slots/${w.slot}/members`, {
      token: w.organizer.token,
      body: { name },
    });
    assert.equal(res.status, 201, res.text);
    ids[name] = res.body.id;
  }
  return { ...w, free: ids };
}

describe("GET /join/:code", () => {
  test("previews the slot and its free names for any caller, token or not", async () => {
    const w = await slotWithFreeNames();
    for (const token of [undefined, w.organizer.token, w.player.token, w.outsider.token]) {
      const res = await call("GET", `/join/${w.code}`, { token });
      assert.equal(res.status, 200, res.text);
      assert.equal(res.body.name, "Mercredi");
      assert.equal(res.body.next_session, w.session);
      assert.deepEqual(
        res.body.free_names,
        [
          { member_id: w.free.Ana, name: "Ana" },
          { member_id: w.free.Bixente, name: "Bixente" },
        ],
        "claimed names are not listed",
      );
    }
  });

  test("the code is read in any case; an unknown code is a 404", async () => {
    const w = await world(call);
    assert.equal((await call("GET", `/join/${w.code.toLowerCase()}`)).status, 200);
    assertProblem(await call("GET", "/join/ABCDEFGH"), 404, "not-found");
    assertProblem(await call("GET", "/join/ABCD0000"), 404, "not-found");
  });
});

describe("POST /join/:code/claim", () => {
  test("without a token: a new player, its token in the body and the cookie", async () => {
    const w = await slotWithFreeNames();
    const res = await call("POST", `/join/${w.code}/claim`, { body: { member_id: w.free.Ana } });
    assert.equal(res.status, 200, res.text);
    assert.deepEqual(
      { slot: res.body.slot_id, member: res.body.member_id },
      { slot: w.slot, member: w.free.Ana },
    );
    assert.match(res.body.player_token, /^[a-z2-7]{52}$/);
    assert.ok(res.headers.get("set-cookie").startsWith(`${COOKIE}=${res.body.player_token};`));
    const slot = await call("GET", `/slots/${w.slot}`, { token: res.body.player_token });
    assert.equal(slot.body.me.member_id, w.free.Ana);
  });

  test("a member of another slot joins with the same player and no new token", async () => {
    const w = await slotWithFreeNames();
    const res = await call("POST", `/join/${w.code}/claim`, {
      token: w.outsider.token,
      body: { member_id: w.free.Ana },
    });
    assert.equal(res.status, 200, res.text);
    assert.equal(res.body.player_token, null);
    const me = await call("GET", "/me", { token: w.outsider.token });
    assert.deepEqual(me.body.slots.map((card) => card.id).sort(), [w.outsider.slot, w.slot].sort());
  });

  test("an organizer or a player of the slot is already a member: 409", async () => {
    const w = await slotWithFreeNames();
    for (const caller of [w.organizer, w.player]) {
      assertProblem(
        await call("POST", `/join/${w.code}/claim`, {
          token: caller.token,
          body: { member_id: w.free.Ana },
        }),
        409,
        "already-member",
      );
    }
  });

  test("a name already claimed is taken: 409", async () => {
    const w = await slotWithFreeNames();
    await call("POST", `/join/${w.code}/claim`, { body: { member_id: w.free.Ana } });
    assertProblem(
      await call("POST", `/join/${w.code}/claim`, { body: { member_id: w.free.Ana } }),
      409,
      "name-taken",
    );
    assertProblem(
      await call("POST", `/join/${w.code}/claim`, { body: { member_id: w.player.member } }),
      409,
      "name-taken",
    );
  });

  test("an unknown token counts as no token on a join route", async () => {
    const w = await slotWithFreeNames();
    const res = await call("POST", `/join/${w.code}/claim`, {
      token: "c".repeat(52),
      body: { member_id: w.free.Bixente },
    });
    assert.equal(res.status, 200, res.text);
    assert.match(res.body.player_token, /^[a-z2-7]{52}$/, "a new player");
  });

  test("a name of another slot or a wrong code: 422 and 404", async () => {
    const w = await slotWithFreeNames();
    assertProblem(
      await call("POST", `/join/${w.code}/claim`, { body: { member_id: w.outsider.member } }),
      422,
      "invalid-request",
    );
    assertProblem(
      await call("POST", "/join/ABCDEFGH/claim", { body: { member_id: w.free.Ana } }),
      404,
      "not-found",
    );
  });

  test("rate limit: 429 with Retry-After past 10 claims a minute", async () => {
    const w = await slotWithFreeNames();
    const ip = "198.51.100.7";
    for (let i = 0; i < 10; i += 1) {
      const res = await call("POST", `/join/${w.code}/claim`, {
        ip,
        body: { member_id: w.player.member },
      });
      assert.equal(res.status, 409, "the claims count even when they fail");
    }
    const limited = await call("POST", `/join/${w.code}/claim`, {
      ip,
      body: { member_id: w.free.Ana },
    });
    assertProblem(limited, 429, "rate-limited");
    assert.equal(limited.headers.get("retry-after"), "60");
  });
});

describe("POST /join/:code/members", () => {
  test("adds the device's name; an outsider keeps their player", async () => {
    const w = await world(call);
    const anonymous = await call("POST", `/join/${w.code}/members`, { body: { name: "Unai" } });
    assert.equal(anonymous.status, 201, anonymous.text);
    assert.match(anonymous.body.player_token, /^[a-z2-7]{52}$/);
    const outsider = await call("POST", `/join/${w.code}/members`, {
      token: w.outsider.token,
      body: { name: "Txomin" },
    });
    assert.equal(outsider.status, 201, outsider.text);
    assert.equal(outsider.body.player_token, null);
    const slot = await call("GET", `/slots/${w.slot}`, { token: w.organizer.token });
    assert.deepEqual(
      slot.body.members.map((member) => member.name),
      ["Maite", "Peio", "Unai", "Txomin"],
    );
  });

  test("an organizer or a player of the slot is already a member: 409", async () => {
    const w = await world(call);
    for (const caller of [w.organizer, w.player]) {
      assertProblem(
        await call("POST", `/join/${w.code}/members`, { token: caller.token, body: { name: "Other" } }),
        409,
        "already-member",
      );
    }
  });

  test("a name taken in any case: 409; an invalid name: 422", async () => {
    const w = await world(call);
    assertProblem(
      await call("POST", `/join/${w.code}/members`, { body: { name: "  PEIO " } }),
      409,
      "name-taken",
    );
    assertProblem(
      await call("POST", `/join/${w.code}/members`, { body: { name: "" } }),
      422,
      "invalid-request",
    );
  });

  test("a closed list: 409, but a free name can still be claimed", async () => {
    const w = await world(call);
    const added = await call("POST", `/slots/${w.slot}/members`, {
      token: w.organizer.token,
      body: { name: "Ana" },
    });
    await call("PUT", `/slots/${w.slot}/list`, { token: w.organizer.token, body: { closed: true } });
    assertProblem(
      await call("POST", `/join/${w.code}/members`, { body: { name: "Unai" } }),
      409,
      "list-closed",
    );
    const claimed = await call("POST", `/join/${w.code}/claim`, { body: { member_id: added.body.id } });
    assert.equal(claimed.status, 200, claimed.text);
  });

  test("the same device cannot join twice under two names", async () => {
    const { code } = await createSlot(call);
    const first = await call("POST", `/join/${code}/members`, { body: { name: "Unai" } });
    assertProblem(
      await call("POST", `/join/${code}/members`, {
        token: first.body.player_token,
        body: { name: "Unai bis" },
      }),
      409,
      "already-member",
    );
  });
});
