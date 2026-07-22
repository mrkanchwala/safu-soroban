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
fn submit_claim_awaiting_approval_when_gate_already_met() {
    // CHANGED 2026-07-22: meeting the gate at submission time no longer
    // auto-activates (forfeits/burns/starts cooldown) — it lands in
    // AwaitingApproval with a deadline set, and the staker must actively
    // call approve_claim (Rule A) before anything is forfeited.
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
    assert_eq!(claim.status, ClaimStatus::AwaitingApproval);
    assert!(claim.approve_deadline_ledger > 0);
    // Nothing forfeited yet — total_staked/stakers unchanged.
    assert_eq!(s.client.get_total_staked(), MID_STAKE);
    assert_eq!(s.client.get_total_stakers(), 1);
}

#[test]
fn approve_claim_forfeits_and_activates() {
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
    s.client.approve_claim(&claim_id);
    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.status, ClaimStatus::Active);
    assert_eq!(s.client.get_total_staked(), 0);
    assert_eq!(s.client.get_total_stakers(), 0);
    // Rule B's clock anchors at cooldown end, not the approval ledger.
    assert_eq!(claim.last_collected_ledger, claim.cooldown_ends_ledger);
}

#[test]
fn approve_claim_burns_entire_lifetime_points_balance() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    advance_days(&env, 90);
    // First cycle: withdraw before ever claiming, banking points into the
    // wallet's lifetime balance.
    s.client.withdraw(&staker, &ben);
    let banked_from_cycle_1 = s.client.get_points_balance(&staker);
    assert!(banked_from_cycle_1 > 0);

    // Second cycle: stake again, get hacked, approve — should burn BOTH
    // this cycle's points AND the banked balance from cycle 1. withdraw()
    // sent the first cycle's principal to `ben` (the beneficiary), not
    // back to `staker` — needs fresh funding to stake again.
    s.token_admin.mint(&staker, &MID_STAKE);
    s.client.stake(&staker, &MID_STAKE, &ben);
    advance_days(&env, 90);
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    s.client.approve_claim(&claim_id);
    assert_eq!(s.client.get_points_balance(&staker), 0);
}

#[test]
#[should_panic(expected = "SAFU: claim not awaiting approval")]
fn approve_claim_before_gate_panics() {
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
    // Still PendingTime — gate not met, never transitioned to AwaitingApproval.
    s.client.approve_claim(&claim_id);
}

#[test]
#[should_panic(expected = "SAFU: approval window expired")]
fn approve_claim_after_100_days_panics() {
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
    advance_days(&env, 101);
    s.client.approve_claim(&claim_id);
}

#[test]
#[should_panic(expected = "SAFU: stake suspended")]
fn approve_claim_while_suspended_panics() {
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
    s.client.suspend_stake(&staker);
    s.client.approve_claim(&claim_id);
}

// -----------------------------------------------------------------------
// expire_pending_approval — Rule A sweep
// -----------------------------------------------------------------------

#[test]
fn expire_pending_approval_releases_reservation() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    advance_days(&env, 90);
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &ENTITLEMENT,
        &TIER_C,
        &now_ts(&env),
    );
    advance_days(&env, 101);
    s.client.expire_pending_approval(&claim_id);

    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.status, ClaimStatus::Expired);
    assert_eq!(s.client.get_total_allocated(), 0);
    // Nothing was ever forfeited — stake is untouched and withdrawable.
    s.client.withdraw(&staker, &ben);
}

