//! Mass Asset v1 — minimal on-chain asset for X1.
//!
//! Normative specification: `docs/spec.md` (Mass Asset v1.0).
//!
//! Account layout (little-endian), HEADER_LEN = 72:
//!   [0]       version         u8  = 1
//!   [1]       flags           u8  bits 0-1 transfer_mode, bits 2-7 must be 0
//!   [2..4]    payload_len     u16 (<= 1024)
//!   [4..36]   namespace       Pubkey (issuing program)
//!   [36..68]  owner           Pubkey
//!   [68]      authority_bump  u8  canonical bump of PDA(["mass-asset-authority"], namespace),
//!                                 written only by `create`
//!   [69..72]  reserved        must be 0
//!   [72..]    payload         [u8; payload_len] (8-byte aligned)
//!
//! Logs are static strings only (no formatting) to keep the binary small and CU low.

use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    msg,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    pubkey::Pubkey,
    rent::Rent,
    sysvar::Sysvar,
};

entrypoint!(process_instruction);

// ---------------------------------------------------------------------------
// Constants (frozen by spec v1)
// ---------------------------------------------------------------------------

pub const VERSION: u8 = 1;
pub const OBJECT_SEED: &[u8] = b"mass-asset";
pub const AUTHORITY_SEED: &[u8] = b"mass-asset-authority";
pub const HEADER_LEN: usize = 72;
pub const MAX_PAYLOAD: usize = 1024;

pub const OFF_VERSION: usize = 0;
pub const OFF_FLAGS: usize = 1;
pub const OFF_PAYLOAD_LEN: usize = 2;
pub const OFF_NAMESPACE: usize = 4;
pub const OFF_OWNER: usize = 36;
pub const OFF_AUTHORITY_BUMP: usize = 68;
pub const OFF_RESERVED: usize = 69;

const TRANSFER_MODE_MASK: u8 = 0b0000_0011;
const RESERVED_FLAGS_MASK: u8 = 0b1111_1100;

pub const MODE_OWNER: u8 = 0;
pub const MODE_NAMESPACE: u8 = 1;
pub const MODE_FROZEN: u8 = 2;

/// System program id (11111111111111111111111111111111).
pub const SYSTEM_PROGRAM_ID: Pubkey = Pubkey::new_from_array([0u8; 32]);

/// BPFLoaderUpgradeab1e11111111111111111111111
pub const BPF_LOADER_UPGRADEABLE: Pubkey = Pubkey::new_from_array([
    2, 168, 246, 145, 78, 136, 161, 176, 226, 16, 21, 62, 247, 99, 174, 43, 0, 194, 185, 61, 22,
    193, 36, 210, 192, 83, 122, 16, 4, 128, 0, 0,
]);

/// UpgradeableLoaderState::Program discriminant.
const LOADER_STATE_PROGRAM: u32 = 2;

pub const IX_CREATE: u8 = 0;
pub const IX_TRANSFER: u8 = 1;
pub const IX_WRITE_PAYLOAD: u8 = 2;
pub const IX_CONSUME: u8 = 3;
pub const IX_ORPHAN_CLOSE: u8 = 4;

// ---------------------------------------------------------------------------
// Errors (spec §8): ProgramError::Custom(ERROR_BASE + code)
// ---------------------------------------------------------------------------

/// "MA" — keeps Mass Asset codes apart from System Program and issuer codes.
pub const ERROR_BASE: u32 = 0x4D41_0000;

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub enum MassAssetError {
    InvalidInstruction = 0,
    UnsupportedVersion = 1,
    InvalidFlags = 2,
    InvalidAccountSize = 3,
    InvalidAuthority = 4,
    // 5 is reserved and never used.
    InvalidObjectAddress = 6,
    ObjectAddressInUse = 7,
    InvalidOwner = 8,
    TransferDisabled = 9,
    PayloadOutOfBounds = 10,
    RefundMismatch = 11,
    NamespaceAlive = 12,
    DuplicateAccount = 13,
    ArithmeticOverflow = 14,
    NotMassAsset = 15,
    AccountNotWritable = 16,
    NamespaceMismatch = 17,
    InsufficientFunds = 18,
    InvalidHeader = 19,
    MissingSigner = 20,
    InvalidSystemProgram = 21,
}

