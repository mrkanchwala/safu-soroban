//! Tests added 2026-07-15 to close mutation-testing gaps (cargo-mutants,
//! first full run: 464 mutants, 60 missed). Each test names the mutant(s)
//! it kills by file:line. The 9 mutants NOT covered here are provably
//! equivalent (no observable behavior change) — documented with
//! justification in `.cargo/mutants.toml` at the workspace root, not
//! silently skipped.
//!
//! The common thread in what the original suite missed: line coverage was
//! 96%+ but boundary values (`>` vs `>=` at exact thresholds) and exact
//! arithmetic results (points formula, allocation release math) were
//! executed without being pinned by assertions. These tests assert exact
//! numbers at exact boundaries.

#![cfg(test)]

use soroban_sdk::testutils::Address as _;
use soroban_sdk::Address;

use super::common::*;

const ENTITLEMENT: i128 = 1_000_000;
const TIER_B: u32 = 2;
const TIER_C: u32 = 3;

/// Test-only re-derivation of the on-chain claim id — same as the helper
/// in override_tests.rs (kept local: test modules don't export helpers).
fn claim_id_for(
    env: &soroban_sdk::Env,
    wallet: &Address,
    hash: &soroban_sdk::BytesN<32>,
) -> soroban_sdk::BytesN<32> {
    use soroban_sdk::xdr::ToXdr;
    let mut buf = wallet.to_xdr(env);
    buf.append(&soroban_sdk::Bytes::from_array(env, &hash.to_array()));
    env.crypto().sha256(&buf).to_bytes()
}

/// Full 2-of-2 override (admin then coSigner, identical params).
fn do_override(
    env: &soroban_sdk::Env,
    s: &Setup<'_>,
    wallet: &Address,
    hash: &soroban_sdk::BytesN<32>,
    entitlement: i128,
    tier: u32,
) {
    let _ = env;
    s.client
        .approve_override(&s.admin, wallet, hash, &entitlement, &tier);
    s.client
        .approve_override(&s.co_signer, wallet, hash, &entitlement, &tier);
}

// -----------------------------------------------------------------------
// Points formula — exact values at every day-tier boundary.
// Kills stake.rs:82:19 (>→==), 82:38 (−→+, −→/), 83:19 (>→==),
// 83:29 (−→+, −→/), 84:36 (+→−), 84:41 (*→/), 84:47 (+→−, +→*),
// 84:52 (*→/), and lib.rs:151 (get_stake→None, via the live-computed
// points path needing a real record).
// points = (d1*100 + d2*120 + d3*150 + d4*200) * amount / max_stake,
// with amount=MID_STAKE=100M, max_stake=1.25B → factor 2/25.
// -----------------------------------------------------------------------

#[test]
fn points_formula_exact_values_at_all_day_tier_boundaries() {
    let cases: [(u32, i128); 10] = [
        (45, 360),    // inside tier 1: 4500 * 2/25
        (90, 720),    // tier-1/2 boundary: d2 still 0
        (91, 729),    // first tier-2 day: 9120 * 2/25 = 729.6 → 729
        (135, 1152),  // mid tier 2: 14400 * 2/25
        (180, 1584),  // tier-2/3 boundary: 19800 * 2/25
        (181, 1596),  // first tier-3 day: 19950 * 2/25
        (250, 2424),  // mid tier 3: 30300 * 2/25
        (365, 3804),  // tier-3/4 boundary: 47550 * 2/25
        (366, 3820),  // first tier-4 day: 47750 * 2/25
        (400, 4364),  // deep tier 4: 54550 * 2/25
    ];
    for (days, expected) in cases {
        let env = new_env();
        let s = setup(&env);
        let (staker, _ben) = staked_wallet(&env, &s);
        advance_days(&env, days);
        assert_eq!(
            s.client.get_points_balance(&staker),
            expected,
            "points mismatch at day {}",
            days
        );
    }
}

// -----------------------------------------------------------------------
// lib.rs views.
// -----------------------------------------------------------------------

/// Kills lib.rs:151 (get_stake → None).
#[test]
fn get_stake_returns_real_record() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    let record = s.client.get_stake(&staker).expect("record must exist");
    assert_eq!(record.amount, MID_STAKE);
    assert!(!record.withdrawn);
}

