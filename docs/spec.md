# Mass Asset v1 — Specification

**Version:** 1.0.1 (frozen; 1.0.1 changes only the deployment policy in §4.4 and §9)
**Date:** 2026-11-14
**Network:** X1 (SVM)

The key words **MUST**, **MUST NOT**, **SHOULD** and **MAY** are to be interpreted as described in RFC 2119.

---

## 1. Purpose and scope

Mass Asset v1 is a minimal on-chain asset standard for X1. Every asset:

- belongs to exactly one **issuer**: a program, called the *namespace*;
- has exactly one **owner**;
- carries a **payload**: bytes that only the issuer can write;
- is fully closed when it is consumed, and its rent is returned.

v1 does **not** define:

- N→M transitions;
- provenance;
- a persistent logical identity at the standard level;
- proof of historical issuance after an asset has been closed.

**Identity.** In v1 the identity of an asset is **the address of its account while that account is live**. If an issuer needs uniqueness over time, it MUST NOT reuse a `seed` (§3.1). The standard does not enforce this.

## 2. Roles and trust

| Role | Who | How it is established |
|---|---|---|
| Issuer (namespace) | a program | the `namespace` field of the asset account |
| Issuer authority | the canonical PDA `["mass-asset-authority"]` of the issuer program | a signature, which only the issuer can produce via `invoke_signed` |
| Owner | any `Pubkey` except `Pubkey::default()`, the asset's own address, the Mass Asset program, the issuer program and the issuer authority | the `owner` field; the owner's signature where required |
| Payer | any signer | pays rent at `create`; gains no rights over the asset |

### 2.1 Authority

- The authority seed is fixed forever: the byte string `mass-asset-authority`, with no suffix.
- At `create`, the authority is derived by the canonical search `find_program_address(["mass-asset-authority"], namespace)`. The bump found is written to the header as `authority_bump` (§3.2).
- For every operation on an existing asset, the authority MUST be derived only from `object.namespace` and `object.authority_bump`, using `create_program_address`. It MUST NOT be taken from instruction data or from a supplied account.
- `authority_bump` is written only by `create`. No instruction can change it afterwards.

### 2.2 Trust in the issuer

The issuer is fully trusted. `owner` mode does **not** protect the value of an asset from its issuer.

- In `owner` mode the issuer cannot call `transfer`.
- The issuer can always:
  - consume the asset (the rent goes to the current owner);
  - rewrite the payload;
  - create a new asset at the same address with a different owner.

An upgradeable issuer can change its own logic after issuance. Marketplaces and indexers MUST evaluate an asset together with the trust model of its issuer.

### 2.3 Owner requirements

To be consumable, an owner needs to be able to receive lamports as a writable, non-executable account.

- Mass Asset rejects the following as owner, at `create` and at `transfer`:
  - the asset's own address;
  - the Mass Asset program;
  - the issuer program;
  - the issuer authority. Otherwise the asset would be under the issuer's control despite `owner` mode.
- Any other unsuitable address (another program, a sysvar) is the responsibility of:
  - the issuer, at `create`;
  - the holder, at their own `transfer`.

On X1, consuming an asset whose owner is an executable account fails with `ExternalAccountLamportSpend`, so such an asset can never be consumed.

**Consumability is guaranteed only for owners that can receive lamports as a writable, non-executable account.**

### 2.4 Owner that is a PDA of another program

If the owner is a PDA (for example an escrow), only its program can sign for it. Every owner action, `orphan_close` included, is therefore possible only while that program is available and supports the call.

- Programs that hold Mass Assets MUST be able to invoke `orphan_close` and, for `owner`-mode assets, `transfer`.
- If the owning program is closed or cannot make the call, the asset and its rent are locked. This is the same property as an owner who has lost their key. The standard does not remove it.

## 3. Asset account

### 3.1 Address

```text
object = find_program_address(["mass-asset", namespace, seed], MASS_ASSET_PROGRAM_ID)
```

- `seed` is 32 bytes chosen by the issuer.
- Only the canonical bump is accepted. A caller-supplied bump is never accepted.
- The object bump and `seed` are not stored in the account: no instruction uses them after `create`. An issuer that needs them stores them in the payload.

### 3.2 Layout (little-endian), `HEADER_LEN = 72`

