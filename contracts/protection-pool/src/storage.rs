//! Storage layout + TTL bump helpers.
//!
//! Placement rules (context/knowledge/smartcontract-soroban.md §4, vuln
//! checklist V4/V11):
//! - `instance()`: pool-wide globals only (admin, oracle, co_signer,
//!   total_staked, total_allocated, daily_outflow/last_outflow_day). Never
//!   per-user or unbounded data here — every call loads all of instance.
//! - `persistent()`: per-staker and per-claim records, keyed by Address /
//!   claim id. Distributed across separate keys, not one growing struct.
//! - `temporary()`: daily claim-admission counters — naturally expires,
//!   nothing load-bearing for solvency lives here.
//!
//! TTL is never a security mechanism (V11) — the 90-day time gate and the
//! 365-day penalty lock are both enforced by comparing an explicit stored
//! ledger-sequence deadline in contract logic, never by relying on
//! storage-entry expiry. Bump TTLs generously so state never silently
//! archives out from under an active staker/claim.

use soroban_sdk::{contracttype, Address, BytesN, Env};

use crate::types::{Claim, OverrideRequest, StakeRecord};

const BUMP_THRESHOLD: u32 = 30 * crate::types::LEDGERS_PER_DAY;
const BUMP_TO: u32 = 120 * crate::types::LEDGERS_PER_DAY;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    // -- instance (pool-wide globals) --
    Admin,
    Oracle,
    CoSigner,
    XlmToken,
    /// Configurable pool cap (V8: `maxPoolSize`) — admin-adjustable up to
    /// whatever hard ceiling the deployment chooses. Per-staker min/max
    /// stake is computed live from this value × MIN_STAKE_BPS/MAX_STAKE_BPS
    /// (types.rs), so bounds never need re-anchoring when the cap changes.
    PoolCap,
    TotalStaked,
    TotalAllocated,
    DailyOutflow,
    LastOutflowDay,
    // -- persistent (per-entity) --
    Stake(Address),
    ClaimRec(BytesN<32>),
    Override(BytesN<32>),
    // -- temporary (daily counters) --
    ClaimAdmissionCount(u32), // keyed by ledger-day
}

// -----------------------------------------------------------------------
// Instance globals
// -----------------------------------------------------------------------

pub fn get_admin(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Admin).unwrap()
}

pub fn set_admin(env: &Env, admin: &Address) {
    env.storage().instance().set(&DataKey::Admin, admin);
}

pub fn get_oracle(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Oracle).unwrap()
}

pub fn set_oracle(env: &Env, oracle: &Address) {
    env.storage().instance().set(&DataKey::Oracle, oracle);
}

pub fn get_co_signer(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::CoSigner).unwrap()
}

pub fn set_co_signer(env: &Env, co_signer: &Address) {
    env.storage().instance().set(&DataKey::CoSigner, co_signer);
}

/// Network-specific XLM SAC address, set once at `initialize`. TODO: the
/// actual contract address differs between testnet/mainnet — pass in as
/// an `initialize` argument, never hardcode.
pub fn get_xlm_token(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::XlmToken).unwrap()
}

pub fn set_xlm_token(env: &Env, token: &Address) {
    env.storage().instance().set(&DataKey::XlmToken, token);
}

pub fn get_pool_cap(env: &Env) -> i128 {
    env.storage().instance().get(&DataKey::PoolCap).unwrap()
}

pub fn set_pool_cap(env: &Env, value: i128) {
    env.storage().instance().set(&DataKey::PoolCap, &value);
}

pub fn get_total_staked(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalStaked)
        .unwrap_or(0)
}

pub fn set_total_staked(env: &Env, value: i128) {
    env.storage().instance().set(&DataKey::TotalStaked, &value);
}

pub fn get_total_allocated(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalAllocated)
        .unwrap_or(0)
}

pub fn set_total_allocated(env: &Env, value: i128) {
    env.storage()
        .instance()
        .set(&DataKey::TotalAllocated, &value);
}