impl MassAssetError {
    fn name(self) -> &'static str {
        match self {
            Self::InvalidInstruction => "MassAsset: InvalidInstruction",
            Self::UnsupportedVersion => "MassAsset: UnsupportedVersion",
            Self::InvalidFlags => "MassAsset: InvalidFlags",
            Self::InvalidAccountSize => "MassAsset: InvalidAccountSize",
            Self::InvalidAuthority => "MassAsset: InvalidAuthority",
            Self::InvalidObjectAddress => "MassAsset: InvalidObjectAddress",
            Self::ObjectAddressInUse => "MassAsset: ObjectAddressInUse",
            Self::InvalidOwner => "MassAsset: InvalidOwner",
            Self::TransferDisabled => "MassAsset: TransferDisabled",
            Self::PayloadOutOfBounds => "MassAsset: PayloadOutOfBounds",
            Self::RefundMismatch => "MassAsset: RefundMismatch",
            Self::NamespaceAlive => "MassAsset: NamespaceAlive",
            Self::DuplicateAccount => "MassAsset: DuplicateAccount",
            Self::ArithmeticOverflow => "MassAsset: ArithmeticOverflow",
            Self::NotMassAsset => "MassAsset: NotMassAsset",
            Self::AccountNotWritable => "MassAsset: AccountNotWritable",
            Self::NamespaceMismatch => "MassAsset: NamespaceMismatch",
            Self::InsufficientFunds => "MassAsset: InsufficientFunds",
            Self::InvalidHeader => "MassAsset: InvalidHeader",
            Self::MissingSigner => "MassAsset: MissingSigner",
            Self::InvalidSystemProgram => "MassAsset: InvalidSystemProgram",
        }
    }
}

impl From<MassAssetError> for ProgramError {
    fn from(e: MassAssetError) -> Self {
        ProgramError::Custom(ERROR_BASE + e as u32)
    }
}

/// Log the error name (static string, no formatting) and return it.
fn err(e: MassAssetError) -> ProgramError {
    msg!(e.name());
    e.into()
}

fn fail(e: MassAssetError) -> ProgramResult {
    Err(err(e))
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    let (&tag, rest) = match instruction_data.split_first() {
        Some(x) => x,
        None => return fail(MassAssetError::InvalidInstruction),
    };

    match tag {
        IX_CREATE => process_create(program_id, accounts, rest),
        IX_TRANSFER => process_transfer(program_id, accounts, rest),
        IX_WRITE_PAYLOAD => process_write_payload(program_id, accounts, rest),
        IX_CONSUME => process_consume(program_id, accounts, rest),
        IX_ORPHAN_CLOSE => process_orphan_close(program_id, accounts, rest),
        _ => fail(MassAssetError::InvalidInstruction),
    }
}

// ---------------------------------------------------------------------------
// System Program instructions, encoded by hand (no bincode in the binary).
// Encoding: u32 LE variant index, then fields LE. Verified against
// solana-system-interface in the host unit tests below.
// ---------------------------------------------------------------------------

pub fn sys_create_account(from: &Pubkey, to: &Pubkey, lamports: u64, space: u64, owner: &Pubkey) -> Instruction {
    let mut data = Vec::with_capacity(52);
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    data.extend_from_slice(&space.to_le_bytes());
    data.extend_from_slice(owner.as_ref());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, true)],
        data,
    }
}

pub fn sys_assign(account: &Pubkey, owner: &Pubkey) -> Instruction {
    let mut data = Vec::with_capacity(36);
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(owner.as_ref());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*account, true)],
        data,
    }
}

pub fn sys_transfer(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, false)],
        data,
    }
}

pub fn sys_allocate(account: &Pubkey, space: u64) -> Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&8u32.to_le_bytes());
    data.extend_from_slice(&space.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*account, true)],
        data,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Canonical authority PDA and its bump (used only by `create`).