| Offset | Size | Field | Rule |
|---:|---:|---|---|
| 0 | 1 | `version` | `= 1` |
| 1 | 1 | `flags` | bits 0–1 = `transfer_mode`; bits 2–7 MUST be 0 |
| 2 | 2 | `payload_len` | `u16`, `0 ≤ payload_len ≤ 1024` |
| 4 | 32 | `namespace` | issuer program |
| 36 | 32 | `owner` | current owner |
| 68 | 1 | `authority_bump` | canonical authority bump; written only by `create` |
| 69 | 3 | `reserved` | MUST be 0 |
| 72 | `payload_len` | `payload` | issuer bytes; 8-byte aligned |

- The account size is `72 + payload_len`. It is fixed at `create` and never changes.
- `MAX_PAYLOAD = 1024`. Changing it, using `reserved`, or using `flags` bits 2–7 requires a new `version`.

`transfer_mode`:

| Value | Name | Who can transfer |
|---:|---|---|
| 0 | `owner` | only the current owner |
| 1 | `namespace` | only the issuer authority |
| 2 | `frozen` | nobody (non-transferable asset) |
| 3 | — | invalid |

**For indexers:** use `getProgramAccounts` with a memcmp filter at offset 4 (`namespace`) or offset 36 (`owner`).

### 3.3 `validate(object)` — checks, in exactly this order

| # | Check | Error |
|---|---|---|
| V1 | `object.owner == MASS_ASSET_PROGRAM_ID` | `NotMassAsset` |
| V2 | `data_len ≥ 72` | `InvalidAccountSize` |
| V3 | `version == 1` | `UnsupportedVersion` |
| V4 | `flags & 0xFC == 0` and `transfer_mode ≠ 3` | `InvalidFlags` |
| V5 | `reserved == [0, 0, 0]` | `InvalidHeader` |
| V6 | `payload_len ≤ 1024` and `data_len == 72 + payload_len` | `InvalidAccountSize` |
| V7 | `owner ≠ Pubkey::default()` | `InvalidOwner` |

### 3.4 `require_authority(signer, header)`

| # | Check | Error |
|---|---|---|
| A1 | `signer.is_signer` | `InvalidAuthority` |
| A2 | `create_program_address(["mass-asset-authority", [authority_bump]], namespace) == signer.key` | `InvalidAuthority` |

The cost does not depend on the issuer: it is one `create_program_address`.

## 4. Instructions

The first byte of instruction data is the tag. In the tables below, `data` means the bytes **after** the tag, and `len(data)` is their length.

Accounts are listed in a fixed order. Extra accounts are ignored.

**Order of execution:**

1. All data checks: rows C1–C5, T1, W1, K1, O1.
2. Account reads. If accounts are missing, the built-in error `NotEnoughAccountKeys` is returned at this step.
3. The remaining rows of the table, in order.

The "readonly" marks on accounts are for clients; the program does not check them.

### 4.0 `create` — tag `0`

Data:

```text
seed[32] | transfer_mode u8 | payload_len u16 | owner[32] | payload[payload_len]
```

Accounts:

| # | Account | Properties |
|---:|---|---|
| 0 | `authority` | signer |
| 1 | `namespace` | readonly |
| 2 | `payer` | signer, writable |
| 3 | `object` | writable |
| 4 | `system_program` | |

| # | Check | Error |
|---|---|---|
| C1 | `len(data) ≥ 67` | `InvalidInstruction` |
| C2 | `transfer_mode ≤ 2` | `InvalidFlags` |
| C3 | `payload_len ≤ 1024` | `InvalidAccountSize` |
| C4 | `len(payload) == payload_len` | `InvalidInstruction` |
| C5 | `owner ≠ default` | `InvalidOwner` |
| C6 | `authority`, `namespace`, `payer`, `object` are pairwise distinct | `DuplicateAccount` |
| C7 | `owner ∉ {object, MASS_ASSET_PROGRAM_ID, namespace, authority(namespace)}`. The authority is derived here by the canonical search and reused in C8 | `InvalidOwner` |
| C8 | `authority.is_signer` and `authority.key == find_program_address(["mass-asset-authority"], namespace).0`. This binds `namespace` to the signing program | `InvalidAuthority` |
| C9 | `payer.is_signer` | `MissingSigner` |
| C10 | `payer` and `object` are writable | `AccountNotWritable` |
| C11 | `system_program == 11111111111111111111111111111111` | `InvalidSystemProgram` |
| C12 | `object == find_program_address(["mass-asset", namespace, seed])` | `InvalidObjectAddress` |
| C13 | address state, see below | `ObjectAddressInUse` |
| C14 | empty address: `payer.lamports ≥ rent(72 + payload_len)`. Pre-funded shell with `current < rent`: `payer.lamports ≥ rent − current`. Not checked when `current ≥ rent` | `InsufficientFunds` |

