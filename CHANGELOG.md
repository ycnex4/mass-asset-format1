# Changelog

## 1.0.2 — 2026-11-14

- Official immutable deployment: `MAssPQXuBXvvf196oXKFZ1GiC9kTsJJ8podQV3m72Wv` (slot 93180244, executable hash `187aecc0…c626`). README lists the deployment record.
- `scripts/deploy-final.sh`: waits for the RPC to see the new program before the post-deployment checks.
- Program code unchanged.

## 1.0.1 — 2026-11-14

Documentation only; no change to the program, layout, instructions or errors.

- Deployment policy: the official program is deployed immutable. Fixes and extensions are published as new programs with new program ids (spec §4.4, §9).
- Issuers should support more than one Mass Asset program id.

## 1.0.0 — 2026-11-14

First frozen version of the Mass Asset v1 standard:

- **Instructions:** `create`, `transfer`, `write_payload`, `consume`, `orphan_close`.
- **Header:** 72 bytes, with the canonical authority bump; the payload is 8-byte aligned.
- **Errors:** namespaced error codes (`0x4D41_0000 + n`).