pub fn authority_of(namespace: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[AUTHORITY_SEED], namespace)
}

pub fn object_address(program_id: &Pubkey, namespace: &Pubkey, seed: &[u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[OBJECT_SEED, namespace.as_ref(), seed], program_id)
}

fn read_pubkey(data: &[u8], offset: usize) -> Result<Pubkey, ProgramError> {
    let slice = data
        .get(offset..offset + 32)
        .ok_or_else(|| err(MassAssetError::InvalidAccountSize))?;
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(slice);
    Ok(Pubkey::new_from_array(bytes))
}

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug))]
struct Header {
    transfer_mode: u8,
    payload_len: usize,
    namespace: Pubkey,
    owner: Pubkey,
    authority_bump: u8,
}

fn pubkey_at(data: &[u8], offset: usize) -> Pubkey {
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&data[offset..offset + 32]);
    Pubkey::new_from_array(bytes)
}

/// Spec §3.3 — pure header validation, in this exact order (unit-tested on host).
fn parse_header(account_owner: &Pubkey, program_id: &Pubkey, data: &[u8]) -> Result<Header, MassAssetError> {
    // V1
    if account_owner != program_id {
        return Err(MassAssetError::NotMassAsset);
    }
    // V2
    if data.len() < HEADER_LEN {
        return Err(MassAssetError::InvalidAccountSize);
    }
    // V3
    if data[OFF_VERSION] != VERSION {
        return Err(MassAssetError::UnsupportedVersion);
    }
    // V4
    let flags = data[OFF_FLAGS];
    let transfer_mode = flags & TRANSFER_MODE_MASK;
    if flags & RESERVED_FLAGS_MASK != 0 || transfer_mode > MODE_FROZEN {
        return Err(MassAssetError::InvalidFlags);
    }
    // V5
    if data[OFF_RESERVED..HEADER_LEN].iter().any(|b| *b != 0) {
        return Err(MassAssetError::InvalidHeader);
    }
    // V6
    let payload_len = u16::from_le_bytes([data[OFF_PAYLOAD_LEN], data[OFF_PAYLOAD_LEN + 1]]) as usize;
    if payload_len > MAX_PAYLOAD || data.len() != HEADER_LEN + payload_len {
        return Err(MassAssetError::InvalidAccountSize);
    }
    // V7
    let owner = pubkey_at(data, OFF_OWNER);
    if owner == Pubkey::default() {
        return Err(MassAssetError::InvalidOwner);
    }
    Ok(Header {
        transfer_mode,
        payload_len,
        namespace: pubkey_at(data, OFF_NAMESPACE),
        owner,
        authority_bump: data[OFF_AUTHORITY_BUMP],
    })
}

fn validate(program_id: &Pubkey, object: &AccountInfo) -> Result<Header, ProgramError> {
    let data = object.try_borrow_data()?;
    parse_header(object.owner, program_id, &data).map_err(err)
}

/// The issuer's authority address for an existing asset: one create_program_address
/// with the canonical bump stored at create (constant cost, spec §3.4).
fn authority_key(header: &Header) -> Option<Pubkey> {
    Pubkey::create_program_address(&[AUTHORITY_SEED, &[header.authority_bump]], &header.namespace).ok()
}

/// Spec §3.4 — pure authority check (unit-tested on host).
fn check_authority(is_signer: bool, key: &Pubkey, header: &Header) -> Result<(), MassAssetError> {
    // A1
    if !is_signer {
        return Err(MassAssetError::InvalidAuthority);
    }
    // A2
    match authority_key(header) {
        Some(expected) if expected == *key => Ok(()),
        _ => Err(MassAssetError::InvalidAuthority),
    }
}

fn require_authority(authority: &AccountInfo, header: &Header) -> ProgramResult {
    check_authority(authority.is_signer, authority.key, header).map_err(err)
}

