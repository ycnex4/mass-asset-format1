#!/usr/bin/env bash
# Mass Asset v1 — on-chain conformance suite (spec: docs/spec.md).
#
# 1. Host unit tests, then SBF builds of Mass Asset and the disposable test issuer.
# 2. Deploys Mass Asset + test issuer A + test issuer B under FRESH program ids
#    (issuer B has a different authority bump than issuer A).
# 3. Phase "live" (issuer A): positive checks P1–P9, negative checks N1–N19, N21–N36 (simulated).
# 4. Phase "issuer-b": Mass Asset's own CU for authority-gated instructions must be
#    identical across issuers (U1–U4).
# 5. Closes issuer A, phase "orphan": orphan_close checks P10–P11, N20.
# 6. Closes issuer B and the Mass Asset deployment, so their rent comes back.
#
# Environment:
#   WALLET   keypair that pays for everything (default: ~/.config/solana/id.json)
#   X1_RPC   RPC endpoint (default: https://rpc.mainnet.x1.xyz)
#
# Net cost ≈ fees + 3 program-account rents (~0.0034 XNT, kept by the loader)
#          + one 0-byte shell (~0.0009 XNT) intentionally left at the dead issuer ProgramData.
#          Deploy rent (~0.6 XNT) is needed temporarily and returned at the end.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
COMMIT="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
HERE="$ROOT/tests/onchain"
MASS_DIR="$ROOT/programs/mass-asset-v1"
ISSUER_DIR="$HERE/test-issuer"
RPC="${X1_RPC:-https://rpc.mainnet.x1.xyz}"
WALLET="${WALLET:-$HOME/.config/solana/id.json}"
MASS_SO="$MASS_DIR/target/deploy/mass_asset_v1.so"
MASS_KP="$MASS_DIR/target/deploy/mass_asset_v1-keypair.json"
ISSUER_SO="$ISSUER_DIR/target/deploy/mass_asset_test_issuer.so"
ISSUER_KP="$ISSUER_DIR/target/deploy/mass_asset_test_issuer-keypair.json"
ISSUER_B_KP="$ISSUER_DIR/target/deploy/mass_asset_test_issuer_b-keypair.json"
STATE="$HERE/onchain.state.json"
LOG="$HERE/onchain.log"

cd "$ROOT"
exec > >(tee "$LOG") 2>&1

printf '\n=== MASS ASSET v1 — ON-CHAIN CONFORMANCE SUITE ===\n'
printf 'Repo:   %s\nCommit: %s\nRPC:    %s\nLog:    %s\n' "$ROOT" "$COMMIT" "$RPC" "$LOG"

[[ -f "$WALLET" ]] || { echo "ERROR: wallet keypair not found at $WALLET (set WALLET=...)" >&2; exit 1; }
[[ -d "$HERE/node_modules/@solana/web3.js" ]] || { echo "ERROR: run 'npm install' in tests/onchain first" >&2; exit 1; }
export PROBE_WALLET="$WALLET" X1_RPC="$RPC"
WALLET_PUBKEY="$(solana-keygen pubkey "$WALLET")"
balance() { solana --url "$RPC" balance "$WALLET_PUBKEY" --lamports | awk '{print $1}'; }
rent_for_so() { solana --url "$RPC" rent "$(( $(stat -c%s "$1") + 45 ))" --lamports | awk '/Rent-exempt minimum/{print $3}'; }
close_program() {
  solana --url "$RPC" --keypair "$WALLET" program close "$1" \
    --authority "$WALLET" --recipient "$WALLET_PUBKEY" --bypass-warning || true
}

echo '[0/8] Host unit tests (Mass Asset)...'
(cd "$MASS_DIR" && cargo test --quiet)
echo '[1/8] Building Mass Asset v1...'
(cd "$MASS_DIR" && cargo build-sbf)
echo '[2/8] Building test issuer...'
(cd "$ISSUER_DIR" && cargo build-sbf)
printf 'Mass Asset .so size: %s bytes\n' "$(stat -c%s "$MASS_SO")"