Notes on the checks:

- C6 requires `payer ≠ authority` in particular. Otherwise the authority signature, passed into the System CPI, would let lamports be debited from the authority PDA.
- There is no check of `namespace.executable`: the authority signature (C8) already proves that the issuer program is executing.

**Account creation:**

- **Empty address** (0 lamports): `create_account` via `invoke_signed` with the object seeds, `space = 72 + payload_len`.
- **Pre-funded address** (owner = System, `data_len == 0`, lamports > 0):
  1. top up to rent-exempt from `payer`;
  2. `allocate` via `invoke_signed`;
  3. `assign` via `invoke_signed`.

  Excess lamports stay in the account and go to the owner when the asset is consumed.
- **Any other state:** `ObjectAddressInUse`.

The program then writes the header (with `authority_bump` and `reserved = 0`) and the payload.

**Paths unreachable on a correct runtime:**

- After the CPI the program checks `object.owner == Mass Asset` and the account size, otherwise it returns `InvalidAccountSize`.
- If the payer would drop below rent-exempt, the runtime rejects the transaction.
- If `payer` is not owned by the System Program, a System Program error is returned (without the `0x4D41` prefix).

### 4.1 `transfer` — tag `1`

Data: `new_owner[32]`.

Accounts:

| # | Account | Properties |
|---:|---|---|
| 0 | `object` | writable |
| 1 | `signer` | signer |

| # | Check | Error |
|---|---|---|
| T1 | `len(data) == 32` | `InvalidInstruction` |
| T2 | `object` is writable | `AccountNotWritable` |
| T3 | `validate(object)` | §3.3 |
| T4 | mode `owner`: `signer.is_signer` and `signer.key == object.owner` | `InvalidAuthority` |
| | mode `namespace`: `require_authority` | `InvalidAuthority` |
| | mode `frozen` | `TransferDisabled` |
| T5 | `new_owner ∉ {default, object, MASS_ASSET_PROGRAM_ID, object.namespace, authority}`, where `authority = create_program_address(["mass-asset-authority", [authority_bump]], namespace)` | `InvalidOwner` |

Effect: `owner = new_owner`. T5 adds about 1,500 CU to every transfer.

### 4.2 `write_payload` — tag `2`

Data: `offset u16 | bytes[..]`.

Accounts:

| # | Account | Properties |
|---:|---|---|
| 0 | `object` | writable |
| 1 | `authority` | signer |

| # | Check | Error |
|---|---|---|
| W1 | `len(data) ≥ 2` | `InvalidInstruction` |
| W2 | `object` is writable | `AccountNotWritable` |
| W3 | `validate(object)` | §3.3 |
| W4 | `require_authority` | `InvalidAuthority` |
| W5 | `offset + len(bytes) ≤ payload_len` (checked arithmetic) | `PayloadOutOfBounds` |

Effect: the payload range is overwritten. The header (bytes 0–71) cannot be written.

### 4.3 `consume` — tag `3`

Data: none.

Accounts:

| # | Account | Properties |
|---:|---|---|
| 0 | `object` | writable |
| 1 | `authority` | signer |
| 2 | `owner_account` | writable |

| # | Check | Error |
|---|---|---|
| K1 | no data | `InvalidInstruction` |
| K2 | `object` and `owner_account` are writable | `AccountNotWritable` |
| K3 | `validate(object)` | §3.3 |
| K4 | `require_authority` | `InvalidAuthority` |
| K5 | `owner_account == object.owner` | `RefundMismatch` |
| K6 | `owner_account ≠ object` (defence in depth) | `DuplicateAccount` |

Effect — **hard close**:

1. zero the data;
2. `resize(0)`;
3. move all lamports to `owner_account`;
4. `assign(System)`.

If the destination balance would overflow u64, the instruction fails with `ArithmeticOverflow` before any state change (unreachable in practice).

### 4.4 `orphan_close` — tag `4`

Data: none.

Accounts:

| # | Account | Properties |
|---:|---|---|
| 0 | `object` | writable |
| 1 | `owner` | signer, writable |
| 2 | `namespace` | readonly |
| 3 | `namespace_programdata` | readonly |