/// Kills lib.rs:206 (is_paused → true AND is_paused → false).
#[test]
fn is_paused_tracks_pause_state_both_ways() {
    let env = new_env();
    let s = setup(&env);
    assert!(!s.client.is_paused());
    s.client.pause();
    assert!(s.client.is_paused());
    s.client.unpause();
    assert!(!s.client.is_paused());
}

// -----------------------------------------------------------------------
// stress_cap / daily-entitlement accumulation (submit_claim admission).
// -----------------------------------------------------------------------

/// Five stakers (500M). First claim brings utilization to EXACTLY the
/// 2000-bps threshold; the next claim the same real day must hit the
/// tightened (1000-bps-rate) stress cap.
/// Kills claim.rs:119:43 (*→/ in stress_cap's utilization),
/// 120:39 (<→<= at the 2000 boundary), 293:25 (+→−, +→* in the
/// daily-entitlement accumulation), and 150:31 (/→* in current_day —
/// the ledger advance between the claims shifts the timestamp within
/// the same real day; a mutated day value would wrongly reset the
/// daily counter).
#[test]
#[should_panic(expected = "SAFU: daily stress cap exceeded")]
fn stress_cap_tightens_at_exactly_20_percent_utilization() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s);
    let (w2, _b2) = staked_wallet(&env, &s);
    let _ = staked_wallet(&env, &s);
    let _ = staked_wallet(&env, &s);
    let _ = staked_wallet(&env, &s);
    // total_staked = 500M. Stress cap at 0% utilization = 25% = 125M.
    s.client
        .submit_claim(&s.oracle, &w1, &tx_hash(&env, 1), &100_000_000, &TIER_C, &now_ts(&env));
    // utilization now exactly 2000 bps → rate drops to 1000 → cap 50M.
    // Same real day (500s later), different ledger.
    advance_ledgers(&env, 100);
    // 100M (today's total) + 1M > 50M → must panic.
    s.client
        .submit_claim(&s.admin, &w2, &tx_hash(&env, 2), &1_000_000, &TIER_C, &now_ts(&env));
}

/// Days 1–4: fill the daily stress cap EXACTLY each day, walking
/// utilization through 2000→3000→4000→5000 bps. All four must succeed —
/// any mutant that tightens the rate early (e.g. <→== / <→> at the
/// 5000-bps branch, which would misprice utilization 3000/4000 at
/// 300 bps) panics mid-walk and is caught.
/// Kills claim.rs:122:31 (<→==, <→>) and 270:38 (>→>= via the exact
/// daily fills).
#[test]
fn stress_cap_exact_daily_fills_walk_utilization_to_50_percent() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s);
    let (w2, _b2) = staked_wallet(&env, &s);
    let (w3, _b3) = staked_wallet(&env, &s);
    let (w4, _b4) = staked_wallet(&env, &s);
    let _ = staked_wallet(&env, &s);
    // Day 1: util 0 → cap 125M; take 100M → util 2000.
    s.client
        .submit_claim(&s.oracle, &w1, &tx_hash(&env, 1), &100_000_000, &TIER_C, &now_ts(&env));
    // Days 2–4: rate 1000 → cap 50M; exact fills.
    advance_days(&env, 1);
    s.client
        .submit_claim(&s.admin, &w2, &tx_hash(&env, 2), &50_000_000, &TIER_C, &now_ts(&env));
    advance_days(&env, 1);
    s.client
        .submit_claim(&s.admin, &w3, &tx_hash(&env, 3), &50_000_000, &TIER_C, &now_ts(&env));
    advance_days(&env, 1);
    s.client
        .submit_claim(&s.admin, &w4, &tx_hash(&env, 4), &50_000_000, &TIER_C, &now_ts(&env));
    assert_eq!(s.client.get_total_allocated(), 250_000_000);
}

