//! SAFU ProtectionPool — Soroban port of `SAFUPoolV8.sol`, Tranche 1 (MVP)
//! scope only: staking, points/tier/claim mechanics, payout streaming,
//! on-chain solvency invariant. Yield-deployment (wstETH/Lido) is
//! deliberately excluded — see context/knowledge/smartcontract-soroban.md
//! in the research-ops repo for the full mechanics map and the eng review
//! that locked this scope boundary (2026-07-14).
//!
//! See README.md at the repo root for build/test instructions, the full
//! storage model, mechanics ported from V8, deliberate deviations, and
//! current known-open items — kept there so it's visible to anyone
//! reading this repo directly, not just this doc comment.

#![no_std]
#[cfg(test)]
extern crate std;

mod admin;
mod claim;
mod stake;
mod storage;
#[cfg(test)]
mod test;
mod types;

use soroban_sdk::{contract, contractimpl, Address, BytesN, Env};

use crate::types::{Claim, StakeRecord};

#[contract]
pub struct ProtectionPool;

#[contractimpl]
impl ProtectionPool {
    /// One-time initialization. Guarded against re-invocation (vuln
    /// checklist V6) — see admin.rs.
    pub fn initialize(
        env: Env,
        admin: Address,
        oracle: Address,
        co_signer: Address,
        xlm_token: Address,
        pool_cap: i128,
    ) {
        admin::initialize(&env, &admin, &oracle, &co_signer, &xlm_token, pool_cap);
    }

    pub fn set_oracle(env: Env, new_oracle: Address) {
        admin::set_oracle(&env, &new_oracle);
    }

    pub fn set_co_signer(env: Env, new_co_signer: Address) {
        admin::set_co_signer(&env, &new_co_signer);
    }

    pub fn set_pool_cap(env: Env, new_cap: i128) {
        admin::set_pool_cap(&env, new_cap);
    }

    pub fn transfer_admin(env: Env, new_admin: Address) {
        admin::transfer_admin(&env, &new_admin);
    }

    pub fn pause(env: Env) {
        admin::pause(&env);
    }

    pub fn unpause(env: Env) {
        admin::unpause(&env);
    }

    pub fn suspend_stake(env: Env, wallet: Address) {
        admin::suspend_stake(&env, &wallet);
    }

    /// `claim_id` optional — pass the wallet's in-flight claim (if any) so
    /// its Rule A/B deadline clock resets on unsuspend (eng review
    /// blocker #1). `None` if the wallet has no claim needing a reset.
    pub fn unsuspend_stake(env: Env, wallet: Address, claim_id: Option<BytesN<32>>) {
        admin::unsuspend_stake(&env, &wallet, claim_id);
    }

    // -- stake / withdraw --

    pub fn stake(env: Env, staker: Address, amount: i128, beneficiary: Address) {
        stake::stake(&env, &staker, amount, &beneficiary);
    }

    pub fn withdraw(env: Env, staker: Address, beneficiary: Address) {
        stake::withdraw(&env, &staker, &beneficiary);
    }

    pub fn set_beneficiary(env: Env, staker: Address, new_beneficiary: Address) {
        stake::set_beneficiary(&env, &staker, &new_beneficiary);
    }

    pub fn emergency_exit(env: Env, staker: Address) {
        stake::emergency_exit(&env, &staker);
    }

    // -- claims --

    #[allow(clippy::too_many_arguments)]
    pub fn submit_claim(
        env: Env,
        caller: Address,
        wallet: Address,
        tx_hash: BytesN<32>,
        entitlement: i128,
        tier: u32,
        hack_timestamp: u64,
    ) -> BytesN<32> {
        claim::submit_claim(&env, &caller, &wallet, &tx_hash, entitlement, tier, hack_timestamp)
    }

    pub fn unlock_pending_claim(env: Env, claim_id: BytesN<32>) {
        claim::unlock_pending_claim(&env, &claim_id);
    }

    /// NEW 2026-07-22 (Rule A) — staker-authorized. Burns the wallet's
    /// entire lifetime points balance, forfeits the stake, starts
    /// cooldown/vesting. Must be called within `APPROVE_WINDOW_LEDGERS` of
    /// the claim entering `AwaitingApproval`.
    pub fn approve_claim(env: Env, claim_id: BytesN<32>) {
        claim::approve_claim(&env, &claim_id);
    }