| # | Check | Error |
|---|---|---|
| O1 | no data | `InvalidInstruction` |
| O2 | `object` and `owner` are writable | `AccountNotWritable` |
| O3 | `validate(object)` | §3.3 |
| O4 | `owner.is_signer` and `owner.key == object.owner` | `InvalidAuthority` |
| O5 | `namespace.key == object.namespace` | `NamespaceMismatch` |
| O6 | `namespace_absent(namespace, namespace_programdata)` | `NamespaceAlive` |

Effect: hard close (§4.3, including the `ArithmeticOverflow` guard); the lamports go to `owner`.

Predicate:

```text
namespace_absent(ns, pd) :=
      ns.owner == BPFLoaderUpgradeab1e11111111111111111111111
  AND ns.data.len() >= 36
  AND u32_le(ns.data[0..4]) == 2            // UpgradeableLoaderState::Program
  AND pd.key == Pubkey(ns.data[4..36])      // programdata_address
  AND pd.owner != BPFLoaderUpgradeab1e11111111111111111111111
```

- The check is on the **owner** of the ProgramData account, not on its lamports (Appendix A.2).
- In v1, issuers under any other loader are never considered absent.
- Support for other loaders (for example loader-v4) can only be added by a **new program** (a new program id) with its own predicate. The deployed v1 program never changes (§9).

### 4.5 Reserved tags

Tags 5–255 are reserved. A missing tag or any of tags 5–255 returns `InvalidInstruction`.

## 5. Invariants

| ID | Invariant |
|---|---|
| I1 | Every live account owned by Mass Asset passes `validate` |
| I2 | The `namespace` of a live asset equals the program whose canonical authority signed its `create`; `authority_bump` is that authority's canonical bump. Both are immutable |
| I3 | `version`, `flags`, `payload_len`, `namespace`, `authority_bump`, `reserved` and the account size never change during the asset's life |
| I4 | Only the authority of `object.namespace` can change the `payload` |
| I5 | Only the party allowed by `transfer_mode` can change `owner`; in `frozen` mode nobody can. `owner` is never `default`, the asset's own address, the Mass Asset program, the issuer program or the issuer authority |
| I6 | An asset can be closed only by `consume` (issuer authority) or by `orphan_close` (owner, when the issuer is absent) |
| I7 | On close, all of the asset's lamports go to the current `owner` |
| I8 | Immediately after close, the address is either deleted by the runtime or a System-owned shell with zero data; it is not a Mass Asset account until a new `create` |
| I9 | Mass Asset owns no shared accounts. Write locks still apply to the payer and owner accounts: an escrow that uses a single vault PDA as owner creates its own hot account |

## 6. Issuer obligations (normative)

Mass Asset cannot see who stands between it and the issuer. The security of a namespace depends on these rules.

| ID | Rule |
|---|---|
| E1 | The PDA `["mass-asset-authority"]` is used **only** as the Mass Asset authority. It MUST NOT hold tokens, lamports or data. It MUST NOT be set as an asset owner. It MUST NOT be passed as a signer to any program other than Mass Asset. Before every `invoke_signed` with the authority seeds, the issuer MUST check that `ix.program_id == MASS_ASSET_PROGRAM_ID`, a constant in the issuer's code. Mass Asset itself also rejects the authority as an asset owner (C7, T5) |
| E2 | The issuer MUST NOT derive any other PDA whose concatenated seeds equal `mass-asset-authority`, for example `["mass-asset-", "authority"]`. User-supplied data MUST NOT enter seeds the issuer signs with, unless a prefix rules out such a collision |
| E3 | The issuer MUST hard-code its authority bump as a constant (computed offline and checked by a test), not call `find_program_address` at runtime. Otherwise the issuer pays a variable cost per call |
| E4 | The test issuer in `tests/onchain/test-issuer` is a signing proxy for tests. It violates E1 and MUST NOT be used as a template |

Future Mass Asset versions MUST use a first seed that does not start with `mass-asset` (for example `ma2-…`), so that their addresses cannot collide with v1 addresses.

## 7. Integration rules for value-backed assets (normative for such issuers)

These rules apply when an asset represents value held elsewhere by the issuer, for example tokens in a vault that are paid out on redemption. Such an issuer MUST follow every rule in this table.

