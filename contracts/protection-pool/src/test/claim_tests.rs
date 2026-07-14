#![cfg(test)]

use soroban_sdk::testutils::Address as _;
use soroban_sdk::Address;

use super::common::*;
use crate::types::ClaimStatus;

const ENTITLEMENT: i128 = 1_000_000;
const TIER_C: u32 = 3;

// -----------------------------------------------------------------------
// submit_claim — happy paths
// -----------------------------------------------------------------------

#[test]
fn submit_claim_by_oracle_pending_when_gate_not_met() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.status, ClaimStatus::PendingTime);
    assert_eq!(claim.entitlement, ENTITLEMENT);
    // Stake not forfeited yet — total_staked unchanged.
    assert_eq!(s.client.get_total_staked(), MID_STAKE);
}

#[test]
fn submit_claim_activates_immediately_when_gate_met() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    advance_days(&env, 90);
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.status, ClaimStatus::Active);
    // Stake forfeited in the same call.
    assert_eq!(s.client.get_total_staked(), 0);
    assert_eq!(s.client.get_total_stakers(), 0);
}

#[test]
fn submit_claim_by_admin_also_works() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    s.client.submit_claim(
        &s.admin,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
}

#[test]
fn submit_claim_banks_points_on_immediate_activation() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    advance_days(&env, 90);
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    assert!(s.client.get_points_balance(&staker) > 0);
}

#[test]
fn submit_claim_reserves_total_allocated() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    assert_eq!(s.client.get_total_allocated(), ENTITLEMENT);
}

// -----------------------------------------------------------------------
// submit_claim — validation panics
// -----------------------------------------------------------------------

#[test]
#[should_panic(expected = "SAFU: caller must be oracle or admin")]
fn submit_claim_wrong_caller_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    let random = Address::generate(&env);
    s.client.submit_claim(
        &random,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
}

#[test]
#[should_panic(expected = "SAFU: entitlement must be positive")]
fn submit_claim_zero_entitlement_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &0,
        &TIER_C,
        &now_ts(&env),
    );
}

#[test]
#[should_panic(expected = "SAFU: invalid tier")]
fn submit_claim_invalid_tier_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &4,
        &now_ts(&env),
    );
}

#[test]
#[should_panic(expected = "SAFU: no stake")]
fn submit_claim_no_stake_panics() {
    let env = new_env();
    let s = setup(&env);
    let random = Address::generate(&env);
    s.client.submit_claim(
        &s.oracle,
        &random,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
}

#[test]
#[should_panic(expected = "SAFU: no active stake")]
fn submit_claim_after_withdraw_panics() {
    // Reaches "no active stake" (amount<=0), not "stake already
    // withdrawn" — same check-ordering note as elsewhere in this suite.
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    s.client.withdraw(&staker, &ben);
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
}

#[test]
#[should_panic(expected = "SAFU: claim already active for this stake")]
fn submit_claim_twice_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 2),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
}

#[test]
#[should_panic(expected = "SAFU: entitlement exceeds tier cap")]
fn submit_claim_exceeds_tier_cap_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    // Tier C cap = stake * 5 = 500_000_000; ask for more.
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &600_000_000,
        &TIER_C,
        &now_ts(&env),
    );
}

#[test]
#[should_panic(expected = "SAFU: hack timestamp in the future")]
fn submit_claim_future_hack_timestamp_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &(now_ts(&env) + 1),
    );
}

#[test]
#[should_panic(expected = "SAFU: hack predates stake")]
fn submit_claim_hack_before_stake_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &(now_ts(&env) - 1),
    );
}

#[test]
#[should_panic(expected = "SAFU: claim window expired")]
fn submit_claim_outside_claim_window_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    let hack_ts = now_ts(&env);
    advance_days(&env, 31); // > 30-day claim window
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &hack_ts,
    );
}

#[test]
fn submit_claim_at_exactly_claim_window_boundary_succeeds() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    let hack_ts = now_ts(&env);
    advance_days(&env, 30); // exactly at the boundary, still valid
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &hack_ts,
    );
}

