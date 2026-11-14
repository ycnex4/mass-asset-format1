#!/usr/bin/env bash
# Mass Asset v1 — official IMMUTABLE deployment.
#
# Deploys the verifiable-build binary with --final (no upgrade authority, ever),
# then checks on-chain that:
#   - the program has no upgrade authority;
#   - the on-chain executable hash equals the verifiable-build hash.
# Writes build/deployment.txt with everything needed for the README.
#
# THIS IS IRREVERSIBLE: the program can never be upgraded or closed, and its rent
# (~0.33 XNT) is locked forever.
#
# Environment:
#   WALLET           payer keypair (default: ~/.config/solana/id.json)
#   PROGRAM_KEYPAIR  keypair that defines the program id (required; back it up first)
#   X1_RPC           RPC endpoint (default: https://rpc.mainnet.x1.xyz)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SO="$ROOT/programs/mass-asset-v1/target/deploy/mass_asset_v1.so"
BUILD_RECORD="$ROOT/build/mass_asset_v1.sha256"
DEPLOY_RECORD="$ROOT/build/deployment.txt"
RPC="${X1_RPC:-https://rpc.mainnet.x1.xyz}"
WALLET="${WALLET:-$HOME/.config/solana/id.json}"
: "${PROGRAM_KEYPAIR:?set PROGRAM_KEYPAIR=/path/to/program-keypair.json (back it up first)}"

[[ -f "$WALLET" ]] || { echo "ERROR: wallet not found: $WALLET" >&2; exit 1; }
[[ -f "$PROGRAM_KEYPAIR" ]] || { echo "ERROR: program keypair not found: $PROGRAM_KEYPAIR" >&2; exit 1; }
[[ -f "$SO" && -f "$BUILD_RECORD" ]] || { echo "ERROR: run scripts/verifiable-build.sh first" >&2; exit 1; }
command -v solana-verify >/dev/null || { echo "ERROR: solana-verify not found" >&2; exit 1; }

BUILD_HASH="$(awk '/^executable_hash:/{print $2}' "$BUILD_RECORD")"
BUILD_COMMIT="$(awk '/^commit:/{print $2}' "$BUILD_RECORD")"
BUILD_IMAGE="$(awk -F': +' '/^base_image:/{print $2}' "$BUILD_RECORD")"
LOCAL_HASH="$(solana-verify get-executable-hash "$SO")"
if [[ "$LOCAL_HASH" != "$BUILD_HASH" ]]; then
  echo "ERROR: $SO hash $LOCAL_HASH != recorded verifiable-build hash $BUILD_HASH" >&2
  echo "       (was the binary rebuilt with cargo build-sbf after the verifiable build?)" >&2
  exit 1
fi

PROGRAM_ID="$(solana-keygen pubkey "$PROGRAM_KEYPAIR")"
WALLET_PUBKEY="$(solana-keygen pubkey "$WALLET")"
if solana --url "$RPC" account "$PROGRAM_ID" >/dev/null 2>&1; then
  echo "ERROR: account $PROGRAM_ID already exists on-chain; use a fresh program keypair." >&2
  exit 1
fi
RENT="$(solana --url "$RPC" rent "$(( $(stat -c%s "$SO") + 45 ))" --lamports | awk '/Rent-exempt minimum/{print $3}')"
BAL="$(solana --url "$RPC" balance "$WALLET_PUBKEY" --lamports | awk '{print $1}')"

cat <<INFO

=== MASS ASSET v1 — IMMUTABLE DEPLOYMENT ===
RPC:              $RPC
Program id:       $PROGRAM_ID
Binary:           $SO
Executable hash:  $BUILD_HASH   (matches verifiable build)
Built from:       $BUILD_COMMIT  ($BUILD_IMAGE)
Payer:            $WALLET_PUBKEY  balance $BAL lamports
Rent locked:      ~$RENT lamports (forever)

The program will be deployed with --final: NO upgrade authority, cannot be changed or closed.
INFO
read -r -p "Type DEPLOY to continue: " ANSWER
[[ "$ANSWER" == "DEPLOY" ]] || { echo "Aborted."; exit 1; }

solana --url "$RPC" --keypair "$WALLET" program deploy "$SO" --program-id "$PROGRAM_KEYPAIR" --final

echo
echo "=== Post-deployment checks ==="
SHOW=""
for i in $(seq 1 20); do
  SHOW="$(solana --url "$RPC" program show "$PROGRAM_ID" 2>/dev/null)" && break
  echo "  waiting for the RPC to see $PROGRAM_ID ($i/20)..."; sleep 3
done
[[ -n "$SHOW" ]] || { echo "ERROR: RPC still does not show $PROGRAM_ID; check manually: solana program show $PROGRAM_ID" >&2; exit 1; }
echo "$SHOW"
AUTH="$(echo "$SHOW" | awk -F': *' '/^Authority:/{print $2}')"
SLOT="$(echo "$SHOW" | awk -F': *' '/^Last Deployed In Slot:/{print $2}')"
ONCHAIN_HASH="$(solana-verify get-program-hash -u "$RPC" "$PROGRAM_ID" || echo unavailable)"

OK=1
[[ "$AUTH" == "none" ]] && echo "OK   upgrade authority: none (immutable)" || { echo "FAIL upgrade authority: $AUTH"; OK=0; }
[[ "$ONCHAIN_HASH" == "$BUILD_HASH" ]] && echo "OK   on-chain hash == verifiable-build hash" || { echo "FAIL on-chain hash $ONCHAIN_HASH != $BUILD_HASH"; OK=0; }

{
  echo "program_id:       $PROGRAM_ID"
  echo "network:          X1 mainnet ($RPC)"
  echo "deployed_slot:    $SLOT"
  echo "upgrade_authority: $AUTH"
  echo "executable_hash:  $ONCHAIN_HASH"
  echo "source_commit:    $BUILD_COMMIT"
  echo "build_image:      $BUILD_IMAGE"
  echo "deployed_at_utc:  $(date -u +%Y-%m-%dT%H:%M:%SZ)"
} | tee "$DEPLOY_RECORD"

(( OK )) || { echo "One or more post-deployment checks FAILED — do not publish; investigate." >&2; exit 1; }
echo
echo "Next: run the conformance suite against the official program:"
echo "  MASS_PROGRAM_ID=$PROGRAM_ID WALLET=... tests/onchain/run.sh"