/// Day 5 of the walk above: utilization is EXACTLY 5000 bps → rate must
/// be 300 (the `< 5_000` branch must NOT admit 5000 itself) → cap 15M.
/// Kills claim.rs:122:31 (<→<= at the 5000 boundary).
#[test]
#[should_panic(expected = "SAFU: daily stress cap exceeded")]
fn stress_cap_rate_drops_at_exactly_50_percent_utilization() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s);
    let (w2, _b2) = staked_wallet(&env, &s);
    let (w3, _b3) = staked_wallet(&env, &s);
    let (w4, _b4) = staked_wallet(&env, &s);
    let (w5, _b5) = staked_wallet(&env, &s);
    s.client
        .submit_claim(&s.oracle, &w1, &tx_hash(&env, 1), &100_000_000, &TIER_C, &now_ts(&env));
    advance_days(&env, 1);
    s.client
        .submit_claim(&s.admin, &w2, &tx_hash(&env, 2), &50_000_000, &TIER_C, &now_ts(&env));
    advance_days(&env, 1);
    s.client
        .submit_claim(&s.admin, &w3, &tx_hash(&env, 3), &50_000_000, &TIER_C, &now_ts(&env));
    advance_days(&env, 1);
    s.client
        .submit_claim(&s.admin, &w4, &tx_hash(&env, 4), &50_000_000, &TIER_C, &now_ts(&env));
    // Utilization exactly 5000 bps → rate 300 → cap 15M. 16M must panic.
    advance_days(&env, 1);
    s.client
        .submit_claim(&s.admin, &w5, &tx_hash(&env, 5), &16_000_000, &TIER_C, &now_ts(&env));
}

/// Entitlement that EXACTLY fills both the solvency gap and the daily
/// stress cap must be admitted (strict `>` on both checks).
/// Kills claim.rs:264:38 (>→>= solvency) and 270:38 (>→>= stress cap).
#[test]
fn submit_claim_exact_solvency_and_stress_fill_succeeds() {
    let env = new_env();
    let s = setup(&env);
    let (wa, _ba) = staked_wallet(&env, &s);
    let (wb, _bb) = staked_wallet(&env, &s);
    // Override forfeits wa's stake: staked 200M→100M, allocated 97M.
    do_override(&env, &s, &wa, &tx_hash(&env, 1), 97_000_000, TIER_C);
    assert_eq!(s.client.get_total_staked(), 100_000_000);
    // Next day: solvency gap = 100M − 97M = 3M; stress cap = 100M ×
    // 300bps (util 9700) = 3M. e = 3M fills BOTH exactly — must pass.
    advance_days(&env, 1);
    s.client
        .submit_claim(&s.oracle, &wb, &tx_hash(&env, 2), &3_000_000, &TIER_C, &now_ts(&env));
    assert_eq!(s.client.get_total_allocated(), 100_000_000);
}

// -----------------------------------------------------------------------
// claim_stream boundaries and math.
// -----------------------------------------------------------------------

/// At EXACTLY cooldown_ends the cooldown check must pass (strict `<`) —
/// the call then fails on "nothing vested yet" (elapsed = 0), which is a
/// DIFFERENT panic than "cooldown not passed".
/// Kills claim.rs:384:19 (<→<=).
#[test]
#[should_panic(expected = "SAFU: nothing vested yet")]
fn claim_stream_at_exact_cooldown_end_passes_cooldown_check() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    let hash = tx_hash(&env, 1);
    do_override(&env, &s, &staker, &hash, ENTITLEMENT, TIER_C);
    advance_days(&env, 7); // now == cooldown_ends_ledger exactly
    s.client.claim_stream(&claim_id_for(&env, &staker, &hash), &ben);
}

/// Second stream pays EXACTLY the newly-vested delta, not vested+streamed.
/// Kills claim.rs:401:34 (−→+ in claimable).
#[test]
fn claim_stream_second_call_pays_exactly_the_delta() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    let hash = tx_hash(&env, 1);
    do_override(&env, &s, &staker, &hash, 4_500_000, TIER_C);
    let claim_id = claim_id_for(&env, &staker, &hash);
    advance_days(&env, 7);
    advance_days(&env, 10); // 10 of 45 vesting days
    assert_eq!(s.client.claim_stream(&claim_id, &ben), 1_000_000);
    advance_days(&env, 10); // 20 of 45 — vested 2M, already paid 1M
    assert_eq!(s.client.claim_stream(&claim_id, &ben), 1_000_000);
}