#[test]
#[should_panic(expected = "SAFU: insolvent")]
fn submit_claim_insolvent_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    // Only MID_STAKE (100_000_000) actually backing the pool; ask for
    // more than that even though it's under the tier C cap (500_000_000).
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &(MID_STAKE + 1),
        &TIER_C,
        &now_ts(&env),
    );
}

#[test]
#[should_panic(expected = "SAFU: daily stress cap exceeded")]
fn submit_claim_exceeds_stress_cap_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    // stress_cap at 0% utilization = 25% of total_staked = 25_000_000.
    // Ask for more than that but still within solvency/tier bounds.
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &26_000_000,
        &TIER_C,
        &now_ts(&env),
    );
}

#[test]
#[should_panic(expected = "SAFU: oracle daily claim-count limit reached")]
fn submit_claim_oracle_rate_limit_panics() {
    let env = new_env();
    let s = setup(&env);
    // total_stakers/10 max(1) == 1 with a single staker — the SECOND
    // oracle-submitted claim same day must be rejected even though it's
    // a different wallet.
    let (staker1, _b1) = staked_wallet(&env, &s);
    let (staker2, _b2) = staked_wallet(&env, &s);
    s.client.submit_claim(
        &s.oracle,
        &staker1,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    s.client.submit_claim(
        &s.oracle,
        &staker2,
        &tx_hash(&env, 2),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
}

#[test]
fn submit_claim_admin_not_subject_to_oracle_rate_limit() {
    let env = new_env();
    let s = setup(&env);
    let (staker1, _b1) = staked_wallet(&env, &s);
    let (staker2, _b2) = staked_wallet(&env, &s);
    s.client.submit_claim(
        &s.oracle,
        &staker1,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    // Admin call for a second wallet, same day — not gated by the
    // oracle-only rate limit.
    s.client.submit_claim(
        &s.admin,
        &staker2,
        &tx_hash(&env, 2),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
}

#[test]
fn submit_claim_oracle_limit_resets_next_day() {
    let env = new_env();
    let s = setup(&env);
    let (staker1, _b1) = staked_wallet(&env, &s);
    let (staker2, _b2) = staked_wallet(&env, &s);
    s.client.submit_claim(
        &s.oracle,
        &staker1,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    advance_days(&env, 1);
    s.client.submit_claim(
        &s.oracle,
        &staker2,
        &tx_hash(&env, 2),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
}

#[test]
#[should_panic(expected = "SAFU: claim already exists")]
fn submit_claim_duplicate_wallet_tx_hash_after_cancel_panics() {
    // claim_active alone would block a same-wallet resubmit with "claim
    // already active for this stake" — that flag resets on cancel_claim,
    // so this test specifically isolates the SEPARATE claim-id-existence
    // guard: a cancelled claim's id is permanently retired, never reused.
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    let hash = tx_hash(&env, 1);
    let claim_id =
        s.client
            .submit_claim(&s.oracle, &staker, &hash, &ENTITLEMENT, &TIER_C, &now_ts(&env));
    s.client.cancel_claim(&claim_id);
    // Advance a day so the oracle's daily claim-count limit (unrelated to
    // what this test targets) doesn't fire first and mask the real
    // assertion.
    advance_days(&env, 1);
    s.client
        .submit_claim(&s.oracle, &staker, &hash, &ENTITLEMENT, &TIER_C, &now_ts(&env));
}

#[test]
#[should_panic(expected = "SAFU: paused")]
fn submit_claim_blocked_while_paused() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    s.client.pause();
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
}

// -----------------------------------------------------------------------
// unlock_pending_claim
// -----------------------------------------------------------------------

#[test]
fn unlock_pending_claim_activates_after_gate() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    advance_days(&env, 90);
    s.client.unlock_pending_claim(&claim_id);
    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.status, ClaimStatus::Active);
    assert_eq!(s.client.get_total_staked(), 0);
}

#[test]
#[should_panic(expected = "SAFU: time gate not yet met")]
fn unlock_pending_claim_before_gate_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    advance_days(&env, 89);
    s.client.unlock_pending_claim(&claim_id);
}

#[test]
#[should_panic(expected = "SAFU: claim not pending")]
fn unlock_pending_claim_already_active_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    advance_days(&env, 90);
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    s.client.unlock_pending_claim(&claim_id);
}

#[test]
#[should_panic(expected = "SAFU: no such claim")]
fn unlock_pending_claim_nonexistent_panics() {
    let env = new_env();
    let s = setup(&env);
    s.client.unlock_pending_claim(&tx_hash(&env, 99));
}

// -----------------------------------------------------------------------
// claim_stream
// -----------------------------------------------------------------------

fn active_claim_with_entitlement(
    env: &soroban_sdk::Env,
    s: &Setup<'_>,
    entitlement: i128,
) -> (Address, Address, soroban_sdk::BytesN<32>) {
    let (staker, ben) = staked_wallet(env, s);
    advance_days(env, 90); // gate met, activates immediately on submit
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(env, 1),
        &entitlement,
        &TIER_C,
        &now_ts(env),
    );
    (staker, ben, claim_id)
}