/// Hard close (spec §4.3): clear -> resize(0) -> move all lamports -> assign(System).
fn hard_close(object: &AccountInfo, destination: &AccountInfo) -> ProgramResult {
    let amount = object.lamports();
    let destination_after = match destination.lamports().checked_add(amount) {
        Some(v) => v,
        None => return fail(MassAssetError::ArithmeticOverflow),
    };

    object.try_borrow_mut_data()?.fill(0);
    object.resize(0)?;

    **destination.try_borrow_mut_lamports()? = destination_after;
    **object.try_borrow_mut_lamports()? = 0;

    object.assign(&SYSTEM_PROGRAM_ID);
    msg!("MassAsset: closed");
    Ok(())
}

// ---------------------------------------------------------------------------
// 4.0 create
// ---------------------------------------------------------------------------
// data: seed[32] | transfer_mode u8 | payload_len u16 | owner[32] | payload[payload_len]
// accounts: 0 authority (signer), 1 namespace, 2 payer (signer, writable),
//           3 object (writable), 4 system program

fn process_create(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    const FIXED: usize = 32 + 1 + 2 + 32;
    // C1
    if data.len() < FIXED {
        return fail(MassAssetError::InvalidInstruction);
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&data[0..32]);
    let transfer_mode = data[32];
    let payload_len = u16::from_le_bytes([data[33], data[34]]) as usize;
    let owner = read_pubkey(data, 35)?;
    let payload = &data[FIXED..];

    // C2..C5 — instruction parameters.
    if transfer_mode > MODE_FROZEN {
        return fail(MassAssetError::InvalidFlags);
    }
    if payload_len > MAX_PAYLOAD {
        return fail(MassAssetError::InvalidAccountSize);
    }
    if payload.len() != payload_len {
        return fail(MassAssetError::InvalidInstruction);
    }
    if owner == Pubkey::default() {
        return fail(MassAssetError::InvalidOwner);
    }

    let it = &mut accounts.iter();
    let authority = next_account_info(it)?;
    let namespace = next_account_info(it)?;
    let payer = next_account_info(it)?;
    let object = next_account_info(it)?;
    let system_program_info = next_account_info(it)?;

    // C6 — all four roles pairwise distinct: in particular payer != authority,
    // otherwise the issuer's authority signature would let System debit the authority PDA.
    if object.key == authority.key
        || object.key == payer.key
        || object.key == namespace.key
        || payer.key == authority.key
        || payer.key == namespace.key
        || authority.key == namespace.key
    {
        return fail(MassAssetError::DuplicateAccount);
    }
    // C7 — no unredeemable owner: not the asset itself,
    // not the Mass Asset program, not the issuer program (executable owners can
    // never receive the rent), not the issuer's authority PDA (spec §2.3).
    let (expected_authority, authority_bump) = authority_of(namespace.key);
    if owner == *object.key
        || owner == *program_id
        || owner == *namespace.key
        || owner == expected_authority
    {
        return fail(MassAssetError::InvalidOwner);
    }
    // C8 — the namespace is bound to the program whose canonical authority PDA signed.
    if !authority.is_signer || *authority.key != expected_authority {
        return fail(MassAssetError::InvalidAuthority);
    }
    // C9..C11
    if !payer.is_signer {
        return fail(MassAssetError::MissingSigner);
    }
    if !payer.is_writable || !object.is_writable {
        return fail(MassAssetError::AccountNotWritable);
    }
    if *system_program_info.key != SYSTEM_PROGRAM_ID {
        return fail(MassAssetError::InvalidSystemProgram);
    }
    // C12 — canonical object bump only.
    let (expected_object, object_bump) = object_address(program_id, namespace.key, &seed);
    if *object.key != expected_object {
        return fail(MassAssetError::InvalidObjectAddress);
    }

    let space = HEADER_LEN + payload_len;
    let required = Rent::get()?.minimum_balance(space);
    let bump_seed = [object_bump];
    let signer_seeds: &[&[u8]] = &[OBJECT_SEED, namespace.key.as_ref(), &seed, &bump_seed];

    // C13/C14 — address state and payer funds.
    let current = object.lamports();
    if current == 0 {
        if payer.lamports() < required {
            return fail(MassAssetError::InsufficientFunds);
        }
        invoke_signed(
            &sys_create_account(payer.key, object.key, required, space as u64, program_id),
            &[payer.clone(), object.clone(), system_program_info.clone()],
            &[signer_seeds],
        )?;
    } else if *object.owner == SYSTEM_PROGRAM_ID && object.data_len() == 0 {
        // Pre-funded System-owned shell (e.g. after hard close + foreign re-fund).
        msg!("MassAsset: recovering pre-funded shell");
        if current < required {
            let top_up = required - current;
            if payer.lamports() < top_up {
                return fail(MassAssetError::InsufficientFunds);
            }
            invoke(
                &sys_transfer(payer.key, object.key, top_up),
                &[payer.clone(), object.clone(), system_program_info.clone()],
            )?;
        }
        invoke_signed(
            &sys_allocate(object.key, space as u64),
            &[object.clone(), system_program_info.clone()],
            &[signer_seeds],
        )?;
        invoke_signed(
            &sys_assign(object.key, program_id),
            &[object.clone(), system_program_info.clone()],
            &[signer_seeds],
        )?;
    } else {
        return fail(MassAssetError::ObjectAddressInUse);
    }

    if object.owner != program_id || object.data_len() != space {
        return fail(MassAssetError::InvalidAccountSize);
    }

    let mut out = object.try_borrow_mut_data()?;
    out[OFF_VERSION] = VERSION;
    out[OFF_FLAGS] = transfer_mode;
    out[OFF_PAYLOAD_LEN..OFF_PAYLOAD_LEN + 2].copy_from_slice(&(payload_len as u16).to_le_bytes());
    out[OFF_NAMESPACE..OFF_NAMESPACE + 32].copy_from_slice(namespace.key.as_ref());
    out[OFF_OWNER..OFF_OWNER + 32].copy_from_slice(owner.as_ref());
    out[OFF_AUTHORITY_BUMP] = authority_bump;
    out[OFF_RESERVED..HEADER_LEN].fill(0);
    out[HEADER_LEN..].copy_from_slice(payload);

    msg!("MassAsset: created");
    Ok(())
}