/// Cancelling a partially-streamed claim releases EXACTLY the unstreamed
/// remainder from total_allocated.
/// Kills claim.rs:450:40 (−→+ in unstreamed).
///
/// CHANGED 2026-07-22 (bug 1 fix): the old expected value here (1,000,000)
/// was pinning the BUG — claim_stream never used to release its own
/// transferred amount from total_allocated, so the streamed 1M sat there
/// forever as phantom allocation even after cancel released the
/// remaining 3.5M. Now claim_stream releases its own transfer as it
/// happens, so total_allocated is already down to 3.5M by the time
/// cancel_claim runs; cancel then correctly releases that same 3.5M
/// (`entitlement - streamed`, unchanged formula, now operating on an
/// already-accurate total_allocated instead of an inflated one) —
/// ending at exactly 0, not 1,000,000. Kill-coverage for the `−→+`
/// mutant is unaffected: the formula itself didn't change, only what it
/// was released FROM did.
#[test]
fn cancel_partially_streamed_claim_releases_exact_remainder() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    let hash = tx_hash(&env, 1);
    do_override(&env, &s, &staker, &hash, 4_500_000, TIER_C);
    let claim_id = claim_id_for(&env, &staker, &hash);
    advance_days(&env, 7);
    advance_days(&env, 10);
    assert_eq!(s.client.claim_stream(&claim_id, &ben), 1_000_000);
    assert_eq!(s.client.get_total_allocated(), 3_500_000); // bug 1: released as-streamed
    s.client.cancel_claim(&claim_id);
    // Cancel releases the remaining 3.5M reserved for this claim — nothing
    // left allocated for it at all.
    assert_eq!(s.client.get_total_allocated(), 0);
}

// -----------------------------------------------------------------------
// dynamic_outflow_bps — exact utilization boundaries (payout side).
// -----------------------------------------------------------------------

/// Utilization EXACTLY 2000 bps against the cap base → rate must already
/// be 300 (the `< 2_000` branch must not admit 2000 itself).
/// Kills claim.rs:140:24 (<→<=).
#[test]
fn outflow_rate_drops_at_exactly_20_percent_utilization() {
    let env = new_env();
    let s = setup(&env);
    let anchor = new_funded_address(&env, &s, MAX_STAKE);
    let anchor_ben = Address::generate(&env);
    s.client.stake(&anchor, &MAX_STAKE, &anchor_ben);
    let (staker, ben) = staked_wallet(&env, &s);
    advance_days(&env, 90);
    // Lands in AwaitingApproval; approve_claim (same ledger, so the
    // snapshot math below is unaffected) takes the snapshot (cap base) =
    // 1.35B. 270M / 1.35B = exactly 2000 bps → 300 bps → cap 40.5M.
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &270_000_000,
        &TIER_C,
        &now_ts(&env),
    );
    s.client.approve_claim(&claim_id);
    advance_days(&env, 7);
    advance_days(&env, 45); // fully vested
    assert_eq!(s.client.claim_stream(&claim_id, &ben), 40_500_000);
}

/// Utilization EXACTLY 5000 bps → rate must already be 100.
/// Kills claim.rs:142:31 (<→<=).
#[test]
fn outflow_rate_floor_at_exactly_50_percent_utilization() {
    let env = new_env();
    let s = setup(&env);
    let anchor = new_funded_address(&env, &s, MAX_STAKE);
    let anchor_ben = Address::generate(&env);
    s.client.stake(&anchor, &MAX_STAKE, &anchor_ben);
    let (staker, ben) = staked_wallet(&env, &s);
    let hash = tx_hash(&env, 1);
    // Override path (no stress cap): 675M / 1.35B snapshot = exactly
    // 5000 bps → 100 bps → cap 13.5M. Tier B so the tier cap (1B) clears.
    do_override(&env, &s, &staker, &hash, 675_000_000, TIER_B);
    let claim_id = claim_id_for(&env, &staker, &hash);
    advance_days(&env, 7);
    advance_days(&env, 10); // vested 150M — cap binds at 13.5M
    assert_eq!(s.client.claim_stream(&claim_id, &ben), 13_500_000);
}

// -----------------------------------------------------------------------
// execute_override — status gate, release math, deadlines, boundaries.
// -----------------------------------------------------------------------

