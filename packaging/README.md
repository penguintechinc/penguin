# Release & Packaging Pipeline (Rust)

M7.4: retargets the release/packaging pipeline from the frozen Go build
(`go-client/.goreleaser.yaml` + its three workflows) onto the Rust workspace.
There is no goreleaser step anymore — every stage below is a hand-written
GitHub Actions job, because goreleaser's own build model (one Go toolchain,
`GOOS`/`GOARCH` cross-compiles trivially) doesn't fit Rust's per-target
toolchain/build-script model as cleanly.

## The one hard contract: asset naming

`crates/penguin-update/src/platform.rs` (`asset_filename`) builds the exact
filename it will `GET` from the release when self-updating:

```
penguin_<version>_<os>_<arch>.<ext>
```

- `<version>` — no leading `v` (`platform::normalize_version` strips it; the
  workflow computes it the same way: `${GITHUB_REF_NAME#v}`)
- `<os>` — `linux`, `darwin`, or `windows` (note: **not** Rust's own
  `std::env::consts::OS` spelling, which says `"macos"` — this is the one
  mapping `platform.rs` exists to get right, see its module doc)
- `<arch>` — `amd64` or `arm64` (not Rust's `x86_64`/`aarch64` triple spelling)
- `<ext>` — `tar.gz` (Linux/macOS) or `zip` (Windows)

`.github/workflows/release.yml`'s `build` job's "Compute asset filename" step
builds this string with the identical template, driven by the same matrix
fields the archive/upload steps consume — see that job's header comment,
which quotes `platform.rs`'s `asset_filename` doc directly. Every archive
also gets a `<filename>.minisig` sibling (produced in the `finalize` job),
matching `release::select_asset`'s expectation exactly.

**If this drifts, self-update 404s silently — there is no other consumer of
this naming, so nothing else will catch a mismatch.**

## Pipeline layout

| Workflow | Produces | Runs on |
|---|---|---|
| `release.yml` | `penguind` + `pdcli` archives (6 targets), deb/rpm packages, SBOMs, minisign + cosign signatures | tag push `v*` |
| `release-tray.yml` | `penguin-tray` raw executables (5 targets — no windows/arm64), minisign-signed | tag push `v*` |
| `release-windows-msi.yml` | `penguind.msi` (WiX v5, windows/amd64 only) | tag push `v*` |

`release.yml` has three jobs: `version` (computes the stripped version once),
`build` (matrix: builds + archives per target), `packages` (nfpm deb/rpm from
the Linux archives), `finalize` (SBOMs, checksums, minisign, cosign, and the
actual `gh release upload`). Signing is centralized in `finalize` on a single
`ubuntu-latest` runner rather than duplicated per matrix leg.

## Deviations from the Go reference and from the milestone brief — read before changing the matrix

1. **No `cross` for `linux/arm64`, despite the brief suggesting it.**
   `cross` runs the *entire* `cargo build` — including `build.rs` — inside a
   Docker container, and the default `cross-rs` images do not ship `protoc`.
   `penguin-proto`'s `build.rs` shells out to a system `protoc`
   (`tonic_prost_build::configure().compile_protos(...)`, no vendored
   fallback), so a `cross` build would fail before producing anything, and
   fixing it would require a `Cross.toml` with a `pre-build` apt step — a new
   file outside this milestone's stated file scope (`.github/workflows/
   release*.yml`, `packaging/nfpm*.yaml`, `packaging/windows/*.wxs`, this
   README). Both `release.yml` and `release-tray.yml` instead build
   `linux/arm64` natively on GitHub's `ubuntu-24.04-arm` hosted runner — the
   exact runner `release-tray.yml` *already* used for the Go build's
   equivalent problem (cgo needs a native C toolchain). If a future PR wants
   `cross` back (e.g. to control glibc version skew for older target
   distros), it needs that `Cross.toml` added first.

2. **`macos-13` does not exist as a GitHub-hosted runner label anymore.**
   The Go reference (`release-tray.yml`) used it; `actionlint` (run against
   this PR) rejected it outright — GitHub's currently available Intel-macOS
   labels are `macos-15-intel` and `macos-26-intel`. Both `release.yml` and
   `release-tray.yml` use `macos-15-intel`. This was caught by linting, not
   foresight — worth a periodic re-check as GitHub continues retiring older
   macOS images.

3. **`windows/arm64` for `penguind`/`pdcli` is cross-linked, not skipped.**
   Unlike Go's tray build (which never shipped `windows/arm64` — "no stable
   native hosted runner yet"), `release.yml` builds `windows/arm64` by
   cross-linking from `windows-latest` (an amd64 host) using
   `rustup target add aarch64-pc-windows-msvc`. This works because, unlike
   `cross`'s Docker approach, plain `cargo build --target` still runs
   `build.rs`/proc-macros on the *host* — only the final link targets
   `aarch64-pc-windows-msvc` — so `protoc` on the amd64 host is unaffected,
   and there's no cgo-style native-toolchain requirement to fight. This was
   confirmed feasible (not run) by checking `actions/runner-images`'
   `Windows2022-Readme.md`: `windows-latest` ships
   `Microsoft.VisualStudio.Component.VC.Tools.ARM64`, the MSVC ARM64
   cross-linking toolset. **`release-tray.yml` does NOT do the same** for
   `penguin-tray` — its platform-tray-integration crate is still WIP
   (`bins/penguin-tray/Cargo.toml` had an empty `[dependencies]` table as of
   this PR), so cross-linking it isn't asserted safe.

4. **`penguin-tray` inclusion in the MSI: excluded, matching Go.**
   Go's `penguind.wxs` never included `penguin-tray.exe` (it has its own cgo
   build matrix and no Windows service story), and this port keeps that
   decision rather than changing it opportunistically. Flagged explicitly
   per the milestone brief's instruction — this is a decision, not an
   oversight. Revisit once tray has a real Windows startup/service model.

5. **`penguin-tray` gets minisign signatures Go never produced.** Not
   parity — a deliberate improvement, since the rest of this pipeline signs
   every artifact now and there's no reason tray should be the one silent
   exception. Tray's per-leg (not aggregated) signing means minisign gets
   installed once per OS matrix leg (apt on Linux, Homebrew on macOS, a
   pinned+checksummed direct download on Windows — see that workflow's
   comments) rather than once centrally like `release.yml`'s `finalize` job.

6. **Version source: the tag, not `.version` + `scripts/version/`.** The
   Rust workspace root has neither — version lives solely in
   `Cargo.toml`'s `[workspace.package] version`. This deviates from the
   org's usual `.version`-file convention (see `devops.md`). The release
   workflows derive the release version from the pushed git tag
   (`${GITHUB_REF_NAME#v}`) rather than reading `Cargo.toml`, since the tag
   is what actually names the GitHub Release and what the self-updater
   compares against (`GithubRelease::tag_name`) — `Cargo.toml`'s version is
   not read by any of these workflows and can drift from the tag with
   nothing to catch it. Tightening this (e.g., a CI check that the pushed
   tag matches `Cargo.toml`'s version before allowing the release job to
   run) is a reasonable follow-up, not implemented here.

7. **`nfpm` and `protoc` are pinned by a checksum this PR's author computed,
   not one nfpm/protobuf published.** Neither project publishes a
   `checksums.txt` asset that covers every archive this pipeline needs in
   one place at the exact versions pinned (nfpm ships one; protoc doesn't
   ship one at all for the release-asset zips) that could simply be copied
   in, so the SHA256 values embedded in the workflow YAML were computed by
   downloading each pinned asset directly from its GitHub Releases page over
   HTTPS during this PR's authoring and hashing it locally — the same trust
   model `pinning-dependency-digests` uses, done by hand. Re-verify (or
   re-pin to a newer version with a freshly computed hash) if either
   dependency's version pin is bumped later — do not just change the version
   string and leave the old hash in place.

## What is NOT verifiable without an actual tagged release

This milestone's gate is structural correctness, not a working release —
none of the following has been run, only reasoned through:

- **Cross-platform compilation of this workspace at all.** `ci.yml` only
  ever builds/tests on Linux (`rust:1.97-bookworm` container). This pipeline
  is the first thing that would attempt macOS and Windows builds of
  `penguind`/`pdcli`/`penguin-tray` — `rustls` + `aws-lc-rs`,
  `windows-service`, `keyring`'s per-OS backends, and
  `defguard_wireguard_rs`/`boringtun` all have OS-specific code paths that
  have never been compiled, let alone run, on those OSes.
- **The `windows/arm64` cross-link for `penguind`/`pdcli`** (deviation #3
  above) — the ARM64 MSVC toolset's presence on the runner image was
  confirmed from GitHub's own documentation, not by actually invoking it.
- **`nfpm` env-var expansion for `arch`/`version`/`maintainer`/etc.** —
  confirmed from nfpm's own documentation
  (`www/content/docs/configuration.md` in `goreleaser/nfpm`), not by running
  `nfpm package` against `packaging/nfpm.yaml`.
- **`anchore/sbom-action/download-syft`'s `cmd` output** actually being a
  directly-invokable path once installed in a workflow step.
- **minisign's non-interactive behavior** (`-S -s <key> -m <file> -x <sig>`
  with no password prompt) — requires the `MINISIGN_SECRET_KEY` secret to
  hold a key generated with `minisign -GW` (no password); if it was
  generated without `-W`, every `finalize`/tray signing step will hang
  waiting for a password prompt that never comes in CI.
- **The actual GitHub secrets** (`MINISIGN_SECRET_KEY`) existing and being
  correctly formatted (base64 of the raw `minisign.key` file contents) —
  not created or verified as part of this change.

What *was* verified: all four YAML/config files parse (`python3 -c
"yaml.safe_load(...)"`), `packaging/windows/penguind.wxs` is well-formed XML
(`xml.dom.minidom.parse`), and `actionlint` (run via
`docker run rhysd/actionlint`) passes clean on all three workflows and the
rest of `.github/workflows/` — including its runner-label and shellcheck
checks, which is how deviations #1 and #2 above were caught.
