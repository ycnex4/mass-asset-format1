# On-chain conformance suite

This suite checks a real deployment of `programs/mass-asset-v1` on X1 against [`docs/spec.md`](../../docs/spec.md).

## What it does

1. Runs the host unit tests and builds the program plus a disposable test issuer.
2. Deploys Mass Asset and two test issuers under fresh program ids. The two issuers have different authority bumps.
3. Runs three groups of checks, listed below.
4. Closes every deployed program, so the deploy rent comes back.

## Checks

| Group | Checks |
|---|---|
| **Positive** (real transactions) | P1–P9: create on an empty address and on a pre-funded address, `write_payload`, transfer in `owner` and `namespace` modes, frozen asset, `consume` (exact rent refund to the current owner) |
| | P10–P11: `orphan_close` after the issuer is closed, including after a third party re-funds the dead ProgramData address |
| **Negative** (simulated) | N1–N36: simulated rejection paths, each of which must return the specified error. Header checks V1–V7 and authority checks A1–A2 are covered by the host unit tests (`cargo test`). Not exercised on-chain: `MissingSigner` (C9), `InvalidSystemProgram` (C11), `AccountNotWritable` for C10/T2/W2/O2 (K2 is covered by N25), and `ArithmeticOverflow` (unreachable) |
| **Uniform cost** (U1–U4) | U1: issuer B's asset stores its own authority bump. U2–U4: Mass Asset's own CU for `write_payload`, `transfer` (namespace mode) and `consume` must be identical for the two issuers |

## Requirements

- Solana CLI with `cargo build-sbf`;
- Node.js 18+;
- a funded keypair. About 0.65 XNT is held temporarily during the run; the net cost is about 0.015 XNT.

## Run

```bash
cd tests/onchain
npm install
WALLET=/path/to/keypair.json ./run.sh
```

Optional: `X1_RPC=...` (default `https://rpc.mainnet.x1.xyz`).

The full log is written to `tests/onchain/onchain.log`.

The test issuer (`test-issuer/`) is a signing proxy built for tests only. It violates spec §6 (E1) on purpose; never use it as an issuer template.