    /// NEW 2026-07-22 (Rule A sweep) — permissionless, mirrors
    /// `unlock_pending_claim`. Releases the reservation back to the pool
    /// if the staker never approved within the window.
    pub fn expire_pending_approval(env: Env, claim_id: BytesN<32>) {
        claim::expire_pending_approval(&env, &claim_id);
    }

    /// NEW 2026-07-22 (Rule B sweep) — permissionless. Releases whatever's
    /// left uncollected if the staker goes `COLLECTION_INACTIVITY_LEDGERS`
    /// with zero `claim_stream` activity.
    pub fn expire_stale_claim(env: Env, claim_id: BytesN<32>) {
        claim::expire_stale_claim(&env, &claim_id);
    }

    pub fn claim_stream(env: Env, claim_id: BytesN<32>, beneficiary: Address) -> i128 {
        claim::claim_stream(&env, &claim_id, &beneficiary)
    }

    pub fn cancel_claim(env: Env, claim_id: BytesN<32>) {
        claim::cancel_claim(&env, &claim_id);
    }

    pub fn approve_override(
        env: Env,
        caller: Address,
        wallet: Address,
        tx_hash: BytesN<32>,
        entitlement: i128,
        tier: u32,
    ) {
        claim::approve_override(&env, &caller, &wallet, &tx_hash, entitlement, tier);
    }

    pub fn cancel_pending_override(env: Env, caller: Address, wallet: Address, tx_hash: BytesN<32>) {
        claim::cancel_pending_override(&env, &caller, &wallet, &tx_hash);
    }

    // -- views --
    // V8 parity gap closed 2026-07-14, corrected same day: the original
    // "closed" pass only ported raw storage getters (stakeOf-equivalent),
    // missing that V8's isEligible/pointsOf/isClaimEligible are COMPUTED
    // views, not storage reads — found via the same full-source-read that
    // caught the set_co_signer gap. pointsOf in particular returns LIVE
    // computed points for a still-active staker, not the banked balance
    // (which is only meaningful post-withdrawal) — get_points_balance
    // below was returning the wrong number for anyone still staked until
    // this fix.

    pub fn get_stake(env: Env, wallet: Address) -> Option<StakeRecord> {
        storage::get_stake(&env, &wallet)
    }

    pub fn get_claim(env: Env, claim_id: BytesN<32>) -> Option<Claim> {
        storage::get_claim(&env, &claim_id)
    }

    /// V8 `pointsOf`: live-computed if still staked and not withdrawn,
    /// else the banked balance. NOT a raw storage read.
    pub fn get_points_balance(env: Env, wallet: Address) -> i128 {
        if let Some(record) = storage::get_stake(&env, &wallet) {
            if record.amount > 0 && !record.withdrawn {
                return stake::compute_points_for_record(&env, &record);
            }
        }
        storage::get_points_balance(&env, &wallet)
    }

    /// V8 `isEligible`: has an active, non-withdrawn, non-suspended
    /// stake. Does NOT check the time gate — see is_claim_eligible.
    pub fn is_eligible(env: Env, wallet: Address) -> bool {
        match storage::get_stake(&env, &wallet) {
            Some(r) => r.amount > 0 && !r.withdrawn && !r.suspended,
            None => false,
        }
    }

    /// V8 `isClaimEligible`: is_eligible AND the 90-day time gate has
    /// passed.
    pub fn is_claim_eligible(env: Env, wallet: Address) -> bool {
        match storage::get_stake(&env, &wallet) {
            Some(r) => {
                r.amount > 0
                    && !r.withdrawn
                    && !r.suspended
                    && env.ledger().sequence().saturating_sub(r.staked_at_ledger)
                        >= crate::types::TIME_GATE_LEDGERS
            }
            None => false,
        }
    }

    pub fn get_total_staked(env: Env) -> i128 {
        storage::get_total_staked(&env)
    }

    pub fn get_total_allocated(env: Env) -> i128 {
        storage::get_total_allocated(&env)
    }

    pub fn get_total_stakers(env: Env) -> u32 {
        storage::get_total_stakers(&env)
    }

    pub fn is_paused(env: Env) -> bool {
        storage::is_paused(&env)
    }
}
