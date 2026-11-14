# Verifying the deployed program

The official Mass Asset v1 program is **immutable**. Anyone can check that the bytecode deployed on X1 was built from this repository.

## What is published

For the official deployment, the README lists:

- the program id;
- the git commit (tag) it was built from;
- the executable hash produced by `solana-verify`.

## How to verify

Requirements: Docker, Rust, and `cargo install solana-verify`.

```bash
git clone https://github.com/ycnex4/mass-asset.git
cd mass-asset
git checkout <published tag or commit>

# 1. Deterministic build inside the solana-verify Docker image
cd programs/mass-asset-v1
solana-verify build --library-name mass_asset_v1
solana-verify get-executable-hash target/deploy/mass_asset_v1.so

# 2. Hash of the program deployed on X1
solana-verify get-program-hash -u https://rpc.mainnet.x1.xyz <PROGRAM_ID>

# 3. Check that it is immutable: "Authority: none"
solana program show --url https://rpc.mainnet.x1.xyz <PROGRAM_ID>
```

The program is verified when:

- the two hashes from steps 1 and 2 are identical, and they match the hash in the README;
- step 3 shows no upgrade authority.

Notes:

- Only a build made with `solana-verify build` (Docker) reproduces the published hash. A local `cargo build-sbf` produces a different binary.
- `scripts/verifiable-build.sh` runs step 1 and records the hash together with the commit.
