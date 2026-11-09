# User counter backend (Viec 7, Phase 1)

Cloudflare Worker + D1 implementing the server half of
`docs/design/user-counter.md`. The client half lives in the CLI
(`crates/zen-cli/src/telemetry.rs`).

Privacy contract:

- Only `SHA-256(install_id)` is stored. The raw install id is hashed
  on arrival and never written anywhere.
- The client IP is never read and never stored. The schema has no
  IP column.
- `POST /ping` accepts at most one ping per install per 12 hours and
  drops malformed payloads.
- `GET /stats` is public and returns aggregates only:
  `{installs, mau, wau, dau, versions, updated_at}`.

## Files

- `worker.js` - the fetch handler (no build step, no dependencies).
- `schema.sql` - D1 schema: one row per install per day.
- `wrangler.toml` - worker + D1 binding config.
- `test/worker.test.mjs` - unit tests with an in-memory D1 stub.

## Test

```sh
node --test workers/user-counter/test/worker.test.mjs
```

## Deploy

Deploy is a separate, approved step (it needs the Cloudflare account
that owns the runzen.dev zone):

```sh
cd workers/user-counter
npx wrangler d1 create zencode-user-counter   # put the id in wrangler.toml
npx wrangler d1 execute zencode-user-counter --file schema.sql
npx wrangler deploy                            # workers.dev URL first
```

Once DNS for `count.runzen.dev` exists, uncomment the `routes` block
in `wrangler.toml` and deploy again. Raw rows are kept 13 months per
the design; pruning is a scheduled `DELETE FROM pings WHERE day < ?`
run by the zone owner.