#[test]
#[should_panic(expected = "SAFU: cooldown not passed")]
fn claim_stream_before_cooldown_panics() {
    let env = new_env();
    let s = setup(&env);
    let (_staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, ENTITLEMENT);
    s.client.claim_stream(&claim_id, &ben);
}

#[test]
fn claim_stream_partial_vesting_amount() {
    let env = new_env();
    let s = setup(&env);
    let entitlement = 4_500_000i128; // divisible cleanly across 45 days
    let (_staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    advance_days(&env, 7); // cooldown passes
    advance_days(&env, 10); // 10 of 45 vesting days elapsed
    let transferred = s.client.claim_stream(&claim_id, &ben);
    assert_eq!(transferred, 1_000_000); // 4_500_000 * 10 / 45
}

#[test]
fn claim_stream_full_after_vesting_completes() {
    let env = new_env();
    let s = setup(&env);
    let entitlement = 4_500_000i128;
    let (_staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    advance_days(&env, 7);
    advance_days(&env, 45); // fully vested
    let transferred = s.client.claim_stream(&claim_id, &ben);
    assert_eq!(transferred, entitlement);
    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.status, ClaimStatus::Completed);
}

#[test]
#[should_panic(expected = "SAFU: claim not active")]
fn claim_stream_after_completion_panics() {
    let env = new_env();
    let s = setup(&env);
    let entitlement = 4_500_000i128;
    let (_staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    advance_days(&env, 7);
    advance_days(&env, 45);
    s.client.claim_stream(&claim_id, &ben);
    s.client.claim_stream(&claim_id, &ben);
}

#[test]
#[should_panic(expected = "SAFU: wrong beneficiary")]
fn claim_stream_wrong_beneficiary_panics() {
    let env = new_env();
    let s = setup(&env);
    let (_staker, _ben, claim_id) = active_claim_with_entitlement(&env, &s, ENTITLEMENT);
    advance_days(&env, 7);
    advance_days(&env, 45);
    let wrong = Address::generate(&env);
    s.client.claim_stream(&claim_id, &wrong);
}

#[test]
#[should_panic(expected = "SAFU: no such claim")]
fn claim_stream_nonexistent_claim_panics() {
    let env = new_env();
    let s = setup(&env);
    let ben = Address::generate(&env);
    s.client.claim_stream(&tx_hash(&env, 99), &ben);
}

#[test]
fn claim_stream_daily_outflow_cap_limits_large_payout() {
    let env = new_env();
    let s = setup(&env);
    // A second, non-claiming staker inflates the pool so the claim's
    // entitlement clears the tier/stress/solvency checks while still
    // being big enough to trip the 3%-of-base daily outflow cap.
    let anchor_staker = new_funded_address(&env, &s, MAX_STAKE);
    let anchor_ben = Address::generate(&env);
    s.client.stake(&anchor_staker, &MAX_STAKE, &anchor_ben);

    let entitlement = 300_000_000i128;
    let (_staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    advance_days(&env, 7);
    advance_days(&env, 45); // fully vested, claimable = full entitlement

    let transferred = s.client.claim_stream(&claim_id, &ben);
    // cap_base = max(total_staked_now, snapshot) = 1_350_000_000
    // (MAX_STAKE + MID_STAKE, snapshotted before this claim's forfeiture)
    // utilization = 300_000_000 / 1_350_000_000 ≈ 22.2% → 300bps (3%)
    // cap = 1_350_000_000 * 300 / 10_000 = 40_500_000
    assert_eq!(transferred, 40_500_000);
}

#[test]
#[should_panic(expected = "SAFU: daily outflow cap reached, try again tomorrow")]
fn claim_stream_second_call_same_day_hits_cap() {
    let env = new_env();
    let s = setup(&env);
    let anchor_staker = new_funded_address(&env, &s, MAX_STAKE);
    let anchor_ben = Address::generate(&env);
    s.client.stake(&anchor_staker, &MAX_STAKE, &anchor_ben);

    let entitlement = 300_000_000i128;
    let (_staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    advance_days(&env, 7);
    advance_days(&env, 45);

    s.client.claim_stream(&claim_id, &ben); // drains the day's cap
    s.client.claim_stream(&claim_id, &ben); // same day — nothing left
}

#[test]
fn claim_stream_next_day_cap_resets() {
    let env = new_env();
    let s = setup(&env);
    let anchor_staker = new_funded_address(&env, &s, MAX_STAKE);
    let anchor_ben = Address::generate(&env);
    s.client.stake(&anchor_staker, &MAX_STAKE, &anchor_ben);

    let entitlement = 300_000_000i128;
    let (_staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    advance_days(&env, 7);
    advance_days(&env, 45);

    let first = s.client.claim_stream(&claim_id, &ben);
    advance_days(&env, 1);
    let second = s.client.claim_stream(&claim_id, &ben);
    assert_eq!(first, 40_500_000);
    assert_eq!(second, 40_500_000); // fresh daily budget, same rate
}

// -----------------------------------------------------------------------
// cancel_claim
// -----------------------------------------------------------------------

#[test]
fn cancel_active_claim_restores_stake_with_penalty() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, ENTITLEMENT);
    s.client.cancel_claim(&claim_id);

    assert_eq!(s.client.get_total_staked(), MID_STAKE);
    assert_eq!(s.client.get_total_stakers(), 1);
    // Restored but penalty-locked — withdraw must still fail.
    let result = s.client.try_withdraw(&staker, &ben);
    assert!(result.is_err());
}

#[test]
fn cancel_active_claim_releases_total_allocated() {
    let env = new_env();
    let s = setup(&env);
    let (_staker, _ben, claim_id) = active_claim_with_entitlement(&env, &s, ENTITLEMENT);
    s.client.cancel_claim(&claim_id);
    assert_eq!(s.client.get_total_allocated(), 0);
}

#[test]
fn cancel_pending_claim_no_penalty() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    s.client.cancel_claim(&claim_id);
    // Stake was never forfeited (Pending) — no penalty lock, withdraw
    // works immediately.
    s.client.withdraw(&staker, &ben);
}

#[test]
#[should_panic(expected = "SAFU: claim not cancellable")]
fn cancel_completed_claim_panics() {
    let env = new_env();
    let s = setup(&env);
    let entitlement = 4_500_000i128;
    let (_staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    advance_days(&env, 7);
    advance_days(&env, 45);
    s.client.claim_stream(&claim_id, &ben);
    s.client.cancel_claim(&claim_id);
}

#[test]
#[should_panic(expected = "SAFU: no such claim")]
fn cancel_nonexistent_claim_panics() {
    let env = new_env();
    let s = setup(&env);
    s.client.cancel_claim(&tx_hash(&env, 99));
}

#[test]
fn cancel_active_claim_allows_restake_after_penalty_lock() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, ENTITLEMENT);
    s.client.cancel_claim(&claim_id);
    advance_days(&env, 365); // penalty lock clears
    s.client.withdraw(&staker, &ben);
}