/// Audit finding 2026-07-22 (adversarial /audit-chain re-review): the
/// suspend upgrade was dead code for any already-approved claim until
/// admin.rs's `suspend_stake` guard was corrected to allow suspending a
/// forfeited-but-still-active stake, not just a pre-forfeiture one. This
/// proves the positive path actually works end-to-end: suspend mid-
/// streaming genuinely blocks the next collection.
#[test]
#[should_panic(expected = "SAFU: stake suspended")]
fn suspend_during_active_streaming_blocks_claim_stream() {
    let env = new_env();
    let s = setup(&env);
    let entitlement = 4_500_000i128;
    let (staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    advance_days(&env, 7);
    advance_days(&env, 10);
    s.client.claim_stream(&claim_id, &ben); // proves streaming works first
    s.client.suspend_stake(&staker); // now reachable post-forfeiture
    advance_days(&env, 10);
    s.client.claim_stream(&claim_id, &ben); // blocked while suspended
}

/// Audit finding 2026-07-22 (adversarial re-review, /audit-chain): a
/// suspended staker's Rule A clock must not be sweepable while they're
/// still frozen out of acting — otherwise staying suspended past the
/// deadline (never explicitly unsuspended) loses them the reservation
/// regardless, defeating blocker #1's fairness fix.
#[test]
#[should_panic(expected = "SAFU: stake suspended")]
fn expire_pending_approval_blocked_while_suspended() {
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
    s.client.suspend_stake(&staker);
    advance_days(&env, 101); // deadline genuinely passed
    s.client.expire_pending_approval(&claim_id);
}

#[test]
#[should_panic(expected = "SAFU: approval window not yet expired")]
fn expire_pending_approval_before_deadline_panics() {
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
    advance_days(&env, 99);
    s.client.expire_pending_approval(&claim_id);
}

// -----------------------------------------------------------------------
// expire_stale_claim — Rule B sweep
// -----------------------------------------------------------------------

#[test]
fn expire_stale_claim_after_100_days_inactivity_releases_remainder() {
    let env = new_env();
    let s = setup(&env);
    let entitlement = 4_500_000i128;
    let (_staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    advance_days(&env, 7); // cooldown passes
    advance_days(&env, 10); // partial vesting
    let transferred = s.client.claim_stream(&claim_id, &ben);
    assert!(transferred > 0);

    advance_days(&env, 101); // 100+ days with zero further activity
    s.client.expire_stale_claim(&claim_id);

    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.status, ClaimStatus::Expired);
    assert_eq!(s.client.get_total_allocated(), 0);
}

#[test]
fn expire_stale_claim_zero_streamed_case() {
    let env = new_env();
    let s = setup(&env);
    let entitlement = 4_500_000i128;
    let (_staker, _ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    // Never called claim_stream even once.
    advance_days(&env, 7 + 101);
    s.client.expire_stale_claim(&claim_id);
    assert_eq!(s.client.get_total_allocated(), 0);
}

/// Audit finding 2026-07-22 — same fairness gap as
/// expire_pending_approval_blocked_while_suspended, but for Rule B.
#[test]
#[should_panic(expected = "SAFU: stake suspended")]
fn expire_stale_claim_blocked_while_suspended() {
    let env = new_env();
    let s = setup(&env);
    let entitlement = 4_500_000i128;
    let (staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    advance_days(&env, 7);
    advance_days(&env, 10);
    s.client.claim_stream(&claim_id, &ben);
    s.client.suspend_stake(&staker);
    advance_days(&env, 101); // genuinely stale now
    s.client.expire_stale_claim(&claim_id);
}

#[test]
#[should_panic(expected = "SAFU: claim not yet stale")]
fn expire_stale_claim_before_100_days_panics() {
    let env = new_env();
    let s = setup(&env);
    let entitlement = 4_500_000i128;
    let (_staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    advance_days(&env, 7);
    advance_days(&env, 10);
    s.client.claim_stream(&claim_id, &ben);
    advance_days(&env, 99); // resets from the collection above, not yet stale
    s.client.expire_stale_claim(&claim_id);
}

#[test]
fn claim_stream_resets_rule_b_clock() {
    let env = new_env();
    let s = setup(&env);
    let entitlement = 4_500_000i128;
    let (_staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    advance_days(&env, 7);
    advance_days(&env, 10);
    s.client.claim_stream(&claim_id, &ben);
    // Without the reset, 90 more days here plus the prior gap would trip
    // Rule B — collecting again proves the clock genuinely moved forward.
    advance_days(&env, 90);
    let transferred = s.client.claim_stream(&claim_id, &ben);
    assert!(transferred > 0);
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
fn unlock_pending_claim_moves_to_awaiting_approval_after_gate() {
    // CHANGED 2026-07-22: unlock_pending_claim no longer activates
    // directly — like submit_claim's gate-met branch, it now lands in
    // AwaitingApproval, still gated on the staker's own approve_claim.
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
    assert_eq!(claim.status, ClaimStatus::AwaitingApproval);
    assert!(claim.approve_deadline_ledger > 0);
    assert_eq!(s.client.get_total_staked(), MID_STAKE);

    // And approving from here works exactly like the submit_claim path.
    s.client.approve_claim(&claim_id);
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

/// CHANGED 2026-07-22 (points burn-on-claim mechanism): meeting the gate
/// no longer auto-activates — it lands in AwaitingApproval, and the
/// staker must call `approve_claim` themselves (Rule A) before the stake
/// forfeits and cooldown/vesting starts. Added that call here so every
/// test using this helper still gets a genuinely Active claim, same as
/// before the mechanism change.
fn active_claim_with_entitlement(
    env: &soroban_sdk::Env,
    s: &Setup<'_>,
    entitlement: i128,
) -> (Address, Address, soroban_sdk::BytesN<32>) {
    let (staker, ben) = staked_wallet(env, s);
    advance_days(env, 90); // gate met, lands in AwaitingApproval on submit
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(env, 1),
        &entitlement,
        &TIER_C,
        &now_ts(env),
    );
    s.client.approve_claim(&claim_id);
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
    // CHANGED 2026-07-22 (bug 1 fix): entitlement bumped 300M -> 320M.
    // Bug 1 used to leave total_allocated frozen at the full entitlement
    // between calls (claim_stream never released its own transfer), so
    // utilization never moved within a day. Now it does — 300M would
    // drop utilization below the 20% band after the first 40.5M payout,
    // bumping the rate to 500bps and leaving same-day headroom instead of
    // hitting the cap (see claim_stream_next_day_cap_resets for that
    // exact scenario). 320M keeps post-payout utilization just above 20%
    // (279.5M / 1.35B ≈ 20.7%), so the rate stays at 300bps for the
    // second same-day check and it genuinely has nothing left.
    let env = new_env();
    let s = setup(&env);
    let anchor_staker = new_funded_address(&env, &s, MAX_STAKE);
    let anchor_ben = Address::generate(&env);
    s.client.stake(&anchor_staker, &MAX_STAKE, &anchor_ben);

    let entitlement = 320_000_000i128;
    let (_staker, ben, claim_id) = active_claim_with_entitlement(&env, &s, entitlement);
    advance_days(&env, 7);
    advance_days(&env, 45);

    let first = s.client.claim_stream(&claim_id, &ben); // drains the day's cap
    assert_eq!(first, 40_500_000);
    s.client.claim_stream(&claim_id, &ben); // same day, same rate — nothing left
}

#[test]
fn claim_stream_next_day_cap_resets() {
    // CHANGED 2026-07-22 (bug 1 fix): `second` was 40,500,000 under the
    // bug — total_allocated stayed frozen at the full 300M between calls,
    // so utilization (and therefore the payout rate) never moved even
    // though 40.5M had already left the pool. Fixed: total_allocated
    // drops to 259.5M after the first payout, utilization falls to
    // ~19.2% (below the 20% band), and the rate correctly jumps to
    // 500bps (5%) on the new day — cap = 1.35B × 500/10_000 = 67.5M, and
    // claimable (259.5M remaining) comfortably covers it. This is the
    // fix working as intended: the pool pays out FASTER as it de-stresses,
    // not throttled forever by phantom allocation that was already paid.
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
    assert_eq!(second, 67_500_000); // rate improved to 500bps as utilization fell
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
