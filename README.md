# Mass Asset

A minimal on-chain asset standard for X1 (SVM).

**Version 1.0 — frozen.** The account layout, instructions, check order and error codes do not change. The official program is deployed **immutable** (no upgrade authority); any future fix or extension is published as a new program with a new program id (spec §9).

## What it is

- **Account.** Each asset is one account, at the PDA `["mass-asset", namespace, seed]`. It holds a 72-byte header and an issuer-defined payload of up to 1024 bytes.
- **Issuer.** Each asset belongs to one issuer program. Only the issuer, through its canonical authority PDA, can create the asset, write its payload and consume it.
- **Transfer** follows the asset's mode:
  - `owner` — the owner transfers;
  - `namespace` — the issuer transfers;
  - `frozen` — nobody can transfer.
- **Closing.** Consuming an asset hard-closes the account and returns all of its rent to the current owner. If the issuer is an upgradeable-loader program that has been closed, the owner can recover the rent with `orphan_close` (spec §4.4).
- **Errors** are `Custom(0x4D410000 + n)`.

The full rules are in the specification.

## Repository

| Path | Content |
|---|---|
| [`docs/spec.md`](docs/spec.md) | normative specification, with a rationale appendix |
| [`programs/mass-asset-v1/`](programs/mass-asset-v1/) | the program (Rust, `solana-program` 3.0) |
| [`tests/onchain/`](tests/onchain/) | on-chain conformance suite for X1 |
| [`docs/verify.md`](docs/verify.md) | how to verify the deployed program against this source |
| [`scripts/verifiable-build.sh`](scripts/verifiable-build.sh) | deterministic (Docker) build and hash |
| [`CHANGELOG.md`](CHANGELOG.md) | versions |

## Deployment

The official Mass Asset v1 program is deployed on X1 mainnet **without an upgrade authority** (immutable): it can never be changed or closed.

| | |
|---|---|
| Program id | `MAssPQXuBXvvf196oXKFZ1GiC9kTsJJ8podQV3m72Wv` |
| Network | X1 mainnet |
| Deployed in slot | 93180244 |
| Upgrade authority | none |
| Executable hash | `187aecc0a5604a0a3bac3bee5da7c5b253c331ea0ccc604a321116f7e914c626` |
| Program size | 47,480 bytes |
| Source | tag `v1.0.1` (`programs/` identical to the build commit `4114b30`) |
| Build image | `solanafoundation/solana-verifiable-build:4.0.0` |

Anyone can reproduce the build and compare hashes; see [docs/verify.md](docs/verify.md). The full on-chain conformance suite (`tests/onchain`, 53 checks) passed against this deployment.

Assets exist only under the program id they were created with. A future fix ships as a new program id; integrators should be ready to recognise more than one Mass Asset program id (spec §9).

## Build and test

```bash
cd programs/mass-asset-v1
cargo test        # host unit tests
cargo build-sbf   # SBF binary: target/deploy/mass_asset_v1.so
```

On-chain conformance suite: see [`tests/onchain/README.md`](tests/onchain/README.md).

## For issuers

Read spec §6 (issuer obligations) before integrating. If your assets represent value held elsewhere, also read §7.

In short:

- use the authority PDA only for Mass Asset;
- hard-code your authority bump;
- pay out and `consume` in the same instruction.
