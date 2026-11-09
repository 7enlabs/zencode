// Unit tests for the user counter worker, with an in-memory D1 stub
// that interprets exactly the SQL statements worker.js issues.
// Run: node --test workers/user-counter/test/
import { test } from "node:test";
import assert from "node:assert/strict";
import worker from "../worker.js";

const HOUR = 60 * 60 * 1000;
const T0 = Date.UTC(2026, 10, 9, 8, 0, 0); // 2026-11-09T08:00:00Z
const ID_A = "11111111-2222-4333-8444-555555555555";
const ID_B = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

class FakeD1 {
  constructor() {
    this.rows = new Map(); // key: `${hash}|${day}`
  }
  prepare(sql) {
    const rows = this.rows;
    let bound = [];
    const stmt = {
      bind(...args) {
        bound = args;
        return stmt;
      },
      async run() {
        if (sql.startsWith("INSERT INTO pings")) {
          const [hash, day, version, os, arch, last_seen] = bound;
          rows.set(`${hash}|${day}`, {
            install_hash: hash,
            day,
            version,
            os,
            arch,
            last_seen,
          });
          return { success: true };
        }
        throw new Error(`FakeD1: unexpected run SQL: ${sql}`);
      },
      async first() {
        if (sql.includes("SELECT last_seen FROM pings")) {
          const [hash] = bound;
          let best = null;
          for (const r of rows.values()) {
            if (
              r.install_hash === hash &&
              (best === null || r.last_seen > best.last_seen)
            ) {
              best = r;
            }
          }
          return best ? { last_seen: best.last_seen } : null;
        }
        if (sql.includes("COUNT(DISTINCT install_hash)")) {
          let matched = [...rows.values()];
          if (sql.includes("WHERE day = ?")) {
            matched = matched.filter((r) => r.day === bound[0]);
          } else if (sql.includes("WHERE day >= ?")) {
            matched = matched.filter((r) => r.day >= bound[0]);
          }
          return { n: new Set(matched.map((r) => r.install_hash)).size };
        }
        throw new Error(`FakeD1: unexpected first SQL: ${sql}`);
      },
      async all() {
        if (sql.includes("GROUP BY version")) {
          const byVersion = new Map();
          for (const r of rows.values()) {
            if (r.day < bound[0]) continue;
            if (!byVersion.has(r.version)) byVersion.set(r.version, new Set());
            byVersion.get(r.version).add(r.install_hash);
          }
          return {
            results: [...byVersion.entries()].map(([version, set]) => ({
              version,
              n: set.size,
            })),
          };
        }
        throw new Error(`FakeD1: unexpected all SQL: ${sql}`);
      },
    };
    return stmt;
  }
}

function env() {
  return { DB: new FakeD1() };
}

function ping(id, extra = {}) {
  return new Request("https://count.runzen.dev/ping", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      install_id: id,
      version: "0.1.0",
      os: "linux",
      arch: "x86_64",
      ...extra,
    }),
  });
}

async function stats(e, now) {
  const res = await worker.fetch(
    new Request("https://count.runzen.dev/stats"),
    e,
    now,
  );
  assert.equal(res.status, 200);
  return res.json();
}

test("valid ping is stored hashed, never raw, with no IP field", async () => {
  const e = env();
  const res = await worker.fetch(ping(ID_A), e, T0);
  assert.equal(res.status, 204);
  assert.equal(e.DB.rows.size, 1);
  const [row] = e.DB.rows.values();
  assert.notEqual(row.install_hash, ID_A);
  assert.match(row.install_hash, /^[0-9a-f]{64}$/);
  assert.deepEqual(Object.keys(row).sort(), [
    "arch",
    "day",
    "install_hash",
    "last_seen",
    "os",
    "version",
  ]);
});

test("second ping inside 12 hours is accepted but not stored", async () => {
  const e = env();
  await worker.fetch(ping(ID_A), e, T0);
  const res = await worker.fetch(ping(ID_A), e, T0 + 6 * HOUR);
  assert.equal(res.status, 204);
  assert.equal(e.DB.rows.size, 1);
  const [row] = e.DB.rows.values();
  assert.equal(row.last_seen, T0);
});

test("ping on the next day after 12 hours adds a row", async () => {
  const e = env();
  await worker.fetch(ping(ID_A), e, T0);
  await worker.fetch(ping(ID_A), e, T0 + 26 * HOUR);
  assert.equal(e.DB.rows.size, 2);
});

test("malformed payloads are dropped", async () => {
  const e = env();
  for (const bad of [
    ping("not-a-uuid"),
    ping(ID_A, { version: "" }),
    ping(ID_A, { os: "x".repeat(64) }),
    ping(ID_A, { arch: 42 }),
  ]) {
    const res = await worker.fetch(bad, e, T0);
    assert.equal(res.status, 204);
  }
  assert.equal(e.DB.rows.size, 0);
});

test("invalid JSON body is a 400", async () => {
  const e = env();
  const req = new Request("https://count.runzen.dev/ping", {
    method: "POST",
    body: "{nope",
  });
  const res = await worker.fetch(req, e, T0);
  assert.equal(res.status, 400);
  assert.equal(e.DB.rows.size, 0);
});

test("stats aggregate installs, dau, wau, mau and versions", async () => {
  const e = env();
  await worker.fetch(ping(ID_A), e, T0);
  await worker.fetch(ping(ID_B, { version: "0.2.0" }), e, T0);
  await worker.fetch(ping(ID_A), e, T0 + 26 * HOUR);
  const s = await stats(e, T0 + 26 * HOUR);
  assert.equal(s.installs, 2);
  assert.equal(s.dau, 1);
  assert.equal(s.wau, 2);
  assert.equal(s.mau, 2);
  assert.deepEqual(s.versions, { "0.1.0": 1, "0.2.0": 1 });
  assert.ok(s.updated_at.endsWith("Z"));
});

test("stats on an empty database are zeros", async () => {
  const s = await stats(env(), T0);
  assert.equal(s.installs, 0);
  assert.equal(s.dau, 0);
  assert.deepEqual(s.versions, {});
});

test("wrong method is 405 and unknown path is 404", async () => {
  const e = env();
  const get = await worker.fetch(
    new Request("https://count.runzen.dev/ping"),
    e,
    T0,
  );
  assert.equal(get.status, 405);
  const miss = await worker.fetch(
    new Request("https://count.runzen.dev/nope"),
    e,
    T0,
  );
  assert.equal(miss.status, 404);
});

test("fetch with ExecutionContext as 3rd arg falls back to Date.now()", async () => {
  // Regression: in the Workers runtime the 3rd fetch argument is the
  // ExecutionContext object, not a timestamp. Passing it to dayOf used
  // to throw RangeError: Invalid time value (HTTP 500 / error 1101).
  const fakeCtx = { waitUntil() {}, passThroughOnException() {} };
  const e = env();
  const res = await worker.fetch(
    new Request("https://count.runzen.dev/stats"),
    e,
    fakeCtx,
  );
  assert.equal(res.status, 200);
  const s = await res.json();
  assert.equal(s.installs, 0);
  assert.equal(s.dau, 0);
});
