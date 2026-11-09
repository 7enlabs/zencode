-- Anonymous user counter storage (Viec 7, Phase 1).
-- One row per install per day. install_hash is SHA-256(install_id);
-- the raw install_id and the client IP are never stored.
CREATE TABLE IF NOT EXISTS pings (
  install_hash TEXT NOT NULL,
  day TEXT NOT NULL,
  version TEXT NOT NULL,
  os TEXT NOT NULL,
  arch TEXT NOT NULL,
  last_seen INTEGER NOT NULL,
  PRIMARY KEY (install_hash, day)
);

CREATE INDEX IF NOT EXISTS idx_pings_day ON pings (day);
