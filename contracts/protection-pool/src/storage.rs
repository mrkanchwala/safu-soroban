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
    /// Staker COUNT, separate from TotalStaked (the amount). V8: totalStakers.
    /// Missing from the first two build passes — found on full source read.
    TotalStakers,
    Paused,
    DailyOutflow,
    LastOutflowDay,
    /// Admission-side daily cap tracking (submit_claim) — separate from the
    /// payout-side DailyOutflow/LastOutflowDay (claim_stream) above. V8 uses
    /// three parallel instance vars (dailyEntitlementTotal/lastEntitlementDay/
    /// dailyClaimCount) with the same day-rollover pattern, not a per-day
    /// keyed counter — corrected here after an earlier draft built the wrong
    /// mechanism (a temporary-storage ClaimAdmissionCount(u32) key that
    /// tracked count only, not the entitlement sum, and didn't match V8's
    /// reset semantics).
    DailyEntitlementTotal,
    LastEntitlementDay,
    DailyClaimCount,
    // -- persistent (per-entity) --
    Stake(Address),
    ClaimRec(BytesN<32>),
    Override(BytesN<32>),
    /// Banked points after stake exit — accumulates across all cycles. V8:
    /// pointsBalance. Missing from the first two build passes.
    PointsBalance(Address),
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

pub fn get_total_stakers(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::TotalStakers)
        .unwrap_or(0)
}

pub fn set_total_stakers(env: &Env, value: u32) {
    env.storage().instance().set(&DataKey::TotalStakers, &value);
}

pub fn is_paused(env: &Env) -> bool {
    env.storage().instance().get(&DataKey::Paused).unwrap_or(false)
}

pub fn set_paused(env: &Env, value: bool) {
    env.storage().instance().set(&DataKey::Paused, &value);
}

pub fn require_not_paused(env: &Env) {
    if is_paused(env) {
        panic!("SAFU: paused");
    }
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

/// Admission-side daily cap (submit_claim's dailyEntitlementTotal +
/// dailyClaimCount) — same day-rollover pattern as daily_outflow above,
/// but tracked separately since V8 keeps these as distinct counters.
/// Returns (entitlement_total, claim_count) for `current_day`, rolled to
/// (0, 0) if the stored day doesn't match.
pub fn get_daily_entitlement(env: &Env, current_day: u32) -> (i128, u32) {
    let last_day: u32 = env
        .storage()
        .instance()
        .get(&DataKey::LastEntitlementDay)
        .unwrap_or(0);
    if last_day != current_day {
        (0, 0)
    } else {
        let total = env
            .storage()
            .instance()
            .get(&DataKey::DailyEntitlementTotal)
            .unwrap_or(0);
        let count = env
            .storage()
            .instance()
            .get(&DataKey::DailyClaimCount)
            .unwrap_or(0);
        (total, count)
    }
}

pub fn set_daily_entitlement(env: &Env, current_day: u32, total: i128, count: u32) {
    env.storage()
        .instance()
        .set(&DataKey::LastEntitlementDay, &current_day);
    env.storage()
        .instance()
        .set(&DataKey::DailyEntitlementTotal, &total);
    env.storage()
        .instance()
        .set(&DataKey::DailyClaimCount, &count);
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
// Points balance (persistent) — banked at forfeiture time
// -----------------------------------------------------------------------

pub fn get_points_balance(env: &Env, wallet: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::PointsBalance(wallet.clone()))
        .unwrap_or(0)
}

pub fn set_points_balance(env: &Env, wallet: &Address, value: i128) {
    let key = DataKey::PointsBalance(wallet.clone());
    env.storage().persistent().set(&key, &value);
    env.storage()
        .persistent()
        .extend_ttl(&key, BUMP_THRESHOLD, BUMP_TO);
}