/// Returns (daily_outflow, last_outflow_day), rolling over to (0, today)
/// if the stored day doesn't match — mirrors V8's `claimStream` day-reset
/// check. Ported exactly per the eng review finding: this is a simple
/// first-come-first-served-per-day mechanism, not a queue — unclaimed
/// amounts just stay owed and carry forward to the next call.
pub fn get_daily_outflow(env: &Env, current_day: u32) -> i128 {
    let last_day: u32 = env
        .storage()
        .instance()
        .get(&DataKey::LastOutflowDay)
        .unwrap_or(0);
    if last_day != current_day {
        0
    } else {
        env.storage()
            .instance()
            .get(&DataKey::DailyOutflow)
            .unwrap_or(0)
    }
}

pub fn set_daily_outflow(env: &Env, current_day: u32, value: i128) {
    env.storage()
        .instance()
        .set(&DataKey::LastOutflowDay, &current_day);
    env.storage().instance().set(&DataKey::DailyOutflow, &value);
}

/// Bump the shared instance TTL — call at every entry point that touches
/// pool-wide globals so admin/config/totals never silently archive.
pub fn bump_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(BUMP_THRESHOLD, BUMP_TO);
}

// -----------------------------------------------------------------------
// Per-staker records (persistent)
// -----------------------------------------------------------------------

pub fn get_stake(env: &Env, staker: &Address) -> Option<StakeRecord> {
    env.storage().persistent().get(&DataKey::Stake(staker.clone()))
}

pub fn set_stake(env: &Env, staker: &Address, record: &StakeRecord) {
    let key = DataKey::Stake(staker.clone());
    env.storage().persistent().set(&key, record);
    env.storage()
        .persistent()
        .extend_ttl(&key, BUMP_THRESHOLD, BUMP_TO);
}

// -----------------------------------------------------------------------
// Per-claim records (persistent)
// -----------------------------------------------------------------------

pub fn get_claim(env: &Env, claim_id: &BytesN<32>) -> Option<Claim> {
    env.storage()
        .persistent()
        .get(&DataKey::ClaimRec(claim_id.clone()))
}

pub fn set_claim(env: &Env, claim_id: &BytesN<32>, claim: &Claim) {
    let key = DataKey::ClaimRec(claim_id.clone());
    env.storage().persistent().set(&key, claim);
    env.storage()
        .persistent()
        .extend_ttl(&key, BUMP_THRESHOLD, BUMP_TO);
}

// -----------------------------------------------------------------------
// Override requests (persistent) — 2-of-2 oracle+coSigner flow
// -----------------------------------------------------------------------

pub fn get_override(env: &Env, claim_id: &BytesN<32>) -> Option<OverrideRequest> {
    env.storage()
        .persistent()
        .get(&DataKey::Override(claim_id.clone()))
}

pub fn set_override(env: &Env, claim_id: &BytesN<32>, req: &OverrideRequest) {
    let key = DataKey::Override(claim_id.clone());
    env.storage().persistent().set(&key, req);
    env.storage()
        .persistent()
        .extend_ttl(&key, BUMP_THRESHOLD, BUMP_TO);
}

// -----------------------------------------------------------------------
// Daily claim-admission counter (temporary — naturally expires)
// -----------------------------------------------------------------------

pub fn get_claim_admission_count(env: &Env, day: u32) -> u32 {
    env.storage()
        .temporary()
        .get(&DataKey::ClaimAdmissionCount(day))
        .unwrap_or(0)
}

pub fn incr_claim_admission_count(env: &Env, day: u32) {
    let count = get_claim_admission_count(env, day) + 1;
    let key = DataKey::ClaimAdmissionCount(day);
    env.storage().temporary().set(&key, &count);
    // Short TTL — this counter only matters for the current + maybe next day.
    env.storage()
        .temporary()
        .extend_ttl(&key, crate::types::LEDGERS_PER_DAY, 2 * crate::types::LEDGERS_PER_DAY);
}
