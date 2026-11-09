// Anonymous user counter backend (Viec 7, Phase 1).
// Design: docs/design/user-counter.md (OPT-OUT, approved 2026-10-20).
//
// Privacy contract, enforced here:
// - Only SHA-256(install_id) is stored. The raw install_id is hashed
//   on arrival and never written anywhere.
// - The client IP is never read and never stored. There is no IP
//   column in the schema (schema.sql).
// - Malformed payloads are dropped. At most one accepted ping per
//   install per 12 hours.
// - GET /stats is public and returns aggregates only.

const UUID_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const RATE_LIMIT_MS = 12 * 60 * 60 * 1000;
const DAY_MS = 24 * 60 * 60 * 1000;

async function sha256Hex(text) {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(text),
  );
  return [...new Uint8Array(digest)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

function validField(v, max) {
  return typeof v === "string" && v.length > 0 && v.length <= max;
}

function dayOf(ms) {
  return new Date(ms).toISOString().slice(0, 10);
}

async function handlePing(request, env, now) {
  let body;
  try {
    body = await request.json();
  } catch {
    return new Response("bad request", { status: 400 });
  }
  const { install_id, version, os, arch } = body ?? {};
  if (
    typeof install_id !== "string" ||
    !UUID_RE.test(install_id) ||
    !validField(version, 32) ||
    !validField(os, 32) ||
    !validField(arch, 32)
  ) {
    // Malformed payloads are dropped, per design.
    return new Response(null, { status: 204 });
  }
  const hash = await sha256Hex(install_id);
  const recent = await env.DB.prepare(
    "SELECT last_seen FROM pings WHERE install_hash = ? ORDER BY last_seen DESC LIMIT 1",
  )
    .bind(hash)
    .first();
  if (recent && now - recent.last_seen < RATE_LIMIT_MS) {
    return new Response(null, { status: 204 });
  }
  await env.DB.prepare(
    "INSERT INTO pings (install_hash, day, version, os, arch, last_seen) VALUES (?, ?, ?, ?, ?, ?) " +
      "ON CONFLICT(install_hash, day) DO UPDATE SET version = excluded.version, os = excluded.os, arch = excluded.arch, last_seen = excluded.last_seen",
  )
    .bind(hash, dayOf(now), version, os, arch, now)
    .run();
  return new Response(null, { status: 204 });
}

async function handleStats(env, now) {
  const total = await env.DB.prepare(
    "SELECT COUNT(DISTINCT install_hash) AS n FROM pings",
  ).first();
  const dau = await env.DB.prepare(
    "SELECT COUNT(DISTINCT install_hash) AS n FROM pings WHERE day = ?",
  )
    .bind(dayOf(now))
    .first();
  const wau = await env.DB.prepare(
    "SELECT COUNT(DISTINCT install_hash) AS n FROM pings WHERE day >= ?",
  )
    .bind(dayOf(now - 6 * DAY_MS))
    .first();
  const mau = await env.DB.prepare(
    "SELECT COUNT(DISTINCT install_hash) AS n FROM pings WHERE day >= ?",
  )
    .bind(dayOf(now - 29 * DAY_MS))
    .first();
  const versionRows = await env.DB.prepare(
    "SELECT version, COUNT(DISTINCT install_hash) AS n FROM pings WHERE day >= ? GROUP BY version",
  )
    .bind(dayOf(now - 29 * DAY_MS))
    .all();
  const versions = {};
  for (const row of versionRows.results ?? []) {
    versions[row.version] = row.n;
  }
  return Response.json({
    installs: total?.n ?? 0,
    mau: mau?.n ?? 0,
    wau: wau?.n ?? 0,
    dau: dau?.n ?? 0,
    versions,
    updated_at: new Date(now).toISOString(),
  });
}

export default {
  // `now` is injectable for tests (a millisecond timestamp). In the
  // Workers runtime the 3rd fetch argument is ExecutionContext, not a
  // timestamp, so fall back to Date.now() unless a number was passed.
  async fetch(request, env, now) {
    if (typeof now !== "number") now = Date.now();
    const url = new URL(request.url);
    if (url.pathname === "/ping" && request.method === "POST") {
      return handlePing(request, env, now);
    }
    if (url.pathname === "/stats" && request.method === "GET") {
      return handleStats(env, now);
    }
    if (url.pathname === "/ping" || url.pathname === "/stats") {
      return new Response("method not allowed", { status: 405 });
    }
    return new Response("not found", { status: 404 });
  },
};
