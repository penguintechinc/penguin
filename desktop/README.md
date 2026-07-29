# Penguin Desktop (Phase 2b: Tauri Shell)

The Tauri shell wraps `penguin-desktop-core`, providing a native desktop application window and command bridge between the SPA frontend and the authenticated IPC layer.

## Architecture

```
┌──────────────────────────────────────────┐
│        Tauri Window (Webkit2gtk)         │
│  ┌──────────────────────────────────────┐│
│  │  React SPA (Phase 3 frontend dist)   ││
│  │  Calls Tauri commands (no tokens)    ││
│  └──────────────────────────────────────┘│
└───────────────┬──────────────────────────┘
                │ invoke()
         ┌──────▼──────────┐
         │ Tauri Commands  │  ← Rust layer (this crate)
         │ (auth, api_req) │
         └──────┬──────────┘
                │
      ┌─────────▼──────────────┐
      │ penguin-desktop-core   │ ← Session manager
      │ - Token store (keychain)
      │ - IPC to penguind      │
      │ - OAuth flows          │
      └─────────┬──────────────┘
                │ UDS
      ┌─────────▼──────────────┐
      │      penguind          │ ← Hub IPC daemon
      │   (penguin.desktop.v1) │
      └────────────────────────┘
```

## Directory Structure

```
desktop/
├── src-tauri/                    # Rust Tauri app
│   ├── Cargo.toml
│   ├── tauri.conf.json          # THE Tauri config Tauri actually reads (deep-link
│   │                            # scheme, CSP, bundler, frontendDist, beforeBuildCommand)
│   ├── src/
│   │   ├── main.rs              # Tauri app init, deep-link handler
│   │   ├── commands.rs          # Tauri command wrappers (login, api_request, oauth_*)
│   │   ├── error.rs             # DesktopError → String conversion
│   │   └── lib.rs               # Library exports
│   └── build.rs                 # Tauri build script (generated)
├── frontend/dist/                # gitignored — fetched by scripts/fetch-webui.sh
│                                  # (this is `frontendDist` in src-tauri/tauri.conf.json)
├── scripts/
│   └── fetch-webui.sh            # Downloads + sha256-verifies the published waddlebot
│                                  # webui dist (see webui.lock); dev/offline fallback
│                                  # copies index.html as a placeholder instead
├── webui.lock                    # Pins the exact webui version + sha256 to fetch
├── package.json                  # Node dependencies (React, Tauri CLI, Vite)
├── index.html                    # Placeholder frontend; also fetch-webui.sh's
│                                  # offline/no-release-yet fallback content
├── Dockerfile                   # Build environment with webkit2gtk, Rust, Node
└── README.md                    # This file
```

**Note:** there is deliberately no `tauri.conf.json` at the top level of
`desktop/` — Tauri resolves its config relative to the Rust crate
(`src-tauri/Cargo.toml`'s `build.rs` calls `tauri_build::build()`, which reads
`src-tauri/tauri.conf.json`), so that's the only copy that exists.

## Tauri Commands

All commands are async and communicate with `penguin-desktop-core::Session` behind a mutex.

### `login(email, password, hub_url) → {success, email}`

Authenticate with email and password. Internally:
1. Calls `Session::login()` which POSTs `/api/v1/auth/login` via penguind
2. Extracts JWT from response
3. Persists to OS keychain
4. Primes penguind's in-memory session

**Frontend never sees the token.**

### `logout() → ()`

Clear keychain and notify penguind. `Session::logout()` clears the stored session.

### `api_request(method, path, body?) → {status, body}`

Proxy an authenticated API request to the hub.

**Token injection is automatic** — penguind adds the Bearer header, handles 401 → refresh → retry.

### `oauth_start(platform, hub_url) → {authorize_url, state, platform}`

Start an OAuth flow. Internally:
1. Calls `Session::oauth_start()` which generates a CSPRNG state
2. Builds the hub's authorize URL
3. Opens it in the system browser via `tauri-plugin-opener`

Returns the state so the frontend can match it against the callback.

### `oauth_complete(code, state_param, stored_state) → {success, message}`

Complete the OAuth callback from a deep-link. Validates state and extracts tokens.

## Deep-Link Integration

The app registers the `waddles://` protocol (in `tauri.conf.json`). When the browser redirects to:

```
waddles://oauth/callback?code=...&state=...
```

The Tauri deep-link handler:
1. Parses the URL
2. Emits an `oauth-callback` event to the frontend with `{code, state}`
3. Frontend calls `oauth_complete()` with the callback parameters

## Frontend (Phase 3: Published WebUI Dist)

The desktop shell bundles the same React SPA published by
`penguintechinc/waddlebot`'s `publish-webui-dist` CI workflow, fetched and
verified by `scripts/fetch-webui.sh`:

