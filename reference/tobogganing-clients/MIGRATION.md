# tobogganing clients — migration reference

Reference copy of the end-user client code migrated out of `tobogganing` as part
of consolidating all server + laptop endpoint agents into the unified `penguind`.

- **Source repo:** `penguintechinc/tobogganing` (`release/v1.2.X` @ `ff34486`)
- **Copied:** 2026-09-04 — tracked files only (build artifacts excluded), byte-for-byte verified.
- **Contents:** `docker/` (containerized client), `native/` (Go native client), `mobile/` (Flutter).
- These are retired from tobogganing; the server-side node-agent (Rust) migration into penguind is tracked separately.
