# Changelog

## 1.0.0 — 2026-11-14

First frozen version of the Mass Asset v1 standard:

- **Instructions:** `create`, `transfer`, `write_payload`, `consume`, `orphan_close`.
- **Header:** 72 bytes, with the canonical authority bump; the payload is 8-byte aligned.
- **Errors:** namespaced error codes (`0x4D41_0000 + n`).
