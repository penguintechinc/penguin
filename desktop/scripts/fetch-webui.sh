#!/usr/bin/env bash
#
# fetch-webui.sh fetches the pinned WaddleBot webui `dist` release artifact
# (published by penguintechinc/waddlebot's publish-webui-dist workflow) and
# extracts it into desktop/frontend/dist — the Tauri `frontendDist` the
# desktop shell bundles as its React SPA.
#
# Contract: desktop/webui.lock pins an exact WEBUI_VERSION + WEBUI_SHA256.
# The downloaded tarball's sha256 MUST match the pin — a mismatch is a HARD
# FAILURE (the integrity gate), never silently ignored or overridden. A
# missing/placeholder pin, or a download failure (offline, no release
# published yet), instead falls back to the bundled placeholder frontend
# with a WARNING — dev/local builds must not hard-fail just because no
# webui has been published yet.
#
# Invoked automatically via `beforeBuildCommand` in
# desktop/src-tauri/tauri.conf.json (cwd = desktop/, where package.json and
# this script's parent `scripts/` dir live) and documented for manual/CI use
# in desktop/README.md.
#
# Overrides (local testing / dev only — see desktop/README.md):
#   WEBUI_LOCK_FILE        — path to the lock file (default: desktop/webui.lock)
#   WEBUI_DIST_DIR         — extraction target (default: desktop/frontend/dist)
#   WEBUI_LOCAL_TARBALL    — use this local tarball instead of downloading;
#                            still verified against WEBUI_SHA256 in the lock
#
# Bash 3.2 compatible (macOS ships 3.2): no associative arrays, no mapfile.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

LOCK_FILE="${WEBUI_LOCK_FILE:-${DESKTOP_DIR}/webui.lock}"
DIST_DIR="${WEBUI_DIST_DIR:-${DESKTOP_DIR}/frontend/dist}"
PLACEHOLDER_HTML="${DESKTOP_DIR}/index.html"

REPO="penguintechinc/waddlebot"
PLACEHOLDER_VERSION="UNPUBLISHED"

log()  { printf '[fetch-webui] %s\n' "$*"; }
warn() { printf '[fetch-webui] WARNING: %s\n' "$*" >&2; }
die()  { printf '[fetch-webui] ERROR: %s\n' "$*" >&2; exit 1; }

# sha256_of <file> — portable across GNU coreutils (sha256sum) and macOS
# (shasum -a 256); prints just the hex digest.
sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "neither sha256sum nor shasum is available to verify the webui tarball"
    fi
}

# use_placeholder <reason> — wipe DIST_DIR and replace it with the bundled
# Phase-2b placeholder index.html, so the Tauri build always has SOMETHING
# to bundle as frontendDist even when no real webui is available.
use_placeholder() {
    local reason="$1"
    warn "${reason}"
    warn "Using bundled placeholder frontend instead of the published webui."
    warn "The desktop app will show the Phase 2b placeholder screen, not the real WaddleBot UI."
    rm -rf "${DIST_DIR}"
    mkdir -p "${DIST_DIR}"
    cp "${PLACEHOLDER_HTML}" "${DIST_DIR}/index.html"
    log "Placeholder written to ${DIST_DIR}/index.html"
}

[[ -f "${LOCK_FILE}" ]] || die "lock file not found: ${LOCK_FILE}"

# shellcheck disable=SC1090
source "${LOCK_FILE}"

: "${WEBUI_VERSION:?WEBUI_VERSION not set in ${LOCK_FILE}}"
: "${WEBUI_SHA256:?WEBUI_SHA256 not set in ${LOCK_FILE}}"

if [[ "${WEBUI_VERSION}" == "${PLACEHOLDER_VERSION}" ]]; then
    use_placeholder "webui.lock is still pinned to the placeholder version (no webui release published yet)."
    exit 0
fi

TARBALL="waddlebot-webui-${WEBUI_VERSION}.tar.gz"
CHECKSUM_FILE="${TARBALL}.sha256"

WORKDIR="$(mktemp -d)"
trap 'rm -rf "${WORKDIR}"' EXIT

