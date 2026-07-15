# Example External Plugin: hello

This directory contains a trivial external plugin that demonstrates the SDK contract.

## What It Does

The `hello` plugin implements a single command `greet <name>` that returns "hello, <name>".

## Build and Sign

### 1. Build the Binary

```bash
go build -o hello ./examples/plugin-hello
```

### 2. Generate a Minisign Keypair (for testing)

```bash
minisign -G -p my_test.pub -s my_test.key
```

This creates:
- `my_test.pub` — your public key
- `my_test.key` — your private key (keep safe!)

### 3. Sign the Binary

```bash
minisign -S -s my_test.key -m hello
```

This creates `hello.minisig` containing the signature.

### 4. Create plugin.json

Create a `plugin.json` manifest in the plugin directory:

```json
{
  "name": "hello",
  "version": "1.0.0",
  "sdk_version": "v1",
  "binary": "hello",
  "sha256": "$(sha256sum hello | cut -d' ' -f1)",
  "publisher": "example"
}
```

Replace `$(sha256sum hello | cut -d' ' -f1)` with the actual SHA256 hash of your binary.

### 5. Organize for Deployment

A valid plugin directory looks like:

```
plugins/hello/
├── hello           (the compiled binary)
├── hello.minisig   (minisign signature)
└── plugin.json     (manifest)
```

## Testing

The daemon will verify:

1. **Ownership & Permissions** — Directory and binary must be owned by root or the daemon, not world-writable
2. **SHA256** — Binary hash must match manifest
3. **Signature** — Binary signature must verify against a pinned trusted key

All checks must pass before the plugin loads.

## Security Notes

- The plugin runs as a separate subprocess with AutoMTLS over gRPC
- The daemon serves a `HostService` so plugins can access logging, secrets, licensing, and events
- Plugins are terminated when the daemon stops
- Use NO_TOFU minisign verification: only keys in `/etc/penguin/trusted-publishers.d/*.pub` or the embedded PenguinTech key are trusted