/// Re-executing an override on a still-Active claim must release the
/// prior reservation before re-adding — allocation ends at exactly
/// ENTITLEMENT, and the withdrawn-branch deadlines are exact.
/// Kills claim.rs:578:44 (||→&&), 578:21 (==→!= via the Active path),
/// 641:49 and 642:64 (+→− in the withdrawn-branch deadlines).
#[test]
fn override_reexecution_releases_prior_reservation_exactly() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    let hash = tx_hash(&env, 1);
    do_override(&env, &s, &staker, &hash, ENTITLEMENT, TIER_C);
    assert_eq!(s.client.get_total_allocated(), ENTITLEMENT);
    advance_days(&env, 3);
    do_override(&env, &s, &staker, &hash, ENTITLEMENT, TIER_C);
    // Released then re-added — NOT doubled.
    assert_eq!(s.client.get_total_allocated(), ENTITLEMENT);
    // Withdrawn-branch deadlines: fresh from "now" (re-execution ledger).
    let now = env.ledger().sequence();
    let claim = s.client.get_claim(&claim_id_for(&env, &staker, &hash)).unwrap();
    assert_eq!(claim.cooldown_ends_ledger, now + crate::types::COOLDOWN_LEDGERS);
    assert_eq!(
        claim.vesting_ends_ledger,
        now + crate::types::COOLDOWN_LEDGERS + crate::types::VESTING_LEDGERS
    );
}

/// Overriding a wallet whose prior claim was CANCELLED must NOT release
/// anything (cancel already did) — asserted with a second wallet's live
/// reservation present, so a wrong release visibly deducts from it
/// instead of vanishing into the .max(0) clamp.
/// Kills claim.rs:578:21 and 578:56 (==→!= via the Cancelled path).
#[test]
fn override_after_cancel_does_not_touch_other_reservations() {
    let env = new_env();
    let s = setup(&env);
    let (wx, _bx) = staked_wallet(&env, &s);
    let (wy, _by) = staked_wallet(&env, &s);
    // Live reservation on wx (PendingTime — no forfeiture).
    s.client
        .submit_claim(&s.oracle, &wx, &tx_hash(&env, 1), &ENTITLEMENT, &TIER_C, &now_ts(&env));
    // wy: claim then cancel (its reservation already released by cancel).
    let hash_y = tx_hash(&env, 2);
    let claim_y = s.client.submit_claim(
        &s.admin, &wy, &hash_y, &ENTITLEMENT, &TIER_C, &now_ts(&env),
    );
    s.client.cancel_claim(&claim_y);
    assert_eq!(s.client.get_total_allocated(), ENTITLEMENT); // wx's only
    // Override re-targets wy (prior status: Cancelled). Must add wy's
    // new reservation WITHOUT releasing anything — wx's stays intact.
    advance_days(&env, 366); // penalty lock from the cancel clears
    do_override(&env, &s, &wy, &hash_y, ENTITLEMENT, TIER_C);
    assert_eq!(s.client.get_total_allocated(), 2 * ENTITLEMENT);
}

/// Re-execution after a partial stream: release is exactly
/// entitlement − streamed, then the new entitlement re-adds in full.
/// Kills claim.rs:579:44 (−→+) and 581:64 (−→+, −→/).
///
/// CHANGED 2026-07-22 (bugs 1 + 3 fixes): old expected value (5,500,000)
/// was pinning TWO bugs at once — bug 1 (claim_stream never released its
/// own transfer, so total_allocated was still 4.5M going into the
/// re-execution instead of the correct 3.5M) and bug 3 (the re-executed
/// claim's `streamed` was hard-reset to 0, letting the beneficiary
/// re-collect the FULL new entitlement on top of the 1M already paid —
/// an overpayment). Both fixed: total_allocated is 3.5M before
/// re-execution, releases 3.5M (→0), re-adds 4.5M (→4.5M) — and the new
/// claim record carries `streamed = 1,000,000` forward, so only 3.5M is
/// actually still collectible, not another full 4.5M.
#[test]
fn override_reexecution_after_partial_stream_exact_release_math() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    let hash = tx_hash(&env, 1);
    do_override(&env, &s, &staker, &hash, 4_500_000, TIER_C);
    let claim_id = claim_id_for(&env, &staker, &hash);
    advance_days(&env, 7);
    advance_days(&env, 10);
    assert_eq!(s.client.claim_stream(&claim_id, &ben), 1_000_000);
    assert_eq!(s.client.get_total_allocated(), 3_500_000); // bug 1: already net of the 1M paid
    // Re-execute same params: release the current 3.5M (→0), re-add the
    // full 4.5M entitlement (→4.5M) — not 5.5M.
    do_override(&env, &s, &staker, &hash, 4_500_000, TIER_C);
    assert_eq!(s.client.get_total_allocated(), 4_500_000);
    // Bug 3 regression: streamed carried forward, not reset to 0 — the
    // beneficiary can only collect the remaining 3.5M, not another 4.5M.
    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.streamed, 1_000_000);
}

