#!/usr/bin/env bash
# Mass Asset v1 — verifiable (deterministic) build.
#
# Builds programs/mass-asset-v1 inside the solana-verify Docker image, so that anyone
# can rebuild the same commit and obtain the same executable hash.
#
# Output:
#   programs/mass-asset-v1/target/deploy/mass_asset_v1.so   (the binary to test and deploy)
#   build/mass_asset_v1.sha256                               (executable hash + build metadata)
#
# Requirements: Docker, `cargo install solana-verify`.
#
# BASE_IMAGE (optional): Docker image to build in, e.g.
#   BASE_IMAGE=solanafoundation/solana-verifiable-build:3.1.0 ./scripts/verifiable-build.sh
# By default solana-verify picks an image from the solana-program version in Cargo.lock.
# The image must ship a Solana toolchain new enough for the locked dependencies
# (edition 2024 requires cargo >= 1.85). The image used is recorded in the output.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROGRAM_DIR="$ROOT/programs/mass-asset-v1"
SO="$PROGRAM_DIR/target/deploy/mass_asset_v1.so"
OUT_DIR="$ROOT/build"
OUT="$OUT_DIR/mass_asset_v1.sha256"

command -v docker >/dev/null || { echo "ERROR: Docker is not installed or not on PATH." >&2; exit 1; }
docker info >/dev/null 2>&1 || { echo "ERROR: Docker daemon is not running (start Docker Desktop / dockerd)." >&2; exit 1; }
command -v solana-verify >/dev/null || { echo "ERROR: solana-verify not found. Install: cargo install solana-verify" >&2; exit 1; }

COMMIT="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
DIRTY="$(git -C "$ROOT" status --porcelain 2>/dev/null | grep -v '^?? build/' || true)"
if [[ -n "$DIRTY" ]]; then
  echo "WARNING: working tree has uncommitted changes; the build will not correspond to $COMMIT:" >&2
  echo "$DIRTY" >&2
fi

echo "== Verifiable build of $COMMIT =="
BUILD_ARGS=(--library-name mass_asset_v1)
[[ -n "${BASE_IMAGE:-}" ]] && BUILD_ARGS+=(--base-image "$BASE_IMAGE")
(cd "$PROGRAM_DIR" && solana-verify build "${BUILD_ARGS[@]}")

# The Docker build may leave target/ owned by root; give it back to the current user.
if [[ ! -w "$PROGRAM_DIR/target" ]]; then
  echo "target/ is not writable (owned by Docker/root); fixing ownership with sudo..."
  sudo chown -R "$(id -u):$(id -g)" "$PROGRAM_DIR/target"
fi
HASH="$(solana-verify get-executable-hash "$SO")"
mkdir -p "$OUT_DIR"
{
  echo "program:          mass_asset_v1"
  echo "commit:           $COMMIT"
  echo "executable_hash:  $HASH"
  echo "size_bytes:       $(stat -c%s "$SO")"
  echo "solana_verify:    $(solana-verify --version 2>/dev/null || echo unknown)"
  echo "base_image:       ${BASE_IMAGE:-auto (selected by solana-verify from Cargo.lock)}"
  echo "built_at_utc:     $(date -u +%Y-%m-%dT%H:%M:%SZ)"
} | tee "$OUT"

echo
echo "Binary: $SO"
echo "Note: executable_hash (solana-verify) ignores trailing padding, so it differs from a plain sha256sum of the file."
echo "Next: run the on-chain suite against THIS binary:"
echo "  MASS_SO=$SO WALLET=... tests/onchain/run.sh"