# --- Local-tarball override (dev/testing only) ---------------------------
if [[ -n "${WEBUI_LOCAL_TARBALL:-}" ]]; then
    [[ -f "${WEBUI_LOCAL_TARBALL}" ]] || die "WEBUI_LOCAL_TARBALL set but not found: ${WEBUI_LOCAL_TARBALL}"
    log "Using local tarball override: ${WEBUI_LOCAL_TARBALL}"
    cp "${WEBUI_LOCAL_TARBALL}" "${WORKDIR}/${TARBALL}"

    ACTUAL_SHA256="$(sha256_of "${WORKDIR}/${TARBALL}")"
    if [[ "${ACTUAL_SHA256}" != "${WEBUI_SHA256}" ]]; then
        die "sha256 MISMATCH for ${TARBALL}: expected (webui.lock) ${WEBUI_SHA256}, got ${ACTUAL_SHA256}. Refusing to use a tampered/corrupted artifact."
    fi

    rm -rf "${DIST_DIR}"
    mkdir -p "${DIST_DIR}"
    tar -xzf "${WORKDIR}/${TARBALL}" -C "${DIST_DIR}"
    log "Verified sha256 ${ACTUAL_SHA256} and extracted ${TARBALL} (local override) into ${DIST_DIR}"
    exit 0
fi

# --- Download from the waddlebot GitHub Release ---------------------------
if ! command -v gh >/dev/null 2>&1 && ! command -v curl >/dev/null 2>&1; then
    die "neither 'gh' nor 'curl' is available to download ${TARBALL}"
fi

download_asset() {
    local asset="$1" dest_dir="$2"
    if command -v gh >/dev/null 2>&1; then
        gh release download "${WEBUI_VERSION}" \
            --repo "${REPO}" \
            --pattern "${asset}" \
            --dir "${dest_dir}" \
            --clobber
    else
        local url="https://github.com/${REPO}/releases/download/${WEBUI_VERSION}/${asset}"
        curl --fail --silent --show-error --location --output "${dest_dir}/${asset}" "${url}"
    fi
}

log "Fetching ${TARBALL} (release ${WEBUI_VERSION} from ${REPO})..."

if ! download_asset "${TARBALL}" "${WORKDIR}" 2>"${WORKDIR}/download.err"; then
    warn "$(cat "${WORKDIR}/download.err" 2>/dev/null || true)"
    use_placeholder "Failed to download ${TARBALL} from ${REPO} release ${WEBUI_VERSION} (offline, or the release/asset does not exist yet)."
    exit 0
fi

if ! download_asset "${CHECKSUM_FILE}" "${WORKDIR}" 2>"${WORKDIR}/download.err"; then
    warn "$(cat "${WORKDIR}/download.err" 2>/dev/null || true)"
    use_placeholder "Failed to download ${CHECKSUM_FILE} from ${REPO} release ${WEBUI_VERSION}."
    exit 0
fi

ACTUAL_SHA256="$(sha256_of "${WORKDIR}/${TARBALL}")"
ASSET_SHA256="$(awk '{print $1}' "${WORKDIR}/${CHECKSUM_FILE}")"

# The lock file is the trusted pin; the downloaded .sha256 asset is a
# convenience cross-check, not a substitute for it. Either disagreeing is a
# hard failure — never fall back silently on a checksum mismatch.
if [[ "${ACTUAL_SHA256}" != "${WEBUI_SHA256}" ]]; then
    die "sha256 MISMATCH for ${TARBALL}: expected (webui.lock) ${WEBUI_SHA256}, got ${ACTUAL_SHA256}. Refusing to use a tampered/corrupted artifact."
fi

if [[ "${ACTUAL_SHA256}" != "${ASSET_SHA256}" ]]; then
    die "sha256 MISMATCH for ${TARBALL}: downloaded checksum asset (${ASSET_SHA256}) disagrees with the actual tarball (${ACTUAL_SHA256})."
fi

rm -rf "${DIST_DIR}"
mkdir -p "${DIST_DIR}"
tar -xzf "${WORKDIR}/${TARBALL}" -C "${DIST_DIR}"

log "Verified sha256 ${ACTUAL_SHA256} and extracted ${TARBALL} into ${DIST_DIR}"