// ---------------------------------------------------------------------------
// 4.1 transfer
// ---------------------------------------------------------------------------
// data: new_owner[32]
// accounts: 0 object (writable), 1 signer

fn process_transfer(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    // T1
    if data.len() != 32 {
        return fail(MassAssetError::InvalidInstruction);
    }
    let new_owner = read_pubkey(data, 0)?;

    let it = &mut accounts.iter();
    let object = next_account_info(it)?;
    let signer = next_account_info(it)?;

    // T2..T3
    if !object.is_writable {
        return fail(MassAssetError::AccountNotWritable);
    }
    let header = validate(program_id, object)?;

    // T4 — mode-dependent signer.
    match header.transfer_mode {
        MODE_OWNER => {
            // Compare with the CURRENT stored owner.
            if !signer.is_signer || *signer.key != header.owner {
                return fail(MassAssetError::InvalidAuthority);
            }
        }
        MODE_NAMESPACE => require_authority(signer, &header)?,
        _ => return fail(MassAssetError::TransferDisabled),
    }
    // T5 — same owner rule as C7 (incl. the issuer's authority PDA).
    if new_owner == Pubkey::default()
        || new_owner == *object.key
        || new_owner == *program_id
        || new_owner == header.namespace
        || Some(new_owner) == authority_key(&header)
    {
        return fail(MassAssetError::InvalidOwner);
    }

    object.try_borrow_mut_data()?[OFF_OWNER..OFF_OWNER + 32].copy_from_slice(new_owner.as_ref());
    msg!("MassAsset: transferred");
    Ok(())
}

// ---------------------------------------------------------------------------
// 4.2 write_payload
// ---------------------------------------------------------------------------
// data: offset u16 | bytes[..]
// accounts: 0 object (writable), 1 authority (signer)

