[![License](https://img.shields.io/badge/License-Limited%20AGPL3-blue.svg)](LICENSE.md)

```
                                    _
    ____  ___  ____  ____ ___  __(_)___
   / __ \/ _ \/ __ \/ __ `/ / / / / __ \
  / /_/ /  __/ / / / /_/ / /_/ / / / / /
 / .___/\___/_/ /_/\__, /\__,_/_/_/ /_/
/_/               /____/     endpoint agent
```

# Penguin — Unified Endpoint Agent

One hardened desktop/endpoint agent for all PenguinTech products. Product
clients (Tobogganing SASE/ZTNA, Squawk DoH, and future products) are **modules**
on a shared secure core: auth, licensing/feature flags, secure storage, config,
metrics, tray, self-update, and packaging are written once.

## Components

| Binary | Role | Privileges |
|---|---|---|
| `penguind` | Daemon hosting all modules; owns every privileged operation (tunnels, port 53, resolver changes) | System service, least-privilege capabilities |
| `penguin` | CLI; talks to the daemon over authenticated local IPC | Unprivileged |
| `penguin-tray` | System tray with per-module status/actions | Unprivileged, user session |

## CLI

```bash
penguin modules                 # list modules and states
penguin load <module>           # enable + start a module (persists)
penguin unload <module>         # stop + disable a module
penguin status [module]         # agent or per-module status
penguin <module> <command>      # module commands, e.g.:
penguin tobogganing connect
penguin squawk query example.com
penguin logs [module]
penguin update                  # signed self-update
penguin version
```

## Module framework

Every product implements one interface (`pkg/sdk.Module`) — lifecycle,
status/health, a declarative CLI command tree, and a config schema. Modules are
either:

- **Compiled-in** — one registry line in `internal/registry`; or
- **External plugins** — separate binaries verified with pinned minisign
  publisher keys before launch (see `docs/APP_STANDARDS.md`).

Adding a new product client = implement the interface + one registry line.

## Development

```bash
make build        # binaries into ./bin
make lint         # golangci-lint
make test         # unit tests, race detector, 90% coverage gate
make smoke-test   # build + version smoke
make pre-commit   # full gate
```

See `docs/APP_STANDARDS.md` for architecture, security model, and the
dependency risk register. Company standards live in `docs/standards/`.

## Support

- support@penguintech.io · https://www.penguintech.io
- License: Limited AGPL-3.0 — see [LICENSE.md](LICENSE.md)
