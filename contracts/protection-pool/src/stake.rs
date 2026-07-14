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

use soroban_sdk::{contractevent, token::TokenClient, xdr::ToXdr, Address, Env};

use crate::storage;
use crate::types::{
    StakeRecord, LEDGERS_PER_DAY, MAX_STAKE_BPS, MIN_STAKE_BPS, STAKE_BPS_DENOMINATOR,
};

// -----------------------------------------------------------------------
// Events — soroban-sdk 27's #[contractevent] pattern (verified against
// docs.rs/developers.stellar.org 2026-07-14), not the deprecated raw
// env.events().publish() call.
// -----------------------------------------------------------------------

#[contractevent]
pub struct Staked {
    #[topic]
    pub staker: Address,
    pub amount: i128,
}

#[contractevent]
pub struct Withdrawn {
    #[topic]
    pub staker: Address,
    pub amount: i128,
}

#[contractevent]
pub struct EmergencyExit {
    #[topic]
    pub staker: Address,
    pub amount: i128,
}

/// V8 `_computePoints`: tenure-banded (100/120/150/200 pts/day at
/// 0-90/91-180/181-365/365+ days) × stake/max_stake. V8 divides by the
/// FIXED STAKE_MAX; since Tranche 1 made stake bounds dynamic (bps of
/// pool cap), this divides by the CURRENT max_stake(pool_cap) instead —
/// flagged in the KB as a linkage decision, not silently assumed. Returns
/// 0 if the stake is empty or already withdrawn (matches V8 exactly).
pub fn compute_points(env: &Env, wallet: &Address) -> i128 {
    match storage::get_stake(env, wallet) {
        Some(r) => compute_points_for_record(env, &r),
        None => 0,
    }
}

/// Same formula as `compute_points`, taking a record directly rather than
/// re-fetching from storage. Used by claim.rs's forfeiture path, which
/// needs to compute points from an in-memory (not-yet-persisted) record —
/// re-fetching would rely on storage still coincidentally matching local
/// state rather than being an explicit invariant.
pub fn compute_points_for_record(env: &Env, record: &StakeRecord) -> i128 {
    if record.amount <= 0 || record.withdrawn {
        return 0;
    }

    let now = env.ledger().sequence();
    let d = (now.saturating_sub(record.staked_at_ledger) / LEDGERS_PER_DAY) as i128;
    let d1 = d.min(90);
    let d2 = if d > 90 { d.min(180) - 90 } else { 0 };
    let d3 = if d > 180 { d.min(365) - 180 } else { 0 };
    let d4 = if d > 365 { d - 365 } else { 0 };
    let base = d1 * 100 + d2 * 120 + d3 * 150 + d4 * 200;

    let denom = max_stake(env);
    if denom <= 0 {
        return 0;
    }
    base * record.amount / denom
}

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
    storage::require_not_paused(env);
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
        staked_at_timestamp: env.ledger().timestamp(),
        penalty_locked_until_ledger: 0,
        withdrawn: false,
        suspended: false,
        claim_active: false,
    };
    storage::set_stake(env, staker, &record);
    storage::set_total_staked(env, total_staked + amount);
    storage::set_total_stakers(env, storage::get_total_stakers(env) + 1);
    storage::bump_instance_ttl(env);

    Staked { staker: staker.clone(), amount }.publish(env);

    // Interaction — SAC transfer, staker to contract.
    let token = TokenClient::new(env, &xlm_token_address(env));
    token.transfer(staker, &env.current_contract_address(), &amount);
}

/// V8 `setBeneficiary`: update the payout-receiving address before
/// forfeiture. Locked once withdrawn. Missing from the first two build
/// passes — found on full source read.
pub fn set_beneficiary(env: &Env, staker: &Address, new_beneficiary: &Address) {
    storage::require_not_paused(env);
    staker.require_auth();

    if new_beneficiary == staker {
        panic!("SAFU: beneficiary cannot be staker");
    }
    if new_beneficiary == &storage::get_oracle(env) {
        panic!("SAFU: beneficiary cannot be oracle");
    }
    if new_beneficiary == &storage::get_admin(env) {
        panic!("SAFU: beneficiary cannot be admin");
    }
    if new_beneficiary == &storage::get_co_signer(env) {
        panic!("SAFU: beneficiary cannot be coSigner");
    }

    let mut record = storage::get_stake(env, staker).expect("SAFU: no stake");
    if record.amount <= 0 {
        panic!("SAFU: no stake");
    }
    if record.withdrawn {
        panic!("SAFU: stake forfeited");
    }

    record.beneficiary_hash = env.crypto().sha256(&new_beneficiary.to_xdr(env)).to_bytes();
    storage::set_stake(env, staker, &record);
}

pub fn withdraw(env: &Env, staker: &Address, beneficiary: &Address) {
    storage::require_not_paused(env);
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

    // V8: finalPoints computed BEFORE withdrawn=true (compute_points
    // returns 0 once withdrawn). Missing from the first two build passes.
    let final_points = compute_points(env, staker);
    let amount = record.amount;

    // Effects before interaction (CEI).
    record.withdrawn = true;
    record.amount = 0;
    storage::set_stake(env, staker, &record);

    if final_points > 0 {
        let banked = storage::get_points_balance(env, staker);
        storage::set_points_balance(env, staker, banked + final_points);
    }

    let total_staked = storage::get_total_staked(env);
    storage::set_total_staked(env, total_staked - amount);
    storage::set_total_stakers(env, storage::get_total_stakers(env).saturating_sub(1));
    storage::bump_instance_ttl(env);

    Withdrawn { staker: staker.clone(), amount }.publish(env);

    // Interaction — SAC transfer, contract to beneficiary.
    let token = TokenClient::new(env, &xlm_token_address(env));
    token.transfer(&env.current_contract_address(), beneficiary, &amount);
}

/// V8 `emergencyExit`: staker self-rescue during pause, no time lock.
/// V8's version returns wstETH directly (skips Lido/Curve unwind); since
/// Tranche 1 holds native XLM with no yield-deployment layer, there's
/// nothing special to unwind — this is functionally identical to
/// withdraw() except it works WHILE paused (withdraw() is blocked then)
/// and skips the beneficiary-hash check (V8's emergencyExit sends
/// directly to msg.sender, not a separate beneficiary — same here).
pub fn emergency_exit(env: &Env, staker: &Address) {
    staker.require_auth();

    let mut record = storage::get_stake(env, staker).expect("SAFU: no stake");
    if record.amount <= 0 || record.withdrawn {
        panic!("SAFU: no active stake");
    }
    if record.claim_active {
        panic!("SAFU: claim active");
    }

    let amount = record.amount;
    record.withdrawn = true;
    record.amount = 0;
    storage::set_stake(env, staker, &record);

    let total_staked = storage::get_total_staked(env);
    storage::set_total_staked(env, total_staked - amount);
    storage::set_total_stakers(env, storage::get_total_stakers(env).saturating_sub(1));
    storage::bump_instance_ttl(env);

    EmergencyExit { staker: staker.clone(), amount }.publish(env);

    let token = TokenClient::new(env, &xlm_token_address(env));
    token.transfer(&env.current_contract_address(), staker, &amount);
}