| ID | Rule |
|---|---|
| R1 | Before reading the payload, the issuer MUST check `object.owner == MASS_ASSET_PROGRAM_ID`, `version == 1`, `namespace ==` its own program id, and the expected `payload_len`, and then the payload's own version, flags and field invariants |
| R2 | Every owner-initiated action that moves value MUST require the signature of the **current** `object.owner`, read from the account at execution time |
| R3 | The issuer MUST pay tokens only as follows. The token program id MUST be a constant in the issuer's code, never taken from caller accounts. The mint MUST be a constant or issuer state. The recipient account MUST be owned (at the account level) by that token program, with valid parsed state, the expected mint, and token owner = the current `object.owner`. The payout MUST be a CPI into that token program |
| R4 | The issuer MUST read the payload, pay out, and call `consume` in the same instruction, with `consume` on the same `AccountInfo` |
| R5 | The issuer MUST NOT hold a `RefCell` borrow of the object data across a CPI |
| R6 | The issuer MUST NOT rely on a separate owner → asset map: it goes stale after `transfer` |

**Secondary markets:**

- A trade MUST check payload bounds (for example a minimum value) atomically, because the issuer can change the payload between listing and execution.
- If the asset is held in escrow (`owner` = an escrow PDA), redemption pays both the rent and the value to the escrow PDA. The escrow has to handle this.
- A namespace is permanent. An issuer cannot move its assets to a new program id: it upgrades in place, or its holders rely on `orphan_close` after the issuer program is closed.

**Example payload** (illustrative, 48 bytes, starting at offset 72 so 8-byte aligned):

| Payload offset | Field |
|---:|---|
| 0 | `version u8` |
| 1 | `flags u8` |
| 2 | `param u16` |
| 4 | `epoch u32` |
| 8 | `amount u64` |
| 16 | `nominal u64` |
| 24 | `last_update i64` |
| 32 | `unlock_ts i64` |
| 40 | `id u64` |

An asset with a 48-byte payload is 120 bytes. Its rent on X1 is 1,726,080 lamports ≈ 0.0017 XNT. The payer pays it at `create`, and it is returned to the current owner on close.

## 8. Errors

Every Mass Asset error is returned as `ProgramError::Custom(0x4D41_0000 + code)` ("MA"), so it never collides with System Program or issuer codes.

| Code | Name |
|---:|---|
| 0 | `InvalidInstruction` |
| 1 | `UnsupportedVersion` |
| 2 | `InvalidFlags` |
| 3 | `InvalidAccountSize` |
| 4 | `InvalidAuthority` |
| 5 | *(unassigned; never to be used)* |
| 6 | `InvalidObjectAddress` |
| 7 | `ObjectAddressInUse` |
| 8 | `InvalidOwner` |
| 9 | `TransferDisabled` |
| 10 | `PayloadOutOfBounds` |
| 11 | `RefundMismatch` |
| 12 | `NamespaceAlive` |
| 13 | `DuplicateAccount` |
| 14 | `ArithmeticOverflow` |
| 15 | `NotMassAsset` |
| 16 | `AccountNotWritable` |
| 17 | `NamespaceMismatch` |
| 18 | `InsufficientFunds` |
| 19 | `InvalidHeader` |
| 20 | `MissingSigner` |
| 21 | `InvalidSystemProgram` |

Built-in errors Mass Asset can also return:

- `NotEnoughAccountKeys` — missing accounts;
- account borrow errors (`AccountBorrowFailed`);
- System Program errors inside a CPI, when the runtime rejects a transfer. These have no `0x4D41` prefix.

## 9. Operations and external assumptions

- **Runtime rules.** Rent return and the hard-close behaviour rely on the current X1/SVM rent and account rules. `namespace_absent` (§4.4) likewise relies on the current upgradeable-loader behaviour: closing a program deletes its ProgramData, and a closed program id cannot be redeployed. These are assumptions about the runtime, not laws of the chain.
- **Fees.** On X1 the fee is charged for the **requested** CU limit. Clients MUST simulate immediately before sending and set the limit to the simulated value plus a margin. Recommended margin: `create` +8,000, others +2,000. The cost of `create` depends on the `seed` (object bump search) and on whether the address is pre-funded.
- **CPI depth.** `create` uses two stack levels: Mass Asset and System. The chain router → aggregator → issuer → Mass Asset → System already reaches the Agave maximum of 5.
- **Immutability.** The official Mass Asset v1 program is deployed as **immutable**: it has no upgrade authority from the moment of deployment. Its layout (§3), instructions, check order and error codes (§4, §8) can never change.
  - Any fix or extension (including support for other loaders) is published as a **new program with a new program id**. Assets created under a program id stay under that program id forever.
  - Issuers SHOULD be able to work with more than one Mass Asset program id (the current one and earlier ones), so that their existing assets remain redeemable after they adopt a newer program.
  - The official program id and the hash of its verifiable build are published in the repository README.
