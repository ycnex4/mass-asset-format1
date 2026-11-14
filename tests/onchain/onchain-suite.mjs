// Mass Asset v1 — on-chain conformance suite for X1 (spec: docs/spec.md).
// No private key is included. The wallet path comes from PROBE_WALLET (set by run.sh).
//
// 72-byte header (authority_bump at 68); errors = Custom(0x4D410000 + n).
//
// Usage (called by run.sh):
//   node onchain-suite.mjs live     <mass-asset-id> <issuer-id>   <state-file>
//   node onchain-suite.mjs issuer-b <mass-asset-id> <issuer-b-id> <state-file>
//   node onchain-suite.mjs orphan   <mass-asset-id> <issuer-id>   <state-file>
//
// Phase "live":     issuer deployed. Positive checks P1–P9, negative checks N1–N19 and N21–N36 (simulated).
// Phase "issuer-b": a second issuer with a different authority bump. Mass Asset's OWN CU for
//                   authority-gated instructions must be identical to phase "live" (U1–U4, spec §3.4).
// Phase "orphan":   first issuer closed. orphan_close checks P10–P11, N20, incl. a re-funded
//                   dead ProgramData shell (spec §4.4).

import fs from "node:fs";
import crypto from "node:crypto";
import {
  ComputeBudgetProgram,
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
  sendAndConfirmTransaction,
} from "@solana/web3.js";

const RPC = process.env.X1_RPC ?? "https://rpc.mainnet.x1.xyz";
const [, , PHASE, massArg, issuerArg, STATE_FILE] = process.argv;
if (!PHASE || !massArg || !issuerArg || !STATE_FILE) {
  console.error("usage: node onchain-suite.mjs <live|issuer-b|orphan> <mass-asset-id> <issuer-id> <state-file>");
  process.exit(2);
}

const connection = new Connection(RPC, "confirmed");
const wallet = Keypair.fromSecretKey(
  Uint8Array.from(JSON.parse(fs.readFileSync(process.env.PROBE_WALLET, "utf8"))),
);
const MASS = new PublicKey(massArg);
const ISSUER = new PublicKey(issuerArg);
const LOADER = new PublicKey("BPFLoaderUpgradeab1e11111111111111111111111");
const [AUTHORITY, AUTHORITY_BUMP] = PublicKey.findProgramAddressSync([Buffer.from("mass-asset-authority")], ISSUER);
const LOADER_V4 = new PublicKey("LoaderV411111111111111111111111111111111111");
const HEADER_LEN = 72;
const ERROR_BASE = 0x4d410000;
const [PROGRAMDATA] = PublicKey.findProgramAddressSync([ISSUER.toBuffer()], LOADER);
const DEFAULT_KEY = new PublicKey(new Uint8Array(32));

const ERR = {
  0: "InvalidInstruction", 1: "UnsupportedVersion", 2: "InvalidFlags", 3: "InvalidAccountSize",
  4: "InvalidAuthority", 6: "InvalidObjectAddress",
  7: "ObjectAddressInUse", 8: "InvalidOwner", 9: "TransferDisabled", 10: "PayloadOutOfBounds",
  11: "RefundMismatch", 12: "NamespaceAlive", 13: "DuplicateAccount", 14: "ArithmeticOverflow",
  15: "NotMassAsset", 16: "AccountNotWritable", 17: "NamespaceMismatch", 18: "InsufficientFunds",
  19: "InvalidHeader", 20: "MissingSigner", 21: "InvalidSystemProgram",
};
const CODE = Object.fromEntries(Object.entries(ERR).map(([k, v]) => [v, Number(k)]));