1. `webui.lock` pins an exact webui `version` + `sha256`.
2. `scripts/fetch-webui.sh` runs automatically before every `tauri build`
   (wired as `beforeBuildCommand` in `src-tauri/tauri.conf.json`). It
   downloads `waddlebot-webui-<version>.tar.gz` + `.tar.gz.sha256` from the
   matching GitHub Release on `penguintechinc/waddlebot`, verifies the
   tarball's sha256 against the pin, and extracts it into `frontend/dist/`
   (gitignored — never committed).
3. `frontendDist` in `src-tauri/tauri.conf.json` points at `../frontend/dist`.

**Integrity gate:** a sha256 mismatch is a hard failure — the script refuses
to bundle a tampered or corrupted artifact, full stop.

**Dev/offline fallback:** if `webui.lock` still has the placeholder version
(no webui release published yet) or the download fails (offline, no
network), `fetch-webui.sh` instead copies the bundled placeholder
`index.html` into `frontend/dist/` and prints a WARNING. This keeps local
and CI builds working before the first webui release exists — it is
**not** the same thing as a checksum mismatch, which always hard-fails.

For local testing against a webui build you haven't published yet, point
the script at a local tarball instead of downloading:

```bash
WEBUI_LOCAL_TARBALL=/path/to/waddlebot-webui-0.0.0-test.tar.gz \
  bash scripts/fetch-webui.sh
```

(still verified against the `WEBUI_SHA256` pin in `webui.lock` — update the
lock to match your local tarball's sha256 first.)

## Building

### Local (requires Rust, Node, webkit2gtk)

```bash
cd desktop
npm install
cargo tauri build
```

`npm run build` (which `cargo tauri build` invokes under the hood) runs
`scripts/fetch-webui.sh` first via `beforeBuildCommand`, so the frontend
dist is fetched/verified automatically — no separate manual step.

### Docker (full build environment)

```bash
docker build -f Dockerfile -t penguin-desktop-builder .
docker run -it -v $(pwd):/app penguin-desktop-builder bash
# Inside container:
cd /app && npm install && cargo tauri build
```

The resulting binary is in `src-tauri/target/release/penguin-desktop` (or platform-specific bundle).

### CI/CD (recommended)

Full GUI builds are CI-only. Local `cargo check` of the src-tauri Rust side can verify compilation without webkit:

```bash
cd desktop/src-tauri
cargo check
```

## Workspace Separation

`desktop/` is excluded from the root Rust workspace (see `Cargo.toml` `exclude = ["desktop"]`). This keeps:

- The main workspace build (`cargo build --workspace`) clean
- Test coverage gate unaffected
- Desktop/Tauri/webkit dependencies isolated

The Tauri app has its own `Cargo.lock`, which must be committed.

## Token Handling Guarantee

- ✅ Tokens never cross into JavaScript (Rust layer holds them)
- ✅ Tokens persisted to OS keychain only (macOS Keychain, Linux Keyring, Windows Credential Manager)
- ✅ Token refresh handled transparently by penguind (401 → refresh → retry)
- ✅ Frontend receives sanitized responses only (no token, no PII)

## OAuth Flow Sequence

1. Frontend calls `oauth_start("google", hub_url)`
2. Tauri command opens browser to hub's authorize URL
3. User logs in to OAuth provider
4. OAuth provider redirects to `waddles://oauth/callback?code=...&state=...`
5. Deep-link handler parses URL, emits event to frontend
6. Frontend calls `oauth_complete(code, state_param, stored_state)`
7. Tauri command validates state, exchanges code for JWT (or receives it from hub)
8. Tokens stored in keychain, penduind primed
9. Frontend notified of success

## Known Placeholders

- `webui.lock` — pinned to a placeholder `WEBUI_VERSION` until the first
  `waddlebot-webui-<version>.tar.gz` is published; update it to the real
  version + sha256 once that release exists (see `scripts/fetch-webui.sh`)
- `index.html` — placeholder; used as the frontend until `webui.lock` is
  pinned to a real release, and as `fetch-webui.sh`'s offline/no-release
  fallback content even after that
- OAuth callback token exchange — currently expects the hub to return a JWT in the deep-link; verify actual hub callback format

## Architecture Notes

- The desktop core (`penguin-desktop-core`) is NOT part of the workspace lock — it's a path dependency within the excluded app
- Tauri v2.0.5 uses webkit2gtk on Linux; Cocoa on macOS; WinRT on Windows
- Deep-link scheme must be registered in `tauri.conf.json` and the system (handled by Tauri bundler)
- CSP is permissive by default to allow `unsafe-inline`; tighten per security audit

## See Also

- `crates/penguin-desktop-core` — Session API, IPC, keychain, OAuth
- `crates/penguin-secrets` — OS keychain backend (keyring on Linux, Keychain on macOS, Credential Manager on Windows)
