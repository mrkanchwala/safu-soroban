//! Stake + withdraw principal. Ported from `SAFUPoolV8.sol`'s `stakeETH`/
//! `withdraw`, minus the Lido wstETH wrap/unwrap (excluded — Tranche 1 has
//! no yield venue). Verified against source 2026-07-14 (V8's actual
//! function is `stakeETH`, not `stake` — one staker = one active
//! StakeRecord, a second stake requires withdrawing the first: V8 line
//! `require(stakes[msg.sender].amount == 0, "already staked")`).
//!
//! Stake bounds corrected 2026-07-14 (user): dynamic, as basis points of
//! the configurable pool cap (types.rs MIN_STAKE_BPS/MAX_STAKE_BPS), not
//! fixed amounts — portable across future EVM/Solana/BNB relaunches with
//! different pool sizes. Tranche 1 deploy default: 600,000 XLM pool cap
//! (admin.rs::TRANCHE1_DEPLOY_POOL_CAP_STROOPS), approximating V8's 60 ETH.
//!
//! Beneficiary handling corrected 2026-07-14 (user flagged the first draft
//! didn't match V8): takes the plaintext beneficiary `Address`, not a
//! pre-computed hash — the contract hashes it itself (sha256, not
//! keccak256 — no cross-chain hash-matching requirement, see
//! types.rs::StakeRecord doc) and validates identity, matching V8's
//! `stakeETH` checks exactly.

use soroban_sdk::{token::TokenClient, xdr::ToXdr, Address, Env};

use crate::storage;
use crate::types::{StakeRecord, MAX_STAKE_BPS, MIN_STAKE_BPS, STAKE_BPS_DENOMINATOR};

/// XLM native asset SAC address, set once at `initialize` (network-specific,
/// differs testnet/mainnet — never hardcoded in contract logic).
fn xlm_token_address(env: &Env) -> Address {
    storage::get_xlm_token(env)
}

/// Dynamic bounds: `pool_cap × BPS / 10_000`. Recomputed live on every
/// call, so a pool-cap change via `set_pool_cap` takes effect immediately
/// with no separate re-anchoring step.
fn min_stake(env: &Env) -> i128 {
    storage::get_pool_cap(env) * MIN_STAKE_BPS / STAKE_BPS_DENOMINATOR
}

fn max_stake(env: &Env) -> i128 {
    storage::get_pool_cap(env) * MAX_STAKE_BPS / STAKE_BPS_DENOMINATOR
}

pub fn stake(env: &Env, staker: &Address, amount: i128, beneficiary: &Address) {
    staker.require_auth();

    if amount <= 0 {
        panic!("SAFU: stake must be positive");
    }
    // V8: require(msg.value >= STAKE_MIN && msg.value <= STAKE_MAX, "stake out of range")
    // — here, dynamic bounds as bps of pool_cap instead of fixed amounts.
    if amount < min_stake(env) || amount > max_stake(env) {
        panic!("SAFU: stake out of range");
    }

    // V8: require(stakes[msg.sender].amount == 0, "already staked")
    // One active StakeRecord per address — must withdraw before re-staking.
    if let Some(existing) = storage::get_stake(env, staker) {
        if existing.amount > 0 && !existing.withdrawn {
            panic!("SAFU: already staked");
        }
    }

    let total_staked = storage::get_total_staked(env);
    let pool_cap = storage::get_pool_cap(env);
    // V8: totalStaked + msg.value <= MAX_POOL_ETH && <= maxPoolSize
    if total_staked + amount > pool_cap {
        panic!("SAFU: pool cap exceeded");
    }

    // V8 identity checks — beneficiary cannot be the staker, oracle,
    // admin, or coSigner. (V8 also checks beneficiary != address(0);
    // Soroban's Address has no equivalent zero sentinel, so that specific
    // check is dropped, not silently reinterpreted as something else.)
    if beneficiary == staker {
        panic!("SAFU: beneficiary cannot be staker");
    }
    if beneficiary == &storage::get_oracle(env) {
        panic!("SAFU: beneficiary cannot be oracle");
    }
    if beneficiary == &storage::get_admin(env) {
        panic!("SAFU: beneficiary cannot be admin");
    }
    if beneficiary == &storage::get_co_signer(env) {
        panic!("SAFU: beneficiary cannot be coSigner");
    }

    // Verified 2026-07-14 via `cargo check` against soroban-sdk 27.0.0.
    let beneficiary_hash = env.crypto().sha256(&beneficiary.to_xdr(env)).to_bytes();

    // Effects before interaction (CEI).
    let record = StakeRecord {
        beneficiary_hash,
        amount,
        staked_at_ledger: env.ledger().sequence(),
        penalty_locked_until_ledger: 0,
        withdrawn: false,
        suspended: false,
        claim_active: false,
    };
    storage::set_stake(env, staker, &record);
    storage::set_total_staked(env, total_staked + amount);
    storage::bump_instance_ttl(env);

    // Interaction — SAC transfer, staker to contract.
    let token = TokenClient::new(env, &xlm_token_address(env));
    token.transfer(staker, &env.current_contract_address(), &amount);
}

pub fn withdraw(env: &Env, staker: &Address, beneficiary: &Address) {
    staker.require_auth();

    let mut record = storage::get_stake(env, staker).expect("SAFU: no stake");

    if record.amount <= 0 {
        panic!("SAFU: no stake");
    }
    if record.withdrawn {
        panic!("SAFU: already withdrawn");
    }
    if record.claim_active {
        panic!("SAFU: claim active");
    }
    let now = env.ledger().sequence();
    if now < record.penalty_locked_until_ledger {
        panic!("SAFU: penalty lock active");
    }

    let expected_hash = env.crypto().sha256(&beneficiary.to_xdr(env)).to_bytes();
    if expected_hash != record.beneficiary_hash {
        panic!("SAFU: wrong beneficiary");
    }

    let amount = record.amount;

    // Effects before interaction (CEI).
    record.withdrawn = true;
    record.amount = 0;
    storage::set_stake(env, staker, &record);

    let total_staked = storage::get_total_staked(env);
    storage::set_total_staked(env, total_staked - amount);
    storage::bump_instance_ttl(env);

    // Interaction — SAC transfer, contract to beneficiary.
    let token = TokenClient::new(env, &xlm_token_address(env));
    token.transfer(&env.current_contract_address(), beneficiary, &amount);
}