fn process_write_payload(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    // W1
    if data.len() < 2 {
        return fail(MassAssetError::InvalidInstruction);
    }
    let offset = u16::from_le_bytes([data[0], data[1]]) as usize;
    let bytes = &data[2..];

    let it = &mut accounts.iter();
    let object = next_account_info(it)?;
    let authority = next_account_info(it)?;

    // W2..W4
    if !object.is_writable {
        return fail(MassAssetError::AccountNotWritable);
    }
    let header = validate(program_id, object)?;
    require_authority(authority, &header)?;

    // W5
    let end = match offset.checked_add(bytes.len()) {
        Some(e) if e <= header.payload_len => e,
        _ => return fail(MassAssetError::PayloadOutOfBounds),
    };

    object.try_borrow_mut_data()?[HEADER_LEN + offset..HEADER_LEN + end].copy_from_slice(bytes);
    Ok(())
}

// ---------------------------------------------------------------------------
// 4.3 consume
// ---------------------------------------------------------------------------
// accounts: 0 object (writable), 1 authority (signer), 2 owner_account (writable)

fn process_consume(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    // K1
    if !data.is_empty() {
        return fail(MassAssetError::InvalidInstruction);
    }
    let it = &mut accounts.iter();
    let object = next_account_info(it)?;
    let authority = next_account_info(it)?;
    let owner_account = next_account_info(it)?;

    // K2..K4
    if !object.is_writable || !owner_account.is_writable {
        return fail(MassAssetError::AccountNotWritable);
    }
    let header = validate(program_id, object)?;
    // Only the issuer may consume.
    require_authority(authority, &header)?;
    // K5 — rent goes to the current stored owner only.
    if *owner_account.key != header.owner {
        return fail(MassAssetError::RefundMismatch);
    }
    // K6 (defence in depth; owner == object is rejected at create/transfer)
    if owner_account.key == object.key {
        return fail(MassAssetError::DuplicateAccount);
    }

    hard_close(object, owner_account)
}

// ---------------------------------------------------------------------------
// 4.4 orphan_close
// ---------------------------------------------------------------------------
// accounts: 0 object (writable), 1 owner (signer, writable), 2 namespace,
//           3 namespace_programdata

fn process_orphan_close(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    // O1
    if !data.is_empty() {
        return fail(MassAssetError::InvalidInstruction);
    }
    let it = &mut accounts.iter();
    let object = next_account_info(it)?;
    let owner = next_account_info(it)?;
    let namespace = next_account_info(it)?;
    let programdata = next_account_info(it)?;

    // O2..O3
    if !object.is_writable || !owner.is_writable {
        return fail(MassAssetError::AccountNotWritable);
    }
    let header = validate(program_id, object)?;
    // O4
    if !owner.is_signer || *owner.key != header.owner {
        return fail(MassAssetError::InvalidAuthority);
    }
    // O5
    if *namespace.key != header.namespace {
        return fail(MassAssetError::NamespaceMismatch);
    }
    // O6
    if !namespace_absent(namespace, programdata)? {
        return fail(MassAssetError::NamespaceAlive);
    }

    hard_close(object, owner)
}

/// Spec §4.4. Ownership-based, not lamport-based: a dead ProgramData address can be
/// re-funded into a System-owned shell, which must still count as absent.
fn namespace_absent(namespace: &AccountInfo, programdata: &AccountInfo) -> Result<bool, ProgramError> {
    if *namespace.owner != BPF_LOADER_UPGRADEABLE {
        return Ok(false);
    }
    let data = namespace.try_borrow_data()?;
    if data.len() < 36 {
        return Ok(false);
    }
    let state = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if state != LOADER_STATE_PROGRAM {
        return Ok(false);
    }
    let recorded = read_pubkey(&data, 4)?;
    if *programdata.key != recorded {
        return Ok(false);
    }
    Ok(*programdata.owner != BPF_LOADER_UPGRADEABLE)
}