- **Loader-v4.** If X1 enables migration of programs to loader-v4, a migrated issuer keeps working, but `orphan_close` never fires for its assets (§4.4). Issuers that rely on `orphan_close` SHOULD NOT migrate to loader-v4 unless a Mass Asset program with a loader-v4 predicate is available and adopted.

---

## Appendix A — Rationale (informative)

### A.1 Hard close

A "soft" close (zero the data, move the lamports, leave everything else to end-of-transaction garbage collection) is unsafe. Someone can re-fund the closed address in a later instruction of the same transaction, before the runtime removes it. The account then survives, still owned by Mass Asset, with zeroed data. Such a zombie can neither be closed nor recreated, which blocks the address permanently.

The hard close (zero → `resize(0)` → move all lamports → `assign(System)`) leaves at most an empty System-owned shell. On X1, re-funding a closed address with 1 lamport is rejected by the runtime. Re-funding it to rent-exempt produces a System shell, which `create` can safely reuse (the pre-funded path in §4.0). Rent is returned to the lamport.

Physical reuse of an address is not reuse of a logical identity: v1 makes no identity claim beyond the live account (§1).

### A.2 Why `namespace_absent` checks the ProgramData owner

On X1, after `solana program close` the **program account does not change**: it stays executable, owned by the upgradeable loader, in state `Program`. Only the ProgramData account disappears. Predicates based on the program account therefore never detect a closed program.

A predicate such as "ProgramData has zero lamports / does not exist" can be griefed. Anyone can send 890,880 lamports (the 0-byte rent-exempt minimum) to the dead ProgramData address, which turns it into a System-owned shell, and `orphan_close` would be blocked forever. A live program's ProgramData is always owned by the loader. Since the loader does not recreate a closed ProgramData address (see below), a dead address can only be absent or a System shell, so the ownership test is both safe and permanent.

On X1 mainnet:

- a closed program id could not be redeployed through the CLI;
- raw loader-v3 instructions against a closed id were all refused, executed by simulation on mainnet state: `DeployWithMaxDataLen` (with and without the program signature), `Upgrade`, `ExtendProgram`, `ExtendProgramChecked`, `SetAuthority`, `InitializeBuffer` at the ProgramData address, `Close`, and invocation.

This covers the tested paths; it is not a proof over every possible instruction sequence, and it is part of the runtime assumption in §9.

A failed CPI cannot serve as the test, because it aborts the whole transaction.

### A.3 Why the authority bump is stored in the header

Re-running `find_program_address` on every authority check costs about 1,500 CU per failed bump attempt. That makes the cost of every authority-gated instruction depend on the issuer's program id.

`create` must run the canonical search anyway. It stores the bump it finds in byte 68, which no instruction can write afterwards. Later checks use a single `create_program_address`, so the cost is constant for every issuer. This does not weaken the canonical-bump rule: the bump is computed on-chain, never supplied by a caller, and immutable.

The header is 72 bytes, which also aligns the payload to 8 bytes for zero-copy reads. The object bump is not stored, because no instruction uses it after `create`.

### A.4 Error namespace

Errors raised inside a System CPI surface as `Custom(n)` with small `n`. With unprefixed codes, an underfunded payer would read as `UnsupportedVersion`. The `0x4D41_0000` base keeps Mass Asset codes unambiguous. The check order is normative, so independent implementations and clients agree on which error a given input produces.

### A.5 Owner restrictions

- The asset's own address, the Mass Asset program and the issuer program are rejected as owners. An executable owner can never receive the rent, and the asset's own address can never sign, so such assets could never be consumed or orphan-closed.
- The issuer authority is rejected because it would put an `owner`-mode asset under issuer control.

Other executable addresses cannot be detected at `create` or `transfer` without the owner's account, so they remain the responsibility of the issuer and the holder (§2.3).

### A.6 Pairwise-distinct roles in `create`

If `payer` were allowed to equal `authority`, the authority signature would extend into the System CPI. Lamports that ended up on the authority PDA could then be spent through a user-facing create path. Requiring all four roles (`authority`, `namespace`, `payer`, `object`) to be distinct closes this, together with every other aliasing combination.
