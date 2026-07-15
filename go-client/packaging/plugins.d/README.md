# External plugins directory

`penguind` loads external product modules from a root-owned plugins directory:

| OS | Path |
|----|------|
| Linux | `/usr/lib/penguin/plugins` |
| macOS | `/Library/Application Support/Penguin/plugins` |
| Windows | `%ProgramFiles%\Penguin\plugins` |

Each plugin lives in its own subdirectory:

```
<plugins-dir>/<name>/
  plugin.json        # manifest: name, version, sdkVersion, binary, sha256, publisher
  <binary>           # the plugin executable
  <binary>.minisig   # minisign signature over the binary
```

## Trust model (no TOFU)

The daemon verifies, in order, before ever launching a plugin:

1. The plugin directory and files are owned by root (or the daemon uid) and are
   **not world-writable** — a world-writable dir is refused outright.
2. `sha256(binary)` matches `plugin.json`.
3. The minisign signature verifies against a **pinned** publisher key. The
   PenguinTech key is compiled into `penguind`; additional publisher keys are
   read only from the root-owned `/etc/penguin/trusted-publishers.d/*.pub`.

There is no trust-on-first-use. An unsigned, tampered, or unknown-publisher
plugin is never executed.

## Building a plugin

Implement `sdk.Module` and call `sdk.Serve(myModule)` in `main`. See
`examples/plugin-hello/` for a complete reference plugin and the
build → sha256 → minisign → manifest flow.