// ---------------------------------------------------------------------------
// Host unit tests: `cargo test` (not part of the SBF binary).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use solana_system_interface::instruction as si;

    fn k(n: u8) -> Pubkey {
        Pubkey::new_from_array([n; 32])
    }

    #[test]
    fn system_create_account_matches_library() {
        assert_eq!(
            sys_create_account(&k(1), &k(2), 1_726_080, 120, &k(3)),
            si::create_account(&k(1), &k(2), 1_726_080, 120, &k(3))
        );
    }

    #[test]
    fn system_transfer_matches_library() {
        assert_eq!(sys_transfer(&k(1), &k(2), 459_360), si::transfer(&k(1), &k(2), 459_360));
    }

    #[test]
    fn system_allocate_matches_library() {
        assert_eq!(sys_allocate(&k(4), 120), si::allocate(&k(4), 120));
    }

    #[test]
    fn system_assign_matches_library() {
        assert_eq!(sys_assign(&k(4), &k(5)), si::assign(&k(4), &k(5)));
    }

    #[test]
    fn system_program_id_is_all_zero() {
        assert_eq!(SYSTEM_PROGRAM_ID, solana_system_interface::program::ID);
    }

    #[test]
    fn loader_id_decodes() {
        assert_eq!(BPF_LOADER_UPGRADEABLE.to_string(), "BPFLoaderUpgradeab1e11111111111111111111111");
    }

    // ---- §3.3 parse_header ----------------------------------------------------

    const PID: Pubkey = Pubkey::new_from_array([9u8; 32]);

    fn valid_header(payload_len: usize) -> Vec<u8> {
        let mut d = vec![0u8; HEADER_LEN + payload_len];
        d[OFF_VERSION] = VERSION;
        d[OFF_FLAGS] = MODE_OWNER;
        d[OFF_PAYLOAD_LEN..OFF_PAYLOAD_LEN + 2].copy_from_slice(&(payload_len as u16).to_le_bytes());
        d[OFF_NAMESPACE..OFF_NAMESPACE + 32].copy_from_slice(k(7).as_ref());
        d[OFF_OWNER..OFF_OWNER + 32].copy_from_slice(k(8).as_ref());
        d[OFF_AUTHORITY_BUMP] = 255;
        d
    }

    fn parse(d: &[u8]) -> Result<Header, MassAssetError> {
        parse_header(&PID, &PID, d)
    }

    #[test]
    fn v_valid_header_parses() {
        let h = parse(&valid_header(48)).expect("valid");
        assert_eq!(h.payload_len, 48);
        assert_eq!(h.transfer_mode, MODE_OWNER);
        assert_eq!(h.namespace, k(7));
        assert_eq!(h.owner, k(8));
        assert_eq!(h.authority_bump, 255);
        assert!(parse(&valid_header(0)).is_ok());
        assert!(parse(&valid_header(MAX_PAYLOAD)).is_ok());
    }

    #[test]
    fn v1_not_mass_asset() {
        assert_eq!(parse_header(&k(1), &PID, &valid_header(0)).unwrap_err(), MassAssetError::NotMassAsset);
    }

    #[test]
    fn v2_short_account() {
        assert_eq!(parse(&valid_header(0)[..HEADER_LEN - 1]).unwrap_err(), MassAssetError::InvalidAccountSize);
        assert_eq!(parse(&[]).unwrap_err(), MassAssetError::InvalidAccountSize);
    }

    #[test]
    fn v3_version() {
        for v in [0u8, 2, 255] {
            let mut d = valid_header(0);
            d[OFF_VERSION] = v;
            assert_eq!(parse(&d).unwrap_err(), MassAssetError::UnsupportedVersion);
        }
    }

    #[test]
    fn v4_flags() {
        let mut d = valid_header(0);
        d[OFF_FLAGS] = 3;
        assert_eq!(parse(&d).unwrap_err(), MassAssetError::InvalidFlags);
        for bit in 2..8 {
            let mut d = valid_header(0);
            d[OFF_FLAGS] = 1 << bit;
            assert_eq!(parse(&d).unwrap_err(), MassAssetError::InvalidFlags);
        }
        for mode in [MODE_OWNER, MODE_NAMESPACE, MODE_FROZEN] {
            let mut d = valid_header(0);
            d[OFF_FLAGS] = mode;
            assert!(parse(&d).is_ok());
        }
    }

    #[test]
    fn v5_reserved_bytes() {
        for i in OFF_RESERVED..HEADER_LEN {
            let mut d = valid_header(0);
            d[i] = 1;
            assert_eq!(parse(&d).unwrap_err(), MassAssetError::InvalidHeader);
        }
    }

    #[test]
    fn v6_payload_len() {
        let mut d = valid_header(48);
        d.push(0); // account longer than declared
        assert_eq!(parse(&d).unwrap_err(), MassAssetError::InvalidAccountSize);
        let mut d = valid_header(48);
        d.pop(); // account shorter than declared
        assert_eq!(parse(&d).unwrap_err(), MassAssetError::InvalidAccountSize);
        let mut d = valid_header(MAX_PAYLOAD + 1); // over the cap even if sizes agree
        d[OFF_PAYLOAD_LEN..OFF_PAYLOAD_LEN + 2].copy_from_slice(&((MAX_PAYLOAD + 1) as u16).to_le_bytes());
        assert_eq!(parse(&d).unwrap_err(), MassAssetError::InvalidAccountSize);
    }

    #[test]
    fn v7_default_owner() {
        let mut d = valid_header(0);
        d[OFF_OWNER..OFF_OWNER + 32].fill(0);
        assert_eq!(parse(&d).unwrap_err(), MassAssetError::InvalidOwner);
    }

    #[test]
    fn v_check_order() {
        // Several defects at once: the earliest row wins.
        let mut d = valid_header(0);
        d[OFF_VERSION] = 2; // V3
        d[OFF_FLAGS] = 0xFF; // V4
        d[OFF_RESERVED] = 1; // V5
        assert_eq!(parse(&d).unwrap_err(), MassAssetError::UnsupportedVersion);
        d[OFF_VERSION] = VERSION;
        assert_eq!(parse(&d).unwrap_err(), MassAssetError::InvalidFlags);
        d[OFF_FLAGS] = 0;
        assert_eq!(parse(&d).unwrap_err(), MassAssetError::InvalidHeader);
        assert_eq!(parse_header(&k(1), &PID, &d).unwrap_err(), MassAssetError::NotMassAsset);
    }

    // ---- §3.4 check_authority -------------------------------------------------

    fn header_for(ns: Pubkey, bump: u8) -> Header {
        Header { transfer_mode: MODE_OWNER, payload_len: 0, namespace: ns, owner: k(8), authority_bump: bump }
    }

    #[test]
    fn a_authority_canonical_ok() {
        let ns = k(7);
        let (auth, bump) = authority_of(&ns);
        let h = header_for(ns, bump);
        assert_eq!(authority_key(&h), Some(auth));
        assert!(check_authority(true, &auth, &h).is_ok());
    }

    #[test]
    fn a1_not_signer() {
        let ns = k(7);
        let (auth, bump) = authority_of(&ns);
        assert_eq!(check_authority(false, &auth, &header_for(ns, bump)).unwrap_err(), MassAssetError::InvalidAuthority);
    }

    #[test]
    fn a2_wrong_key_or_bump() {
        let ns = k(7);
        let (auth, bump) = authority_of(&ns);
        let h = header_for(ns, bump);
        assert_eq!(check_authority(true, &k(1), &h).unwrap_err(), MassAssetError::InvalidAuthority);
        // Another issuer's authority does not match this namespace.
        let (other, _) = authority_of(&k(6));
        assert_eq!(check_authority(true, &other, &h).unwrap_err(), MassAssetError::InvalidAuthority);
        // A different stored bump never yields the canonical authority.
        for b in 0..=255u8 {
            if b == bump { continue; }
            let hb = header_for(ns, b);
            assert_ne!(authority_key(&hb), Some(auth));
            assert_eq!(check_authority(true, &auth, &hb).unwrap_err(), MassAssetError::InvalidAuthority);
        }
    }

    #[test]
    fn header_is_payload_aligned() {
        assert_eq!(HEADER_LEN % 8, 0);
        assert_eq!(OFF_RESERVED + 3, HEADER_LEN);
    }
}