const results = [];
const liveMassCu = {};
const metrics = [];
const record = (id, ok, detail) => {
  results.push({ id, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${id}  ${detail}`);
};

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

const u16 = (n) => { const b = Buffer.alloc(2); b.writeUInt16LE(n); return b; };
const u32le = (n) => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
const u64le = (n) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
const objectAddress = (seed, ns = ISSUER) =>
  PublicKey.findProgramAddressSync([Buffer.from("mass-asset"), ns.toBuffer(), seed], MASS)[0];

const createData = (seed, mode, owner, payload, declaredLen = payload.length) =>
  Buffer.concat([Buffer.from([0]), seed, Buffer.from([mode]), u16(declaredLen), owner.toBuffer(), payload]);
const transferData = (newOwner) => Buffer.concat([Buffer.from([1]), newOwner.toBuffer()]);
const writeData = (offset, bytes) => Buffer.concat([Buffer.from([2]), u16(offset), bytes]);
const consumeData = () => Buffer.from([3]);
const orphanData = () => Buffer.from([4]);

const ro = (pubkey, isSigner = false) => ({ pubkey, isSigner, isWritable: false });
const rw = (pubkey, isSigner = false) => ({ pubkey, isSigner, isWritable: true });

// Direct call to Mass Asset.
const direct = (keys, data) => new TransactionInstruction({ programId: MASS, keys, data });
// Call through the issuer proxy: it signs as AUTHORITY.
const viaIssuer = (keys, data) =>
  new TransactionInstruction({ programId: ISSUER, keys: [ro(MASS), ...keys], data });

const createKeys = (object, namespace = ISSUER) => [
  ro(AUTHORITY), ro(namespace), rw(wallet.publicKey, true), rw(object), ro(SystemProgram.programId),
];

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function txMeta(sig) {
  for (let i = 0; i < 30; i += 1) {
    const tx = await connection.getTransaction(sig, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
    if (tx) return tx.meta;
    await sleep(500);
  }
  return null;
}

// Fee on X1 is proportional to the REQUESTED CU limit (spec §9, Fees). Measured on X1:
// create <= 23k CU, consume ~10k, transfer ~5k, orphan_close ~4k.
async function send(label, ixs, extraSigners = [], units = 40_000) {
  const tx = new Transaction().add(ComputeBudgetProgram.setComputeUnitLimit({ units }), ...ixs);
  try {
    const sig = await sendAndConfirmTransaction(connection, tx, [wallet, ...extraSigners], { commitment: "confirmed" });
    const meta = await txMeta(sig);
    const massCu = massOwnCu(meta?.logMessages ?? []);
    metrics.push({ label, sig, fee: meta?.fee, cu: meta?.computeUnitsConsumed, massCu });
    console.log(`  tx ${label}: ${sig} fee=${meta?.fee} CU=${meta?.computeUnitsConsumed} massCU=${massCu}`);
    return { ok: true, sig, massCu };
  } catch (e) {
    console.log(`  tx ${label} FAILED: ${e?.message ?? e}`);
    if (e?.logs) console.log("  logs:", JSON.stringify(e.logs, null, 2));
    return { ok: false, error: e };
  }
}

// Mass Asset's own CU (as reported for its invocation, including its System CPIs).
function massOwnCu(logs) {
  const re = new RegExp(`^Program ${MASS.toBase58()} consumed (\\d+) of`);
  for (const line of logs) {
    const m = line.match(re);
    if (m) return Number(m[1]);
  }
  return null;
}

function errorCode(err) {
  const ie = err?.InstructionError;
  if (!ie) return { kind: "other", value: JSON.stringify(err) };
  const inner = ie[1];
  if (inner && typeof inner === "object" && "Custom" in inner) return { kind: "custom", value: inner.Custom };
  return { kind: "builtin", value: typeof inner === "string" ? inner : JSON.stringify(inner) };
}

// Simulate and expect a specific failure. `expected` is a Mass Asset error name or a builtin name.
async function expectFail(id, ixs, extraSigners, expected) {
  const latest = await connection.getLatestBlockhash("confirmed");
  const tx = new Transaction({ feePayer: wallet.publicKey, recentBlockhash: latest.blockhash })
    // A CU limit different from send() keeps the simulated message distinct from any
    // already-sent transaction. Otherwise N7 would reuse P1's exact instruction + blockhash
    // and the RPC would answer AlreadyProcessed instead of executing it.
    .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 199_999 }), ...ixs);
  tx.sign(wallet, ...extraSigners);
  let sim;
  try {
    sim = await connection.simulateTransaction(tx);
  } catch (e) {
    record(id, false, `simulation threw: ${e?.message ?? e}`);
    return;
  }
  const err = sim.value.err;
  if (!err) {
    record(id, false, `expected ${expected}, but simulation SUCCEEDED`);
    return;
  }
  const code = errorCode(err);
  const got = code.kind === "custom"
    ? (code.value >= ERROR_BASE ? ERR[code.value - ERROR_BASE] ?? `MA(${code.value - ERROR_BASE})` : `Custom(${code.value})`)
    : code.value;
  const ok = got === expected;
  record(id, ok, `expected ${expected}, got ${got}`);
  if (!ok) console.log("  logs:", JSON.stringify(sim.value.logs ?? [], null, 2));
}

async function simulate(ixs, extraSigners) {
  const latest = await connection.getLatestBlockhash("confirmed");
  const tx = new Transaction({ feePayer: wallet.publicKey, recentBlockhash: latest.blockhash })
    .add(ComputeBudgetProgram.setComputeUnitLimit({ units: 199_998 }), ...ixs);
  tx.sign(wallet, ...extraSigners);
  return (await connection.simulateTransaction(tx)).value;
}

async function expectOkSim(id, ixs, extraSigners) {
  const v = await simulate(ixs, extraSigners);
  record(id, v.err === null, v.err === null ? "simulation succeeded" : `unexpected error ${JSON.stringify(v.err)}`);
}

// Observation only (decision input, not pass/fail).
async function observe(id, ixs, extraSigners) {
  const v = await simulate(ixs, extraSigners);
  console.log(`OBSERVE  ${id}: err=${JSON.stringify(v.err)}`);
  for (const l of (v.logs ?? []).filter((x) => /error|failed|MassAsset/i.test(x))) console.log(`    ${l}`);
  return v.err;
}

// ---------------------------------------------------------------------------
// State inspection
// ---------------------------------------------------------------------------

async function readObject(address) {
  const info = await connection.getAccountInfo(address, "confirmed");
  if (!info) return null;
  const d = Buffer.from(info.data);
  return {
    owner: info.owner.toBase58(),
    lamports: info.lamports,
    size: d.length,
    version: d[0],
    flags: d[1],
    payloadLen: d.readUInt16LE(2),
    namespace: new PublicKey(d.subarray(4, 36)).toBase58(),
    assetOwner: new PublicKey(d.subarray(36, 68)).toBase58(),
    authorityBump: d[68],
    reserved: Array.from(d.subarray(69, 72)),
    header: Buffer.from(d.subarray(0, HEADER_LEN)),
    payload: d.subarray(HEADER_LEN),
  };
}

async function sweep(kp) {
  const bal = await connection.getBalance(kp.publicKey, "confirmed");
  if (bal > 0) {
    await send(`sweep ${kp.publicKey.toBase58().slice(0, 6)}`, [
      SystemProgram.transfer({ fromPubkey: kp.publicKey, toPubkey: wallet.publicKey, lamports: bal }),
    ], [kp], 10_000);
  }
}

const rentFor = (payloadLen) => connection.getMinimumBalanceForRentExemption(HEADER_LEN + payloadLen, "confirmed");

// ---------------------------------------------------------------------------
// Phase: live
// ---------------------------------------------------------------------------

async function phaseLive() {
  // Environment: is loader-v4 present on this cluster? (spec §4.4, §9)
  const v4 = await connection.getAccountInfo(LOADER_V4, "confirmed");
  console.log(`ENV LoaderV4 account: ${v4 ? `exists, executable=${v4.executable}, owner=${v4.owner.toBase58()}` : "does not exist"}`);

  const alice = Keypair.generate();
  const bob = Keypair.generate();
  console.log("Alice:", alice.publicKey.toBase58());
  console.log("Bob:  ", bob.publicKey.toBase58());

  // --- Positive checks --------------------------------------------------------
  console.log("\n=== POSITIVE CHECKS ===");

  // P1 create on an empty address (48-byte example payload (spec §7), owner mode).
  const s1 = crypto.randomBytes(32);
  const o1 = objectAddress(s1);
  const payload1 = Buffer.alloc(48, 0);
  payload1.write("TEST", 0);
  let r = await send("P1 create (empty address)", [viaIssuer(createKeys(o1), createData(s1, 0, alice.publicKey, payload1))]);
  let st = r.ok ? await readObject(o1) : null;
  record("P1 create empty", !!st && st.owner === MASS.toBase58() && st.version === 1 && st.flags === 0 &&
    st.payloadLen === 48 && st.size === 120 && st.namespace === ISSUER.toBase58() &&
    st.assetOwner === alice.publicKey.toBase58() && st.lamports === await rentFor(48) &&
    st.authorityBump === AUTHORITY_BUMP && st.reserved.every((b) => b === 0),
    st ? `size=${st.size} lamports=${st.lamports} authority_bump=${st.authorityBump} (expected ${AUTHORITY_BUMP})` : "not created");

  // P2 create on a pre-funded System shell (namespace mode).
  const s2 = crypto.randomBytes(32);
  const o2 = objectAddress(s2);
  const rent0 = await connection.getMinimumBalanceForRentExemption(0, "confirmed");
  await send("P2a pre-fund address (System shell)", [
    SystemProgram.transfer({ fromPubkey: wallet.publicKey, toPubkey: o2, lamports: rent0 }),
  ], [], 10_000);
  r = await send("P2b create (pre-funded shell)", [viaIssuer(createKeys(o2), createData(s2, 1, alice.publicKey, Buffer.alloc(16, 7)))]);
  st = r.ok ? await readObject(o2) : null;
  record("P2 create pre-funded", !!st && st.owner === MASS.toBase58() && st.flags === 1 &&
    st.payloadLen === 16 && st.lamports === await rentFor(16),
    st ? `lamports=${st.lamports} (shell had ${rent0})` : "not created");

  // P3 write_payload via issuer; header must stay byte-identical.
  const headerBefore = (await readObject(o1))?.header;
  r = await send("P3 write_payload", [viaIssuer([rw(o1), ro(AUTHORITY)], writeData(4, Buffer.from("ABCDEFGH")))]);
  liveMassCu.write = r.massCu;
  st = r.ok ? await readObject(o1) : null;
  record("P3 write_payload", !!st && st.payload.subarray(0, 12).toString() === "TESTABCDEFGH" &&
    headerBefore && st.header.equals(headerBefore),
    st ? `payload[0..12]=${st.payload.subarray(0, 12).toString()} headerUnchanged=${headerBefore ? st.header.equals(headerBefore) : "?"}` : "failed");

  // P4 transfer in owner mode, signed by Alice.
  r = await send("P4 transfer owner-mode alice->bob", [direct([rw(o1), ro(alice.publicKey, true)], transferData(bob.publicKey))], [alice]);
  st = r.ok ? await readObject(o1) : null;
  record("P4 transfer owner-mode", !!st && st.assetOwner === bob.publicKey.toBase58(), st ? `owner=${st.assetOwner}` : "failed");

  // P5 transfer in namespace mode, signed by issuer authority.
  r = await send("P5 transfer namespace-mode", [viaIssuer([rw(o2), ro(AUTHORITY)], transferData(bob.publicKey))]);
  liveMassCu.transferNs = r.massCu;
  st = r.ok ? await readObject(o2) : null;
  record("P5 transfer namespace-mode", !!st && st.assetOwner === bob.publicKey.toBase58(), st ? `owner=${st.assetOwner}` : "failed");

  // P6 frozen asset.
  const s3 = crypto.randomBytes(32);
  const o3 = objectAddress(s3);
  r = await send("P6 create frozen", [viaIssuer(createKeys(o3), createData(s3, 2, alice.publicKey, Buffer.alloc(0)))]);
  st = r.ok ? await readObject(o3) : null;
  record("P6 create frozen", !!st && st.flags === 2 && st.size === 72, st ? `size=${st.size}` : "failed");

  // --- Negative checks (simulated, no fees) -----------------------------------
  console.log("\n=== NEGATIVE CHECKS (simulated) ===");
  const sN = crypto.randomBytes(32);
  const oN = objectAddress(sN);

  await expectFail("N1 create direct, authority not signer", [direct(createKeys(oN), createData(sN, 0, alice.publicKey, Buffer.alloc(0)))], [], "InvalidAuthority");
  await expectFail("N2 create via issuer under foreign namespace", [viaIssuer(createKeys(objectAddress(sN, MASS), MASS), createData(sN, 0, alice.publicKey, Buffer.alloc(0)))], [], "InvalidAuthority");
  await expectFail("N3 create wrong object address", [viaIssuer(createKeys(oN), createData(crypto.randomBytes(32), 0, alice.publicKey, Buffer.alloc(0)))], [], "InvalidObjectAddress");
  await expectFail("N4a create transfer_mode=3", [viaIssuer(createKeys(oN), createData(sN, 3, alice.publicKey, Buffer.alloc(0)))], [], "InvalidFlags");
  await expectFail("N4b create reserved flag bit", [viaIssuer(createKeys(oN), createData(sN, 4, alice.publicKey, Buffer.alloc(0)))], [], "InvalidFlags");
  await expectFail("N5 create payload_len 1025", [viaIssuer(createKeys(oN), createData(sN, 0, alice.publicKey, Buffer.alloc(0), 1025))], [], "InvalidAccountSize");
  await expectFail("N6 create owner = default", [viaIssuer(createKeys(oN), createData(sN, 0, DEFAULT_KEY, Buffer.alloc(0)))], [], "InvalidOwner");
  await expectFail("N7 create on live asset address", [viaIssuer(createKeys(o1), createData(s1, 0, alice.publicKey, payload1))], [], "ObjectAddressInUse");
  await expectFail("N8 transfer owner-mode by issuer authority", [viaIssuer([rw(o1), ro(AUTHORITY)], transferData(alice.publicKey))], [], "InvalidAuthority");
  await expectFail("N9 transfer by former owner (alice)", [direct([rw(o1), ro(alice.publicKey, true)], transferData(alice.publicKey))], [alice], "InvalidAuthority");
  await expectFail("N10 transfer frozen asset", [direct([rw(o3), ro(alice.publicKey, true)], transferData(bob.publicKey))], [alice], "TransferDisabled");
  await expectFail("N11 transfer to default pubkey", [direct([rw(o1), ro(bob.publicKey, true)], transferData(DEFAULT_KEY))], [bob], "InvalidOwner");
  await expectFail("N12 namespace-mode transfer by owner", [direct([rw(o2), ro(bob.publicKey, true)], transferData(alice.publicKey))], [bob], "InvalidAuthority");
  await expectFail("N13 write_payload by owner (not authority)", [direct([rw(o1), ro(bob.publicKey, true)], writeData(0, Buffer.from("X")))], [bob], "InvalidAuthority");
  await expectFail("N14 write_payload out of bounds", [viaIssuer([rw(o1), ro(AUTHORITY)], writeData(40, Buffer.alloc(9)))], [], "PayloadOutOfBounds");
  await expectFail("N15 consume with wrong refund account", [viaIssuer([rw(o1), ro(AUTHORITY), rw(alice.publicKey)], consumeData())], [], "RefundMismatch");
  await expectFail("N16 consume by owner directly", [direct([rw(o1), ro(bob.publicKey, true), rw(bob.publicKey)], consumeData())], [bob], "InvalidAuthority");
  await expectFail("N17 orphan_close while issuer alive", [direct([rw(o1), rw(bob.publicKey, true), ro(ISSUER), ro(PROGRAMDATA)], orphanData())], [bob], "NamespaceAlive");
  await expectFail("N18 orphan_close with fake ProgramData", [direct([rw(o1), rw(bob.publicKey, true), ro(ISSUER), ro(Keypair.generate().publicKey)], orphanData())], [bob], "NamespaceAlive");
  await expectFail("N19 orphan_close by non-owner", [direct([rw(o1), rw(alice.publicKey, true), ro(ISSUER), ro(PROGRAMDATA)], orphanData())], [alice], "InvalidAuthority");
  await expectFail("N21 transfer to the asset's own address", [direct([rw(o1), ro(bob.publicKey, true)], transferData(o1))], [bob], "InvalidOwner");
  await expectFail("N22 create with owner = object address", [viaIssuer(createKeys(oN), createData(sN, 0, oN, Buffer.alloc(0)))], [], "InvalidOwner");
  await expectFail("N23 create with object = authority", [viaIssuer(createKeys(AUTHORITY), createData(sN, 0, alice.publicKey, Buffer.alloc(0)))], [], "DuplicateAccount");
  const poor = Keypair.generate();
  await expectFail("N24 create with underfunded payer", [viaIssuer([ro(AUTHORITY), ro(ISSUER), rw(poor.publicKey, true), rw(oN), ro(SystemProgram.programId)], createData(sN, 0, alice.publicKey, Buffer.alloc(0)))], [poor], "InsufficientFunds");
  await expectFail("N25 consume with read-only owner account", [viaIssuer([rw(o1), ro(AUTHORITY), ro(bob.publicKey)], consumeData())], [], "AccountNotWritable");
  await expectFail("N26 orphan_close with wrong namespace", [direct([rw(o1), rw(bob.publicKey, true), ro(MASS), ro(PROGRAMDATA)], orphanData())], [bob], "NamespaceMismatch");
  await expectFail("N27 unknown instruction tag", [direct([rw(o1)], Buffer.from([9]))], [], "InvalidInstruction");
  await expectOkSim("N28 write_payload empty at offset == payload_len (no-op)", [viaIssuer([rw(o1), ro(AUTHORITY)], writeData(48, Buffer.alloc(0)))], []);

  // Unredeemable owners are rejected (spec §4.0 C7, §4.1 T5).
  await expectFail("N29 create with owner = Mass Asset program", [viaIssuer(createKeys(oN), createData(sN, 0, MASS, Buffer.alloc(0)))], [], "InvalidOwner");
  await expectFail("N30 create with owner = issuer program", [viaIssuer(createKeys(oN), createData(sN, 0, ISSUER, Buffer.alloc(0)))], [], "InvalidOwner");
  await expectFail("N31 transfer to Mass Asset program", [direct([rw(o1), ro(bob.publicKey, true)], transferData(MASS))], [bob], "InvalidOwner");
  await expectFail("N32 transfer to issuer program", [direct([rw(o1), ro(bob.publicKey, true)], transferData(ISSUER))], [bob], "InvalidOwner");
  // The issuer's authority PDA can never be an owner (spec §2.3).
  await expectFail("N33 create with owner = issuer authority PDA", [viaIssuer(createKeys(oN), createData(sN, 0, AUTHORITY, Buffer.alloc(0)))], [], "InvalidOwner");
  await expectFail("N34 transfer to issuer authority PDA", [direct([rw(o1), ro(bob.publicKey, true)], transferData(AUTHORITY))], [bob], "InvalidOwner");

  // payer == authority must be rejected even when the authority PDA holds lamports (spec §4.0 C6).
  console.log("\n=== PAYER = AUTHORITY (authority PDA pre-funded) ===");
  const rentA = await connection.getMinimumBalanceForRentExemption(0, "confirmed");
  await send("pre-fund authority PDA", [
    SystemProgram.transfer({ fromPubkey: wallet.publicKey, toPubkey: AUTHORITY, lamports: rentA * 3 }),
  ], [], 10_000);
  const authBalBefore = await connection.getBalance(AUTHORITY, "confirmed");
  await expectFail("N35 create with payer = authority PDA (funded)", [viaIssuer(
    [ro(AUTHORITY), ro(ISSUER), rw(AUTHORITY), rw(oN), ro(SystemProgram.programId)],
    createData(sN, 0, alice.publicKey, Buffer.alloc(0)))], [], "DuplicateAccount");
  await expectFail("N36 create with payer = issuer program", [viaIssuer(
    [ro(AUTHORITY), ro(ISSUER), rw(ISSUER), rw(oN), ro(SystemProgram.programId)],
    createData(sN, 0, alice.publicKey, Buffer.alloc(0)))], [], "DuplicateAccount");
  // Cleanup: the test proxy signs as authority, so it can sweep the PDA back via System.
  const sweepAuth = new TransactionInstruction({
    programId: ISSUER,
    keys: [ro(SystemProgram.programId), rw(AUTHORITY), rw(wallet.publicKey)],
    data: Buffer.concat([u32le(2), u64le(authBalBefore)]),
  });
  const swept = await send("cleanup: sweep authority PDA", [sweepAuth], [], 20_000);
  const authBalAfter = await connection.getBalance(AUTHORITY, "confirmed");
  console.log(`  authority PDA balance: before=${authBalBefore} after=${authBalAfter} (sweep ${swept.ok ? "ok" : "failed"})`);

  // Consume reference for U1–U4: destination = wallet (a transaction signer), same as phase issuer-b.
  const sW = crypto.randomBytes(32);
  const oW = objectAddress(sW);
  await send("U-ref create owner = wallet", [viaIssuer(createKeys(oW), createData(sW, 1, wallet.publicKey, Buffer.alloc(48)))]);
  r = await send("U-ref consume -> wallet", [viaIssuer([rw(oW), ro(AUTHORITY), rw(wallet.publicKey)], consumeData())]);
  liveMassCu.consume = r.massCu;

  // --- Consume -------------------------------------------------------------------
  console.log("\n=== CONSUME ===");
  for (const [label, obj, refund] of [["P7 consume o1 -> bob", o1, bob], ["P8 consume o2 -> bob", o2, bob], ["P9 consume frozen o3 -> alice", o3, alice]]) {
    const before = await readObject(obj);
    const balBefore = await connection.getBalance(refund.publicKey, "confirmed");
    r = await send(label, [viaIssuer([rw(obj), ro(AUTHORITY), rw(refund.publicKey)], consumeData())]);
    const after = await connection.getAccountInfo(obj, "confirmed");
    const balAfter = await connection.getBalance(refund.publicKey, "confirmed");
    const delta = balAfter - balBefore;
    record(label, r.ok && after === null && before && delta === before.lamports,
      `refund delta=${delta} expected=${before?.lamports} account=${after ? "STILL EXISTS" : "gone"}`);
  }

  // --- Objects for the orphan phase -------------------------------------------
  console.log("\n=== PREPARE ORPHAN PHASE ===");
  const s5 = crypto.randomBytes(32);
  const s6 = crypto.randomBytes(32);
  const o5 = objectAddress(s5);
  const o6 = objectAddress(s6);
  const r5 = await send("create o5 for orphan", [viaIssuer(createKeys(o5), createData(s5, 0, alice.publicKey, Buffer.alloc(48)))]);
  const r6 = await send("create o6 for orphan", [viaIssuer(createKeys(o6), createData(s6, 0, alice.publicKey, Buffer.alloc(48)))]);
  record("prepare orphan objects", r5.ok && r6.ok, `o5=${o5.toBase58()} o6=${o6.toBase58()}`);

  await sweep(bob);
  await sweep(alice);

  fs.writeFileSync(STATE_FILE, JSON.stringify({
    alice: Array.from(alice.secretKey),
    o5: o5.toBase58(),
    o6: o6.toBase58(),
    liveIssuer: ISSUER.toBase58(),
    liveAuthorityBump: AUTHORITY_BUMP,
    liveMassCu,
  }));
}

// ---------------------------------------------------------------------------
// Phase: issuer-b (second issuer, different authority bump) — constant authority cost (spec §3.4)
// ---------------------------------------------------------------------------

async function phaseIssuerB() {
  const state = JSON.parse(fs.readFileSync(STATE_FILE, "utf8"));
  console.log(`\n=== U1–U4 — CONSTANT AUTHORITY COST ACROSS ISSUERS ===`);
  console.log(`Issuer A ${state.liveIssuer} authority_bump=${state.liveAuthorityBump}`);
  console.log(`Issuer B ${ISSUER.toBase58()} authority_bump=${AUTHORITY_BUMP}`);
  const carol = Keypair.generate();
  const s = crypto.randomBytes(32);
  const o = objectAddress(s);
  const c = await send("issuer B create (namespace mode)", [viaIssuer(createKeys(o), createData(s, 1, carol.publicKey, Buffer.alloc(48)))]);
  const st = c.ok ? await readObject(o) : null;
  record("U1 issuer B asset stores its own authority bump", !!st && st.authorityBump === AUTHORITY_BUMP,
    st ? `authority_bump=${st.authorityBump} expected=${AUTHORITY_BUMP}` : "not created");
  const w = await send("issuer B write_payload", [viaIssuer([rw(o), ro(AUTHORITY)], writeData(4, Buffer.from("ABCDEFGH")))]);
  const t = await send("issuer B transfer namespace-mode", [viaIssuer([rw(o), ro(AUTHORITY)], transferData(wallet.publicKey))]);
  const k = await send("issuer B consume", [viaIssuer([rw(o), ro(AUTHORITY), rw(wallet.publicKey)], consumeData())]);
  const a = state.liveMassCu ?? {};
  for (const [uid, name, mine, theirs] of [["U2", "write_payload", w.massCu, a.write], ["U3", "transfer ns", t.massCu, a.transferNs], ["U4", "consume", k.massCu, a.consume]]) {
    record(`${uid} Mass Asset CU identical across issuers: ${name}`, mine !== null && mine === theirs,
      `issuer A=${theirs} issuer B=${mine}`);
  }
}

// ---------------------------------------------------------------------------
// Phase: orphan (issuer program already closed by the runner)
// ---------------------------------------------------------------------------

async function phaseOrphan() {
  const state = JSON.parse(fs.readFileSync(STATE_FILE, "utf8"));
  const alice = Keypair.fromSecretKey(Uint8Array.from(state.alice));
  const o5 = new PublicKey(state.o5);
  const o6 = new PublicKey(state.o6);

  console.log("\n=== ORPHAN CLOSE (issuer closed) ===");
  const pd = await connection.getAccountInfo(PROGRAMDATA, "confirmed");
  console.log("Issuer ProgramData after close:", pd ? { owner: pd.owner.toBase58(), lamports: pd.lamports } : "does not exist");

  const stranger = Keypair.generate();
  await expectFail("N20 orphan_close by non-owner after issuer closed",
    [direct([rw(o5), rw(stranger.publicKey, true), ro(ISSUER), ro(PROGRAMDATA)], orphanData())], [stranger], "InvalidAuthority");

  // P10 plain orphan_close.
  let before = await readObject(o5);
  let balBefore = await connection.getBalance(alice.publicKey, "confirmed");
  let r = await send("P10 orphan_close o5", [direct([rw(o5), rw(alice.publicKey, true), ro(ISSUER), ro(PROGRAMDATA)], orphanData())], [alice]);
  let after = await connection.getAccountInfo(o5, "confirmed");
  let delta = (await connection.getBalance(alice.publicKey, "confirmed")) - balBefore;
  record("P10 orphan_close", r.ok && after === null && before && delta === before.lamports, `refund delta=${delta} expected=${before?.lamports}`);

  // P11 griefing attempt: re-fund the dead ProgramData address into a System shell.
  const rent0 = await connection.getMinimumBalanceForRentExemption(0, "confirmed");
  await send("P11a re-fund dead ProgramData (griefing attempt)", [
    SystemProgram.transfer({ fromPubkey: wallet.publicKey, toPubkey: PROGRAMDATA, lamports: rent0 }),
  ], [], 10_000);
  const shell = await connection.getAccountInfo(PROGRAMDATA, "confirmed");
  console.log("Dead ProgramData now:", shell ? { owner: shell.owner.toBase58(), lamports: shell.lamports, size: shell.data.length } : "does not exist");

  before = await readObject(o6);
  balBefore = await connection.getBalance(alice.publicKey, "confirmed");
  r = await send("P11b orphan_close o6 after griefing", [direct([rw(o6), rw(alice.publicKey, true), ro(ISSUER), ro(PROGRAMDATA)], orphanData())], [alice]);
  after = await connection.getAccountInfo(o6, "confirmed");
  delta = (await connection.getBalance(alice.publicKey, "confirmed")) - balBefore;
  record("P11 orphan_close survives re-funded ProgramData shell", r.ok && after === null && before && delta === before.lamports,
    `shellOwner=${shell?.owner.toBase58()} refund delta=${delta}`);

  await sweep(alice);
}

// ---------------------------------------------------------------------------

console.log("======================================================");
console.log(` MASS ASSET v1 — ON-CHAIN SUITE — phase ${PHASE}`);
console.log("======================================================");
console.log("Mass Asset: ", MASS.toBase58());
console.log("Issuer:     ", ISSUER.toBase58());
console.log("Authority:  ", AUTHORITY.toBase58());
console.log("ProgramData:", PROGRAMDATA.toBase58());

if (PHASE === "live") await phaseLive();
else if (PHASE === "issuer-b") await phaseIssuerB();
else if (PHASE === "orphan") await phaseOrphan();
else { console.error("unknown phase"); process.exit(2); }

console.log(`\n=== SUMMARY phase ${PHASE} ===`);
const passed = results.filter((x) => x.ok).length;
console.log(`${passed}/${results.length} checks passed`);
for (const x of results.filter((y) => !y.ok)) console.log(`  FAILED: ${x.id} — ${x.detail}`);
console.log("\nMetrics (fee lamports / CU consumed):");
for (const m of metrics) console.log(`  ${m.label}: fee=${m.fee} CU=${m.cu} massCU=${m.massCu}`);
console.log("RESULT_JSON", JSON.stringify({ phase: PHASE, passed, total: results.length, results, metrics }));