/// An override entitlement EXACTLY at the tier cap AND exactly at the
/// solvency limit must execute (strict `>` on both checks).
/// Kills claim.rs:598:24 (>→>=) and 618:42 (>→>=).
#[test]
fn override_exact_tier_cap_and_solvency_fill_succeeds() {
    let env = new_env();
    let s = setup(&env);
    let (wa, _ba) = staked_wallet(&env, &s);
    let _ = staked_wallet(&env, &s);
    let _ = staked_wallet(&env, &s);
    let _ = staked_wallet(&env, &s);
    let _ = staked_wallet(&env, &s);
    // total_staked 500M. Tier C cap for wa = 5 × 100M = 500M. e = 500M
    // is exactly AT the tier cap and exactly fills solvency.
    do_override(&env, &s, &wa, &tx_hash(&env, 1), 500_000_000, TIER_C);
    assert_eq!(s.client.get_total_allocated(), 500_000_000);
}

/// Bug 2 regression (eng review 2026-07-22): execute_override must not be
/// able to create a second, independently-payable claim on a wallet that
/// already has one in flight under a different tx_hash — the old code had
/// no equivalent to submit_claim's claim_active guard.
#[test]
#[should_panic(expected = "SAFU: wallet already has a different active claim")]
fn override_blocks_second_claim_on_wallet_with_existing_claim() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    // Wallet already has a live claim (PendingTime — gate not met).
    s.client
        .submit_claim(&s.oracle, &staker, &tx_hash(&env, 1), &ENTITLEMENT, &TIER_C, &now_ts(&env));
    // A DIFFERENT tx_hash for the SAME wallet via override must be
    // refused — without the fix, this would create a second, independently
    // payable claim against the same forfeited stake.
    do_override(&env, &s, &staker, &tx_hash(&env, 2), ENTITLEMENT, TIER_C);
}

/// Bug 4 regression (eng review 2026-07-22): the daily-outflow-cap
/// subtraction must clamp, not panic, when the recomputed cap ends up
/// BELOW what's already been paid out that day — the scenario the old
/// `.max(0)`-after-subtract pattern could never actually protect against
/// (the subtraction itself panicked first, under this workspace's
/// `overflow-checks = true`). Forces exactly that: staker A collects
/// while utilization is low (cheap 500bps rate), then a same-day override
/// on a different wallet spikes utilization past 50%, shrinking the
/// recomputed cap below what A already collected today. A's next call
/// must fail with the graceful message, not a raw arithmetic panic.
#[test]
#[should_panic(expected = "SAFU: daily outflow cap reached, try again tomorrow")]
fn claim_stream_cap_shrinking_mid_day_fails_gracefully_not_via_panic() {
    let env = new_env();
    let s = setup(&env);
    let anchor = new_funded_address(&env, &s, MAX_STAKE);
    let anchor_ben = Address::generate(&env);
    s.client.stake(&anchor, &MAX_STAKE, &anchor_ben);

    let (staker_a, ben_a) = staked_wallet(&env, &s); // MID_STAKE = 100M
    advance_days(&env, 90); // gate met
    let claim_a = s.client.submit_claim(
        &s.oracle,
        &staker_a,
        &tx_hash(&env, 1),
        &100_000_000, // TIER_B cap = 100M*10 = 1B, plenty of room
        &TIER_B,
        &now_ts(&env),
    );
    s.client.approve_claim(&claim_a);
    advance_days(&env, 7); // cooldown
    advance_days(&env, 45); // full vesting

    // cap_base_a snapshot = anchor + A = 1.35B. Utilization = 100M/1.35B
    // ≈ 7.4% (<20%) → 500bps → cap = 67.5M. Fully vested, so this drains
    // the day's cap in one call.
    let first = s.client.claim_stream(&claim_a, &ben_a);
    assert_eq!(first, 67_500_000);

    // Same day: a second, much larger wallet gets an overridden claim —
    // spikes pool-wide total_allocated, and its own stake also raises
    // total_staked (which cap_base_a tracks via `max(total_staked_now,
    // snapshot)`), pushing utilization for A's own rate past 50%.
    let staker_b = new_funded_address(&env, &s, 200_000_000);
    let ben_b = Address::generate(&env);
    s.client.stake(&staker_b, &200_000_000, &ben_b);
    do_override(&env, &s, &staker_b, &tx_hash(&env, 2), 700_000_000, TIER_B);

    // A's second call, same real day: recomputed cap (100bps of the new,
    // larger cap_base) is now BELOW the 67.5M already paid today. Old
    // code: `(cap - daily_outflow_so_far)` panics with a raw overflow
    // trap. Fixed code: saturating_sub clamps to 0, and the transfer
    // amount check produces the intended, graceful message instead.
    s.client.claim_stream(&claim_a, &ben_a);
}

