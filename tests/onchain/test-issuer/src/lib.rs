//! Disposable test issuer (namespace) for the Mass Asset v1 on-chain conformance suite.
//!
//! It is a signing proxy: it forwards the instruction data unchanged to the program
//! passed as account 0, passes accounts 1.. through in order, and signs as its own
//! Mass Asset authority PDA (["mass-asset-authority"]).
//! This lets the client exercise every Mass Asset instruction "as the issuer",
//! including deliberately invalid ones (negative tests).
//!
//! WARNING: this is exactly the anti-pattern forbidden by spec §6 (E1) — it signs as
//! authority for ANY target program. Never use it as an issuer template.

use solana_program::{
    account_info::AccountInfo,
    entrypoint,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
    program_error::ProgramError,
    pubkey::Pubkey,
};

entrypoint!(process_instruction);

const AUTHORITY_SEED: &[u8] = b"mass-asset-authority";

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    let (target, rest) = accounts
        .split_first()
        .ok_or(ProgramError::NotEnoughAccountKeys)?;

    let (authority, bump) = Pubkey::find_program_address(&[AUTHORITY_SEED], program_id);

    let metas: Vec<AccountMeta> = rest
        .iter()
        .map(|a| AccountMeta {
            pubkey: *a.key,
            is_signer: a.is_signer || *a.key == authority,
            is_writable: a.is_writable,
        })
        .collect();

    let ix = Instruction {
        program_id: *target.key,
        accounts: metas,
        data: instruction_data.to_vec(),
    };

    let bump_seed = [bump];
    invoke_signed(&ix, accounts, &[&[AUTHORITY_SEED, &bump_seed]])
}