echo '[3/8] Fresh program keypairs...'
solana-keygen new --no-bip39-passphrase --silent --force -o "$MASS_KP" >/dev/null
solana-keygen new --no-bip39-passphrase --silent --force -o "$ISSUER_KP" >/dev/null
MASS_ID="$(solana-keygen pubkey "$MASS_KP")"
ISSUER_ID="$(solana-keygen pubkey "$ISSUER_KP")"
# Issuer B: a fresh keypair whose authority bump differs from issuer A's.
(cd "$HERE" && node -e '
const fs = require("fs");
const { Keypair, PublicKey } = require("@solana/web3.js");
const seed = [Buffer.from("mass-asset-authority")];
const bumpOf = (pk) => PublicKey.findProgramAddressSync(seed, pk)[1];
const a = bumpOf(new PublicKey(process.argv[1]));
let kp, b;
do { kp = Keypair.generate(); b = bumpOf(kp.publicKey); } while (b === a);
fs.writeFileSync(process.argv[2], JSON.stringify(Array.from(kp.secretKey)));
console.log(`authority bump: issuer A=${a} issuer B=${b}`);
' "$ISSUER_ID" "$ISSUER_B_KP")
ISSUER_B_ID="$(solana-keygen pubkey "$ISSUER_B_KP")"
printf 'Mass Asset v1:  %s\nTest issuer A:  %s\nTest issuer B:  %s\n' "$MASS_ID" "$ISSUER_ID" "$ISSUER_B_ID"

NEED=$(( $(rent_for_so "$MASS_SO") + 2 * $(rent_for_so "$ISSUER_SO") + 60000000 ))
BAL_START="$(balance)"
printf 'Wallet: %s\nBalance: %s lamports, needed (deploy rent + 0.06 XNT margin): %s lamports\n' "$WALLET_PUBKEY" "$BAL_START" "$NEED"
if (( BAL_START < NEED )); then
  echo "ERROR: top up the wallet by at least $(( NEED - BAL_START )) lamports. Deploy rent is returned at the end." >&2
  exit 1
fi

echo '[ENV] loader-v4 related feature gates on this cluster:'
solana --url "$RPC" feature status 2>/dev/null | grep -i -E 'loader|v4|migrat' || echo '  (none listed / feature status unavailable)'

echo '[4/8] Deploying Mass Asset, issuer A, issuer B...'
solana --url "$RPC" --keypair "$WALLET" program deploy "$MASS_SO" --program-id "$MASS_KP" --upgrade-authority "$WALLET"
if ! solana --url "$RPC" --keypair "$WALLET" program deploy "$ISSUER_SO" --program-id "$ISSUER_KP" --upgrade-authority "$WALLET"; then
  echo "ERROR: issuer A deploy failed; closing Mass Asset to return its rent." >&2
  close_program "$MASS_ID"; exit 1
fi
if ! solana --url "$RPC" --keypair "$WALLET" program deploy "$ISSUER_SO" --program-id "$ISSUER_B_KP" --upgrade-authority "$WALLET"; then
  echo "ERROR: issuer B deploy failed; closing the other programs to return their rent." >&2
  close_program "$ISSUER_ID"; close_program "$MASS_ID"; exit 1
fi

echo '[5/8] Phase LIVE...'
node "$HERE/onchain-suite.mjs" live "$MASS_ID" "$ISSUER_ID" "$STATE" || echo "WARNING: live phase exited with an error"

echo '[6/8] Phase ISSUER-B (constant authority cost)...'
if [[ -f "$STATE" ]]; then
  node "$HERE/onchain-suite.mjs" issuer-b "$MASS_ID" "$ISSUER_B_ID" "$STATE" || echo "WARNING: issuer-b phase exited with an error"
fi
close_program "$ISSUER_B_ID"

echo '[7/8] Closing test issuer A (namespace disappears), phase ORPHAN...'
close_program "$ISSUER_ID"
if [[ -f "$STATE" ]]; then
  node "$HERE/onchain-suite.mjs" orphan "$MASS_ID" "$ISSUER_ID" "$STATE" || echo "WARNING: orphan phase exited with an error"
else
  echo "WARNING: no state file from live phase; skipping orphan phase."
fi

echo '[8/8] Closing the Mass Asset deployment (returns its rent)...'
close_program "$MASS_ID"
rm -f "$STATE"

BAL_END="$(balance)"
printf '\nBalance start: %s\nBalance end:   %s\nNet cost:      %s lamports\n' "$BAL_START" "$BAL_END" "$(( BAL_START - BAL_END ))"
printf '\n=== ON-CHAIN SUITE COMPLETE ===\nSaved full log: %s\n' "$LOG"