// -----------------------------------------------------------------------
// stake / withdraw / emergency_exit.
// -----------------------------------------------------------------------

/// A forfeited stake (withdrawn=true, amount kept live) must NOT block a
/// fresh re-stake — the guard is `amount > 0 && !withdrawn`, not either
/// condition alone.
/// Kills stake.rs:126:32 (&&→||).
#[test]
fn restake_after_forfeiture_and_full_stream_succeeds() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    let hash = tx_hash(&env, 1);
    do_override(&env, &s, &staker, &hash, 900_000, TIER_C);
    let claim_id = claim_id_for(&env, &staker, &hash);
    advance_days(&env, 7);
    advance_days(&env, 45);
    s.client.claim_stream(&claim_id, &ben); // completes the claim
    // Forfeited record: amount still 100M, withdrawn=true. Re-stake must
    // be allowed.
    s.token_admin.mint(&staker, &MID_STAKE);
    let new_ben = Address::generate(&env);
    s.client.stake(&staker, &MID_STAKE, &new_ben);
    assert_eq!(s.client.get_stake(&staker).unwrap().amount, MID_STAKE);
    assert!(!s.client.get_stake(&staker).unwrap().withdrawn);
}

/// Staking to EXACTLY the pool cap must succeed (strict `>`).
/// Kills stake.rs:134:30 (>→>=).
#[test]
fn stake_filling_pool_cap_exactly_succeeds() {
    let env = new_env();
    let s = setup(&env);
    let (_w1, _b1) = staked_wallet(&env, &s); // 100M staked
    s.client.set_pool_cap(&101_000_000);
    // New stake bounds: min 20_200, max 1_262_500. 1M fits, and
    // 100M + 1M == new cap exactly.
    let w2 = new_funded_address(&env, &s, 1_000_000);
    let b2 = Address::generate(&env);
    s.client.stake(&w2, &1_000_000, &b2);
    assert_eq!(s.client.get_total_staked(), 101_000_000);
}

/// Overshooting the pool cap by ANY margin (not only hitting it exactly)
/// must panic.
/// Kills stake.rs:134:30 (>→==).
#[test]
#[should_panic(expected = "SAFU: pool cap exceeded")]
fn stake_overshooting_pool_cap_panics() {
    let env = new_env();
    let s = setup(&env);
    let (_w1, _b1) = staked_wallet(&env, &s); // 100M staked
    s.client.set_pool_cap(&100_500_000);
    // 100M + 1M = 101M > 100.5M (and ≠ 100.5M — kills the == mutant).
    let w2 = new_funded_address(&env, &s, 1_000_000);
    let b2 = Address::generate(&env);
    s.client.stake(&w2, &1_000_000, &b2);
}

/// Staker count increments by exactly 1 per stake.
/// Kills stake.rs:171:69 (+→*).
#[test]
fn total_stakers_increments_exactly() {
    let env = new_env();
    let s = setup(&env);
    let _ = staked_wallet(&env, &s);
    assert_eq!(s.client.get_total_stakers(), 1);
    let _ = staked_wallet(&env, &s);
    assert_eq!(s.client.get_total_stakers(), 2);
}

/// emergency_exit on a forfeited (not voluntarily-withdrawn) stake must
/// fail on the "no active stake" guard — reaching the claim_active check
/// instead would mean the || in the guard degraded to &&.
/// Kills stake.rs:276:27 (||→&&).
#[test]
#[should_panic(expected = "SAFU: no active stake")]
fn emergency_exit_after_forfeiture_fails_on_active_stake_guard() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    let hash = tx_hash(&env, 1);
    do_override(&env, &s, &staker, &hash, 900_000, TIER_C);
    let claim_id = claim_id_for(&env, &staker, &hash);
    advance_days(&env, 7);
    advance_days(&env, 45);
    s.client.claim_stream(&claim_id, &ben); // Completed
    // Record: amount 100M > 0, withdrawn=true → "no active stake".
    s.client.emergency_exit(&staker);
}

/// emergency_exit decrements total_staked by exactly the exiting amount.
/// Kills stake.rs:289:49 (−→+, −→/).
#[test]
fn emergency_exit_decrements_total_staked_exactly() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    let _ = staked_wallet(&env, &s);
    assert_eq!(s.client.get_total_staked(), 2 * MID_STAKE);
    s.client.emergency_exit(&staker);
    assert_eq!(s.client.get_total_staked(), MID_STAKE);
}

// -----------------------------------------------------------------------
// Penalty lock duration (types.rs).
// -----------------------------------------------------------------------

/// The false-positive-cancel penalty lock is 365 DAYS of ledgers — still
/// firmly locked after 2 days (a 365+17280-ledger mutant ≈ 1 day would
/// have expired).
/// Kills types.rs:40:43 (*→+ in PENALTY_LOCK_LEDGERS).
#[test]
#[should_panic(expected = "SAFU: penalty lock active")]
fn penalty_lock_still_active_after_two_days() {
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
    // CHANGED 2026-07-22: gate-met no longer auto-activates — the claim
    // must be explicitly approved before cancelling it exercises the
    // "was Active" penalty-lock branch this test is pinning.
    s.client.approve_claim(&claim_id);
    s.client.cancel_claim(&claim_id); // restores stake + penalty lock
    advance_days(&env, 2);
    s.client.withdraw(&staker, &ben);
}

// -----------------------------------------------------------------------
// Instance-TTL bumps (storage.rs).
// -----------------------------------------------------------------------

/// Every state-touching entrypoint bumps the instance TTL to exactly
/// BUMP_TO (120 days of ledgers), and a later call re-bumps once the
/// remaining TTL falls below BUMP_THRESHOLD (30 days) — the second phase
/// distinguishes the real 518,400-ledger threshold from a mutated
/// (30 + 17280 = 17,310) one.
/// Kills storage.rs:23:32 (*→+, *→/) and 234:5 (bump_instance_ttl→()).
#[test]
fn instance_ttl_bumped_to_exact_target_and_rebumped_below_threshold() {
    use soroban_sdk::testutils::storage::Instance;
    let env = new_env();
    let s = setup(&env);
    let contract_id = s.client.address.clone();
    let _ = staked_wallet(&env, &s); // any bumping entrypoint
    let bump_to = 120 * crate::types::LEDGERS_PER_DAY; // 2_073_600
    let ttl_after_stake = env.as_contract(&contract_id, || env.storage().instance().get_ttl());
    assert_eq!(ttl_after_stake, bump_to);
    // Age the instance so remaining TTL (100_000) sits BETWEEN the
    // mutated threshold (17_310) and the real one (518_400): the real
    // code re-bumps, the mutant would not.
    advance_ledgers(&env, bump_to - 100_000);
    let _ = staked_wallet(&env, &s);
    let ttl_after_second = env.as_contract(&contract_id, || env.storage().instance().get_ttl());
    assert_eq!(ttl_after_second, bump_to);
}
