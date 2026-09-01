//! T3 (2026-08-24) — admission-side retry queue, bidirectional liquidity
//! rebalancing, and the `total_staked` shortfall reconciliation. Split out
//! for the same reason `d2_vault_tests` was: new mechanic, not existing
//! pool mechanics. Reuses `d2_vault_tests`'s `MockVault`/`with_vault`/
//! `assert_invariant` rather than duplicating the mock.

#![cfg(test)]

use super::common::*;
use super::d2_vault_tests::{assert_invariant, with_vault};
use crate::error::PoolError;
use crate::types::ClaimStatus;

const TIER_C: u32 = 3;

// -----------------------------------------------------------------------
// Admission-side queue — submit_claim -> Reserved -> release/expire.
// -----------------------------------------------------------------------

#[test]
fn queued_claim_releases_once_capacity_frees() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s); // 100M staked
    // stress_cap at 0% utilization = 25% of total_staked = 25M. 26M queues.
    let entitlement = 26_000_000;
    let claim_id = submit_claim_signed(
        &env, &s, &s.oracle, &w1, &tx_hash(&env, 1), &entitlement, &TIER_C, &now_ts(&env),
    );
    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.status, ClaimStatus::Reserved);
    // Queuing must not touch capacity accounting — it was never admitted.
    assert_eq!(s.client.get_total_allocated(), 0);

    // Grow the pool so the stress cap clears the queued entitlement —
    // total_staked 400M -> stress_cap 100M at 0% utilization.
    staked_wallet(&env, &s);
    staked_wallet(&env, &s);
    staked_wallet(&env, &s);

    s.client.try_release_queued_claim(&claim_id);
    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_ne!(claim.status, ClaimStatus::Reserved);
    assert_eq!(s.client.get_total_allocated(), entitlement);
    // Wallet's queue slot is freed on release.
    assert!(s.client.get_stake(&w1).unwrap().reserved_claim_id.is_none());
}

#[test]
fn try_release_queued_claim_stays_reserved_while_still_blocked() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s);
    let entitlement = 26_000_000;
    let claim_id = submit_claim_signed(
        &env, &s, &s.oracle, &w1, &tx_hash(&env, 1), &entitlement, &TIER_C, &now_ts(&env),
    );
    // Nothing about pool capacity changed — still blocked.
    let result = s.client.try_try_release_queued_claim(&claim_id);
    assert_eq!(result, Err(Ok(PoolError::QueueReleaseNotYetEligible)));
    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.status, ClaimStatus::Reserved);
}

#[test]
fn try_release_queued_claim_on_unknown_claim_fails() {
    let env = new_env();
    let s = setup(&env);
    let fake_id = tx_hash(&env, 99); // right shape, not a real claim id
    let result = s.client.try_try_release_queued_claim(&fake_id);
    assert_eq!(result, Err(Ok(PoolError::NoSuchQueuedClaim)));
}

#[test]
fn expire_queued_claim_before_window_fails() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s);
    let entitlement = 26_000_000;
    let claim_id = submit_claim_signed(
        &env, &s, &s.oracle, &w1, &tx_hash(&env, 1), &entitlement, &TIER_C, &now_ts(&env),
    );
    let result = s.client.try_expire_queued_claim(&claim_id);
    assert_eq!(result, Err(Ok(PoolError::QueueNotYetExpired)));
}

#[test]
fn expire_queued_claim_after_window_clears_the_wallet_slot() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s);
    let hack_ts = now_ts(&env);
    let entitlement = 26_000_000;
    let claim_id = submit_claim_signed(
        &env, &s, &s.oracle, &w1, &tx_hash(&env, 1), &entitlement, &TIER_C, &hack_ts,
    );
    // Pin the boundary itself: the check is `now <= hack_ts + WINDOW`, so at
    // EXACTLY the window it must still refuse, and one second later it must
    // sweep. A `<=` -> `<` mutation flips only in that one-second gap.
    advance_days(&env, 30);
    let now = now_ts(&env);
    assert!(now <= hack_ts + 30 * 86_400, "must still be inside the window");
    assert_eq!(
        s.client.try_expire_queued_claim(&claim_id),
        Err(Ok(PoolError::QueueNotYetExpired))
    );

    advance_days(&env, 1); // now strictly past it
    s.client.expire_queued_claim(&claim_id);

    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.status, ClaimStatus::Expired);
    assert!(s.client.get_stake(&w1).unwrap().reserved_claim_id.is_none());
}

/// 2-of-2 override, both approvers.
fn do_override(
    s: &Setup<'_>,
    wallet: &soroban_sdk::Address,
    hash: &soroban_sdk::BytesN<32>,
    entitlement: i128,
    tier: u32,
) {
    s.client.approve_override(&s.admin, wallet, hash, &entitlement, &tier);
    s.client.approve_override(&s.co_signer, wallet, hash, &entitlement, &tier);
}

/// REGRESSION — found by the 2026-08-24 T3 audit pass, not by a test.
///
/// A `Reserved` claim deliberately does not set `active_claim_id`, so the
/// one-claim-per-wallet guard in `execute_override` (which only inspects
/// `active_claim_id`) does not see it. A 2-of-2 override could therefore
/// create a second, DIFFERENT live claim for a wallet that already had one
/// queued. Releasing the queued one afterwards would then have produced two
/// independently-payable claims against a single stake — the exact invariant
/// the 2026-07-22 "Bug 2 fix" closed for the override path.
#[test]
fn releasing_a_queued_claim_is_refused_when_the_wallet_has_another_active_claim() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s); // 100M staked
    let (_w2, _b2) = staked_wallet(&env, &s); // more backing so the override is solvent

    // Queue a claim. Two wallets are staked (200M) so the override below is
    // solvent, which puts stress_cap at 25% of 200M = 50M — so the entitlement
    // has to clear 50M, not the 25M that applies in the single-wallet tests
    // above. 51M queues, stays inside tier C's 500M cap on a 100M stake, and
    // is comfortably solvent against 200M.
    let queued_id = submit_claim_signed(
        &env, &s, &s.oracle, &w1, &tx_hash(&env, 1), &51_000_000, &TIER_C, &now_ts(&env),
    );
    assert_eq!(s.client.get_claim(&queued_id).unwrap().status, ClaimStatus::Reserved);
    assert!(s.client.get_stake(&w1).unwrap().active_claim_id.is_none());

    // A 2-of-2 override creates a DIFFERENT live claim for the same wallet.
    do_override(&s, &w1, &tx_hash(&env, 2), 1_000_000, TIER_C);
    let active_id = s.client.get_stake(&w1).unwrap().active_claim_id.unwrap();
    assert_ne!(active_id, queued_id);

    // Releasing the queued claim must now be refused outright.
    let result = s.client.try_try_release_queued_claim(&queued_id);
    assert_eq!(result, Err(Ok(PoolError::WalletHasDifferentActiveClaim)));
    // And it must not have been admitted: still Reserved, still no allocation
    // of its own beyond what the override legitimately took.
    assert_eq!(s.client.get_claim(&queued_id).unwrap().status, ClaimStatus::Reserved);
    assert_eq!(s.client.get_stake(&w1).unwrap().active_claim_id.unwrap(), active_id);
}

/// REGRESSION — same audit pass. An override that takes over the wallet's OWN
/// queued claim_id must release the queue slot with it. Otherwise the record
/// goes Active while `reserved_claim_id` still points at it, and
/// `expire_queued_claim` can never clear that pointer (it requires status ==
/// Reserved) — permanently blocking the wallet from ever queuing again.
#[test]
fn override_of_the_same_claim_id_clears_the_queue_slot() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s);
    let (_w2, _b2) = staked_wallet(&env, &s);

    let hash = tx_hash(&env, 1);
    let queued_id = submit_claim_signed(
        &env, &s, &s.oracle, &w1, &hash, &51_000_000, &TIER_C, &now_ts(&env),
    );
    assert_eq!(s.client.get_claim(&queued_id).unwrap().status, ClaimStatus::Reserved);
    assert_eq!(s.client.get_stake(&w1).unwrap().reserved_claim_id, Some(queued_id.clone()));

    // Override the SAME wallet+tx_hash, so it resolves to the same claim_id.
    do_override(&s, &w1, &hash, 1_000_000, TIER_C);

    let stake = s.client.get_stake(&w1).unwrap();
    assert_eq!(stake.active_claim_id, Some(queued_id.clone()));
    // The queue slot must be released, not left dangling.
    assert_eq!(stake.reserved_claim_id, None);

    // `reserved_claim_id == None` above IS the proof the wallet is not
    // soft-locked — deliberately not re-asserted via a fresh submit_claim
    // attempt: the override forfeits the stake (`withdrawn = true`), and
    // that guard sits EARLIER in submit_claim's validation order than the
    // ClaimAlreadyQueued guard, so such an attempt short-circuits on
    // AlreadyWithdrawn and proves nothing about the queue slot.
}

/// Isolates the SOLVENCY arm of `submit_claim`'s queue decision from the
/// stress-cap arm — the gap that let `total_allocated + entitlement >
/// total_staked` be mutated to `-` with every test still passing.
///
/// Pre-T3 this was covered for free: solvency returned `Err(Insolvent)`
/// immediately, so mutating it produced a DIFFERENT error and the test failed.
/// T3 merged both arms into one `insolvent || stress_capped` with a single
/// outcome, so the distinction is only observable when exactly one arm fires.
///
/// The window is narrow and worth writing down. `+` -> `-` flips the result
/// only when `allocated + e > staked` AND `allocated - e <= staked`; requiring
/// the stress arm to stay silent (`e <= stress_cap`) additionally forces
/// `stress_cap > headroom`, i.e. utilisation above 97% (rate is 300bps there,
/// so headroom must be under 3% of the pool). Hence: 98M allocated against
/// 100M staked, headroom 2M, stress cap 3M, entitlement 2.5M.
#[test]
fn submit_claim_queues_on_solvency_alone_with_the_stress_cap_silent() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s); // 100M
    let (w2, _b2) = staked_wallet(&env, &s); // 200M total_staked

    // Drive utilisation to 98%: the override forfeits w1's stake, taking
    // total_staked to 100M while reserving 98M.
    do_override(&s, &w1, &tx_hash(&env, 1), 98_000_000, TIER_C);
    assert_eq!(s.client.get_total_staked(), 100_000_000);
    assert_eq!(s.client.get_total_allocated(), 98_000_000);

    // Headroom is 2M; the stress cap at this utilisation is 3M (300bps).
    // 2.5M therefore breaches solvency while sitting comfortably UNDER the
    // stress cap — so only the solvency arm can be responsible for queuing.
    let entitlement = 2_500_000i128;
    let claim_id = submit_claim_signed(
        &env, &s, &s.oracle, &w2, &tx_hash(&env, 2), &entitlement, &TIER_C, &now_ts(&env),
    );

    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(
        claim.status,
        ClaimStatus::Reserved,
        "must queue on solvency alone — if this admits, the solvency arm is dead"
    );
    // Still not admitted: queuing reserves no capacity.
    assert_eq!(s.client.get_total_allocated(), 98_000_000);

    // The SAME window applies to the release-side solvency re-check
    // (claim.rs try_release_queued_claim). Nothing has changed, so releasing
    // must be refused — and refused on solvency, with the stress arm still
    // silent. A `+` -> `-` there would compute 98 - 2.5 = 95.5, conclude the
    // pool is solvent, and wrongly admit.
    assert_eq!(
        s.client.try_try_release_queued_claim(&claim_id),
        Err(Ok(PoolError::QueueReleaseNotYetEligible))
    );
    assert_eq!(s.client.get_claim(&claim_id).unwrap().status, ClaimStatus::Reserved);
    assert_eq!(s.client.get_total_allocated(), 98_000_000);
}

/// Both release-side comparisons are `>`, not `>=`: an entitlement that
/// EXACTLY fills the remaining solvency headroom AND exactly fills the day's
/// remaining stress cap must still be admitted.
///
/// Getting here needs care. A claim cannot simply be submitted at the
/// boundary — at exact fill `submit_claim` admits it outright, leaving no
/// queued state to release. It has to be queued while the pool is tighter,
/// then the pool GROWS into the boundary.
///
/// The numbers are chosen so both arms land on their line simultaneously:
/// allocated 194M against staked 200M is 97% utilisation, so the stress rate
/// is 300bps and the cap is exactly 6M — the same 6M that exactly fills
/// solvency (194 + 6 == 200). One test, both boundaries, and any `>` -> `>=`
/// or `>` -> `==` on either line turns this release into a refusal.
#[test]
fn queued_claim_releases_when_both_arms_sit_exactly_on_their_boundary() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s); // 100M
    let (w2, _b2) = staked_wallet(&env, &s); // 200M staked

    // Reserve 194M via override — forfeits w1's stake, so staked falls to
    // 100M while allocated becomes 194M.
    do_override(&s, &w1, &tx_hash(&env, 1), 194_000_000, TIER_C);
    assert_eq!(s.client.get_total_staked(), 100_000_000);
    assert_eq!(s.client.get_total_allocated(), 194_000_000);

    // 6M is deeply insolvent against 100M staked, so it queues.
    let entitlement = 6_000_000i128;
    let claim_id = submit_claim_signed(
        &env, &s, &s.oracle, &w2, &tx_hash(&env, 2), &entitlement, &TIER_C, &now_ts(&env),
    );
    assert_eq!(s.client.get_claim(&claim_id).unwrap().status, ClaimStatus::Reserved);

    // A fresh staker restores the pool to 200M, putting utilisation at 97%.
    staked_wallet(&env, &s);
    assert_eq!(s.client.get_total_staked(), 200_000_000);

    // Solvency:  194M + 6M >  200M  -> false under strict `>`  (exact fill)
    // Stress:      0M + 6M >   6M   -> false under strict `>`  (exact fill)
    s.client.try_release_queued_claim(&claim_id);

    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_ne!(
        claim.status,
        ClaimStatus::Reserved,
        "exact fill on both arms must release under strict `>`"
    );
    assert_eq!(s.client.get_total_allocated(), 200_000_000);
}

/// Release-side STRESS-CAP comparison is `>`, not `>=`: an entitlement that
/// exactly fills the day's remaining cap must be admitted. Solvency is kept
/// slack here so only the stress arm can be responsible.
#[test]
fn queued_claim_releases_at_the_exact_stress_cap_boundary() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s); // 100M -> stress cap 25M

    // 50M exceeds the 25M cap but not solvency (50 < 100), so it queues on
    // the stress arm alone.
    let claim_id = submit_claim_signed(
        &env, &s, &s.oracle, &w1, &tx_hash(&env, 1), &50_000_000, &TIER_C, &now_ts(&env),
    );
    assert_eq!(s.client.get_claim(&claim_id).unwrap().status, ClaimStatus::Reserved);

    // Doubling the pool puts the cap at exactly 50M (25% of 200M).
    staked_wallet(&env, &s);
    assert_eq!(s.client.get_total_staked(), 200_000_000);

    // 0 + 50M > 50M is false under strict `>`, so it releases. `>=` refuses.
    s.client.try_release_queued_claim(&claim_id);
    assert_ne!(s.client.get_claim(&claim_id).unwrap().status, ClaimStatus::Reserved);
    assert_eq!(s.client.get_total_allocated(), 50_000_000);
}

/// `yield_balance`'s FIRST term (`liquid + deployed`). The companion test
/// above runs with nothing deployed, so that `+` was unobservable — with a
/// live vault position it is not.
#[test]
fn yield_balance_counts_deployed_xlm_alongside_liquid() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s);
    let (_w2, _b2) = staked_wallet(&env, &s); // 200M staked
    with_vault(&env, &s, 5_000);

    // Move 50M into the vault: pool-controlled total is unchanged, only its
    // location moves. Yield must therefore still read 0.
    s.client.deploy_to_vault(&50_000_000, &0);
    assert_eq!(s.client.get_total_deployed_xlm(), 50_000_000);
    assert_eq!(s.client.get_yield_balance(), 0);

    // With a live claim: controls 200M, owes 100M staked + 40M allocated.
    do_override(&s, &w1, &tx_hash(&env, 1), 40_000_000, TIER_C);
    assert_eq!(s.client.get_yield_balance(), 60_000_000);
    // Deployed XLM is a real part of that total — subtracting it instead of
    // adding would halve the figure.
    assert!(s.client.get_liquid_balance() < 200_000_000);
    assert_invariant(&s);
}

// -----------------------------------------------------------------------
// total_allocated interaction — the condition every vault test was missing.
//
// Added 2026-08-24 after a cargo-mutants run (144 mutants, --in-diff scoped
// to the T3 changes) showed EVERY `total_allocated` term in the new code was
// mutable with zero test failures. Root cause was one mistake repeated: every
// vault test ran with no active claims, so `total_allocated == 0` and terms
// like `total_staked + total_allocated` / `liquid - total_allocated` were
// arithmetically identical under mutation. The functions were being tested
// with nothing owed — which is precisely the case they exist to handle.
// -----------------------------------------------------------------------

/// `get_yield_balance()` must subtract BOTH what is owed to stakers and what
/// is owed to already-approved claims. This is the live 2026-08-20 finding:
/// two activated claims made the getter read +4,100 XLM of "yield" that was
/// really the claimants' own forfeited principal.
///
/// The pre-existing coverage in `d2_vault_tests` all runs at
/// `total_allocated == 0`, where the T3 fix is a mathematical no-op — so the
/// fix shipped with no test exercising the case it was written for.
#[test]
fn yield_balance_excludes_principal_owed_to_active_claims() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s); // 100M
    let (_w2, _b2) = staked_wallet(&env, &s); // 200M total_staked

    // No claims yet, nothing donated: pool holds exactly what it owes.
    assert_eq!(s.client.get_yield_balance(), 0);

    // Activate a claim via override — forfeits w1's stake (total_staked
    // 200M -> 100M) while the XLM stays in the pool, now earmarked as
    // total_allocated. This is exactly the shape that produced the live
    // mislabel.
    let entitlement = 40_000_000i128;
    do_override(&s, &w1, &tx_hash(&env, 1), entitlement, TIER_C);

    assert_eq!(s.client.get_total_staked(), 100_000_000);
    assert_eq!(s.client.get_total_allocated(), entitlement);

    // Pool still physically holds all 200M; it owes 100M to stakers and 40M
    // to the claimant. The pre-fix formula (total_staked only) would report
    // 200M - 100M = 100M of phantom "yield". The correct answer nets out the
    // claim too: 200M - (100M + 40M) = 60M.
    let liquid = s.client.get_liquid_balance();
    assert_eq!(liquid, 200_000_000);
    assert_eq!(s.client.get_yield_balance(), 60_000_000);

    // A second live claim reduces it further, proving the term tracks
    // total_allocated rather than coincidentally matching once.
    let (w3, _b3) = staked_wallet(&env, &s); // +100M staked, +100M liquid
    do_override(&s, &w3, &tx_hash(&env, 2), 95_000_000, TIER_C);
    // liquid 300M; owes 100M to the remaining staker + 135M across both
    // claims => 300 - 235 = 65M.
    assert_eq!(s.client.get_total_allocated(), 135_000_000);
    assert_eq!(s.client.get_yield_balance(), 65_000_000);
    assert_invariant(&s);
}

/// `auto_deploy_liquidity` must treat claim-reserved XLM as untouchable.
/// Every prior test ran with `total_allocated == 0`, so `liquid -
/// total_allocated` was indistinguishable from `liquid + total_allocated`.
#[test]
fn auto_deploy_liquidity_will_not_deploy_xlm_owed_to_a_live_claim() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s); // 100M
    let (_w2, _b2) = staked_wallet(&env, &s); // 200M total_staked
    with_vault(&env, &s, 8_000); // 80% ceiling — deliberately generous

    // Bootstrap a reference rate with a small manual deposit.
    s.client.deploy_to_vault(&1_000_000, &0);

    // Reserve a large entitlement so total_allocated dominates.
    let entitlement = 40_000_000i128;
    do_override(&s, &w1, &tx_hash(&env, 1), entitlement, TIER_C);
    assert_eq!(s.client.get_total_allocated(), entitlement);

    let liquid_before = s.client.get_liquid_balance();
    let deployed_before = s.client.get_total_deployed_xlm();

    s.client.auto_deploy_liquidity();

    // The invariant that matters: liquid must never fall below what is owed
    // to live claims, no matter how much ceiling headroom exists.
    let liquid_after = s.client.get_liquid_balance();
    assert!(
        liquid_after >= entitlement,
        "deployed into claim-reserved XLM: liquid {} < allocated {}",
        liquid_after, entitlement
    );
    // It deployed exactly min(idle, ceiling room) — pins BOTH bounds rather
    // than asserting a direction. The ceiling binds here: total_staked fell
    // to 100M when the override forfeited w1's stake, so the 80% ceiling is
    // 80M against 1M already deployed.
    let idle = liquid_before - entitlement;
    let ceiling = s.client.get_total_staked() * 8_000 / 10_000;
    let room = ceiling - deployed_before;
    let deployed_delta = s.client.get_total_deployed_xlm() - deployed_before;
    assert_eq!(deployed_delta, idle.min(room));
    assert!(room < idle, "ceiling should be the binding constraint here");
    assert_invariant(&s);
}

/// `ensure_liquidity`'s claims-shortfall arm, pinned at the exact boundary.
/// `queued_claim_releases_once_capacity_frees` and friends leave solvency
/// wildly clear, so `>` vs `>=` at these comparisons was never probed.
#[test]
fn ensure_liquidity_pulls_exactly_to_the_allocated_line_no_further() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s);
    let (_w2, _b2) = staked_wallet(&env, &s);
    with_vault(&env, &s, 8_000);

    // Deploy modestly, then reserve a claim larger than what stays liquid.
    // 60M deployed leaves the post-override ratio (60M of 100M total_staked
    // at an 80% ceiling) INSIDE the line, so `over_ceiling` is zero and this
    // isolates the claims-shortfall arm — otherwise the ceiling arm dominates
    // and the assertion below would be measuring the wrong thing.
    s.client.deploy_to_vault(&60_000_000, &0);
    let entitlement = 150_000_000i128;
    do_override(&s, &w1, &tx_hash(&env, 1), entitlement, TIER_C);
    assert_eq!(s.client.get_total_allocated(), entitlement);
    assert!(s.client.get_liquid_balance() < entitlement);
    let ceiling = s.client.get_total_staked() * 8_000 / 10_000;
    assert!(s.client.get_total_deployed_xlm() <= ceiling, "ceiling arm must be idle");

    let deployed_before = s.client.get_total_deployed_xlm();
    s.client.ensure_liquidity();

    // Exactly the line, not past it: pulling more would be needless vault
    // churn, pulling less would leave the claim unpayable.
    assert_eq!(s.client.get_liquid_balance(), entitlement);
    assert!(s.client.get_total_deployed_xlm() < deployed_before);
    assert_invariant(&s);
}

/// The slippage floor must actually REJECT when the vault mints materially
/// fewer shares than the contract's own reference rate predicts.
///
/// This is the test the 1:1 mock could never express. `min_shares_out` is a
/// floor, and with a 1:1 rate the mock always delivered exactly the expected
/// count, so every mutation of the share-price arithmetic merely lowered a
/// floor that was being cleared anyway. Minting 10% below the reference rate
/// — beyond MAX_REBALANCE_SLIPPAGE_BPS (5%) — makes the floor load-bearing:
/// a mutated formula computes a nonsense expectation, fails to reject, and
/// this assertion catches it.
#[test]
fn auto_deploy_liquidity_rejects_a_deposit_below_the_slippage_floor() {
    let env = new_env();
    let s = setup(&env);
    staked_wallet(&env, &s);
    staked_wallet(&env, &s); // 200M staked
    let (_vault_id, mock) = with_vault(&env, &s, 8_000);

    // Bootstrap at 1:1, establishing the reference rate the contract will use.
    s.client.deploy_to_vault(&10_000_000, &0);
    assert_eq!(s.client.get_total_deployed_shares(), 10_000_000);
    assert_eq!(s.client.get_total_deployed_xlm(), 10_000_000);

    // Vault now mints 10% fewer shares per XLM than that reference — worse
    // than the 5% the contract is willing to tolerate.
    mock.set_deposit_rate_bps(&9_000);

    assert_eq!(
        s.client.try_auto_deploy_liquidity(),
        Err(Ok(PoolError::MinSharesNotMet))
    );
    // Rejected cleanly: nothing moved.
    assert_eq!(s.client.get_total_deployed_xlm(), 10_000_000);
    assert_invariant(&s);
}

/// A deposit inside the tolerance still succeeds, and the contract records the
/// shares the vault ACTUALLY minted rather than the count it predicted.
#[test]
fn auto_deploy_liquidity_accepts_within_tolerance_and_records_real_shares() {
    let env = new_env();
    let s = setup(&env);
    staked_wallet(&env, &s);
    staked_wallet(&env, &s);
    let (_vault_id, mock) = with_vault(&env, &s, 8_000);

    s.client.deploy_to_vault(&10_000_000, &0);
    // 2% below reference — inside the 5% bound.
    mock.set_deposit_rate_bps(&9_800);

    let shares_before = s.client.get_total_deployed_shares();
    let xlm_before = s.client.get_total_deployed_xlm();
    s.client.auto_deploy_liquidity();

    let xlm_delta = s.client.get_total_deployed_xlm() - xlm_before;
    let shares_delta = s.client.get_total_deployed_shares() - shares_before;
    assert!(xlm_delta > 0, "should have deployed something");
    // Shares tracked at what was minted (98%), not at the 1:1 amount.
    assert_eq!(shares_delta, xlm_delta * 9_800 / 10_000);
    assert_invariant(&s);
}

/// `try_release_queued_claim`'s solvency re-check is `>`, not `>=`: an
/// entitlement that EXACTLY fills the remaining solvency headroom must be
/// admitted. Prior release tests left solvency wildly clear, so the boundary
/// itself was never pinned and `>` -> `>=` / `==` survived mutation.
#[test]
fn queued_claim_releases_when_entitlement_exactly_fills_solvency_headroom() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s); // 100M
    let (_w2, _b2) = staked_wallet(&env, &s); // 200M total_staked

    // Queue something the day-1 stress cap refuses (25% of 200M = 50M).
    let entitlement = 60_000_000i128;
    let claim_id = submit_claim_signed(
        &env, &s, &s.oracle, &w1, &tx_hash(&env, 1), &entitlement, &TIER_C, &now_ts(&env),
    );
    assert_eq!(s.client.get_claim(&claim_id).unwrap().status, ClaimStatus::Reserved);

    // Grow the pool so BOTH arms clear, then release.
    staked_wallet(&env, &s);
    staked_wallet(&env, &s); // 400M staked -> stress cap 100M
    s.client.try_release_queued_claim(&claim_id);

    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_ne!(claim.status, ClaimStatus::Reserved);
    // Admitted exactly once, for exactly the pinned entitlement.
    assert_eq!(s.client.get_total_allocated(), entitlement);
    assert_eq!(claim.entitlement, entitlement);
}

// -----------------------------------------------------------------------
// Bidirectional liquidity rebalancing — ensure_liquidity (pull) /
// auto_deploy_liquidity (push) — and the shared total_staked shortfall fix.
// -----------------------------------------------------------------------

#[test]
fn ensure_liquidity_pulls_exactly_the_shortfall() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s); // 100M staked
    with_vault(&env, &s, 8_000); // 80% ceiling
    s.client.deploy_to_vault(&80_000_000, &0); // liquid now 20M

    // 21M entitlement exceeds current liquid (20M) but is within tier cap
    // and the 25M stress cap.
    let entitlement = 21_000_000;
    submit_claim_signed(
        &env, &s, &s.oracle, &w1, &tx_hash(&env, 1), &entitlement, &TIER_C, &now_ts(&env),
    );
    assert_eq!(s.client.get_total_allocated(), entitlement);
    assert!(s.client.get_liquid_balance() < entitlement);

    let shortfall = entitlement - s.client.get_liquid_balance();
    s.client.ensure_liquidity();

    assert_eq!(s.client.get_liquid_balance(), entitlement);
    assert_eq!(s.client.get_total_deployed_xlm(), 80_000_000 - shortfall);
    assert_invariant(&s);
}

/// The founder's case, 2026-08-24: stakers withdrawing shrinks `total_staked`
/// while `deployed_xlm` is unchanged, so the vault's SHARE of the pool climbs
/// above `deploy_bps` without a single new deployment. Previously documented
/// as drift needing an admin `provide_liquidity`; `ensure_liquidity` now
/// corrects it, making `deploy_bps` a genuine two-way line.
#[test]
fn ensure_liquidity_pulls_back_when_withdrawals_push_the_ratio_over_the_ceiling() {
    let env = new_env();
    let s = setup(&env);
    let (w1, b1) = staked_wallet(&env, &s); // 100M
    let (_w2, _b2) = staked_wallet(&env, &s); // 100M -> total_staked 200M
    with_vault(&env, &s, 5_000); // 50% ceiling

    // Deploy right up to the ceiling: 50% of 200M = 100M.
    s.client.deploy_to_vault(&100_000_000, &0);
    assert_eq!(s.client.get_deployment_ratio_bps(), 5_000);

    // w1 withdraws. total_staked 200M -> 100M, deployed_xlm still 100M, so
    // the ratio doubles to 100% — far above the 50% line — with no new
    // deployment having occurred.
    s.client.withdraw(&w1, &b1);
    assert_eq!(s.client.get_total_staked(), 100_000_000);
    assert!(s.client.get_deployment_ratio_bps() > 5_000);

    // No claims exist, so the claims-shortfall arm is zero. Pre-fix this
    // returned Ok(0) and left the drift in place.
    assert_eq!(s.client.get_total_allocated(), 0);
    s.client.ensure_liquidity();

    // Back EXACTLY on the configured line, self-corrected. Asserting the
    // precise figure rather than `<= ceiling`: a loose bound would let the
    // over-ceiling arithmetic be mutated (pull too much / too little) while
    // still landing somewhere under the line.
    let ceiling = s.client.get_total_staked() * 5_000 / 10_000;
    assert_eq!(s.client.get_total_deployed_xlm(), ceiling);
    assert_eq!(s.client.get_deployment_ratio_bps(), 5_000);
    assert_invariant(&s);
}

#[test]
fn ensure_liquidity_is_a_noop_when_nothing_is_short() {
    let env = new_env();
    let s = setup(&env);
    staked_wallet(&env, &s);
    with_vault(&env, &s, 8_000);
    // Nothing deployed, nothing allocated — liquid comfortably covers
    // total_allocated (0).
    assert_eq!(s.client.ensure_liquidity(), 0);
    assert_eq!(s.client.get_total_deployed_shares(), 0);
}

#[test]
fn ensure_liquidity_refuses_a_loss_beyond_the_slippage_bound() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s);
    let (_vault_id, mock) = with_vault(&env, &s, 8_000);
    s.client.deploy_to_vault(&80_000_000, &0);

    let entitlement = 21_000_000;
    submit_claim_signed(
        &env, &s, &s.oracle, &w1, &tx_hash(&env, 1), &entitlement, &TIER_C, &now_ts(&env),
    );
    assert!(s.client.get_liquid_balance() < entitlement);

    // 10% loss — beyond the 5% MAX_REBALANCE_SLIPPAGE_BPS bound.
    mock.set_rate_bps(&9_000);

    let result = s.client.try_ensure_liquidity();
    assert_eq!(result, Err(Ok(PoolError::MinAmountNotMet)));
    // Redeem reverts entirely on the floor — nothing partially happened.
    assert_eq!(s.client.get_total_deployed_xlm(), 80_000_000);
}

#[test]
fn redeem_shortfall_within_tolerance_marks_total_staked_down() {
    let env = new_env();
    let s = setup(&env);
    staked_wallet(&env, &s); // 100M staked
    let (_vault_id, mock) = with_vault(&env, &s, 8_000);
    s.client.deploy_to_vault(&50_000_000, &0);

    // A real 3% Blend loss — within the 5% bound, so this succeeds.
    mock.set_rate_bps(&9_700);

    let shares = s.client.get_total_deployed_shares(); // 50_000_000, 1:1 mint
    let staked_before = s.client.get_total_staked();
    s.client.provide_liquidity(&shares, &0);

    let xlm_received = shares * 9_700 / 10_000;
    let expected_shortfall = shares - xlm_received;
    assert_eq!(s.client.get_total_staked(), staked_before - expected_shortfall);
    assert_invariant(&s);
}

#[test]
fn auto_deploy_liquidity_requires_a_prior_manual_deposit() {
    let env = new_env();
    let s = setup(&env);
    staked_wallet(&env, &s);
    with_vault(&env, &s, 8_000);
    // No deploy_to_vault call ever made — no reference rate to bound
    // a deposit's slippage against yet.
    let result = s.client.try_auto_deploy_liquidity();
    assert_eq!(result, Err(Ok(PoolError::NothingDeployed)));
}

#[test]
fn auto_deploy_liquidity_pushes_idle_cash_up_to_the_ceiling() {
    let env = new_env();
    let s = setup(&env);
    staked_wallet(&env, &s); // 100M staked
    with_vault(&env, &s, 5_000); // 50% ceiling

    // Bootstrap: one manual deposit gives auto_deploy_liquidity a
    // reference rate to check its own deposit against.
    s.client.deploy_to_vault(&10_000_000, &0);

    // More stake arrives, building up idle liquidity beyond what's needed.
    staked_wallet(&env, &s); // total_staked now 200M, ceiling now 100M

    let idle_before = s.client.get_liquid_balance() - s.client.get_total_allocated();
    let room = 100_000_000 - s.client.get_total_deployed_xlm();
    let expected_deploy = idle_before.min(room);

    s.client.auto_deploy_liquidity();

    assert_eq!(s.client.get_total_deployed_xlm(), 10_000_000 + expected_deploy);
    assert_invariant(&s);
}

// -----------------------------------------------------------------------
// Mutation-gap closures — added 2026-08-24 after the full 144-mutant
// campaign (134 caught / 9 missed / 1 unviable). Six of the nine misses
// were real gaps, all in the liquidity-rebalancing pair. Root cause of the
// skew: the audit pass hand-wrote seven tests for `ensure_liquidity` and
// never gave `auto_deploy_liquidity` the same treatment.
//
// The remaining three misses are provably equivalent and are documented as
// exclusions in `.cargo/mutants.toml` instead.
// -----------------------------------------------------------------------

/// Kills `vault.rs:542` (`liquid - total_allocated` -> `+`) and BOTH
/// `vault.rs:563:24` mutants (`<` -> `==`, `<` -> `<=`).
///
/// `auto_deploy_liquidity_will_not_deploy_xlm_owed_to_a_live_claim` asserts
/// `room < idle` — the CEILING binds there, so `amount = room` and `idle`'s
/// value never reaches the outcome. That made the `-`/`+` mutation on idle
/// invisible, and kept `liquid - amount` strictly ABOVE `total_allocated`,
/// so the `==`/`<=` mutants on the allocation guard never fired either.
///
/// Here idle binds instead. The trick: reserve MORE than the stake the
/// override forfeits. Forfeiture shrinks `total_staked` (and with it the
/// ceiling) while the XLM itself stays in the pool, so a large enough
/// entitlement pushes idle below ceiling room.
#[test]
fn auto_deploy_liquidity_deploys_exactly_idle_when_idle_is_the_binding_constraint() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s); // 100M
    let (_w2, _b2) = staked_wallet(&env, &s); // 200M total_staked
    with_vault(&env, &s, 8_000); // 80% ceiling (MAX_DEPLOY_BPS) — must NOT bind

    s.client.deploy_to_vault(&1_000_000, &0); // 1:1 reference rate

    let entitlement = 150_000_000i128; // > the 100M w1 forfeits
    do_override(&s, &w1, &tx_hash(&env, 1), entitlement, TIER_C);
    assert_eq!(s.client.get_total_allocated(), entitlement);

    let liquid_before = s.client.get_liquid_balance();
    let deployed_before = s.client.get_total_deployed_xlm();
    let idle = liquid_before - entitlement;
    let room = s.client.get_total_staked() * 8_000 / 10_000 - deployed_before;
    assert!(
        idle < room,
        "idle must be the binding constraint here: idle {} room {}",
        idle, room
    );

    s.client.auto_deploy_liquidity();

    // Exact, not an inequality — an inequality is what let the `+` mutant live.
    assert_eq!(s.client.get_total_deployed_xlm() - deployed_before, idle);
    // Deploying exactly idle lands liquid precisely ON total_allocated, which
    // is the state that separates `<` from `==`/`<=` at the allocation guard.
    assert_eq!(s.client.get_liquid_balance(), entitlement);
    assert_invariant(&s);
}

/// Kills `vault.rs:569:34` (`amount * deployed_shares` -> `+`).
///
/// The mutation parses as `amount + (deployed_shares / deployed_xlm)` — a
/// SUM, not a re-grouped quotient. With the 1:1 bootstrap every existing
/// test uses, `deployed_shares / deployed_xlm` is 1, so the mutant computes
/// `amount + 1` and `min_shares_out` moves by less than one stroop, which
/// integer truncation erases entirely. That is why even the existing
/// below-the-floor test at a 9_000 rate could not kill it.
///
/// Making the reference RATIO load-bearing (5 shares per XLM, not 1) is what
/// separates them: the original expects `5 * amount`, the mutant expects
/// `amount + 5`.
#[test]
fn auto_deploy_liquidity_rejects_when_share_price_collapses_against_the_reference() {
    let env = new_env();
    let s = setup(&env);
    staked_wallet(&env, &s);
    staked_wallet(&env, &s);
    let (_vault_id, mock) = with_vault(&env, &s, 8_000);

    // Bootstrap at 5 shares per XLM — this ratio becomes the reference.
    mock.set_deposit_rate_bps(&50_000);
    s.client.deploy_to_vault(&1_000_000, &0);
    assert_eq!(s.client.get_total_deployed_shares(), 5_000_000);
    assert_eq!(s.client.get_total_deployed_xlm(), 1_000_000);

    // Vault now mints 1 share per XLM — 80% below the reference, far beyond
    // the 5% MAX_REBALANCE_SLIPPAGE_BPS the contract tolerates.
    mock.set_deposit_rate_bps(&10_000);

    assert_eq!(
        s.client.try_auto_deploy_liquidity(),
        Err(Ok(PoolError::MinSharesNotMet))
    );
    // Rejected cleanly: nothing moved.
    assert_eq!(s.client.get_total_deployed_xlm(), 1_000_000);
    assert_invariant(&s);
}

/// Kills `vault.rs:591:22` (`shares_gained < min_shares_out` -> `<=`).
///
/// Minting at exactly MAX_REBALANCE_SLIPPAGE_BPS below the reference makes
/// `shares_gained` land precisely ON `min_shares_out` — both sides evaluate
/// the identical `amount * 9_500 / 10_000` expression, so they are equal
/// regardless of divisibility. `<` must ACCEPT that; `<=` rejects it.
#[test]
fn auto_deploy_liquidity_accepts_shares_exactly_at_the_slippage_floor() {
    let env = new_env();
    let s = setup(&env);
    staked_wallet(&env, &s);
    staked_wallet(&env, &s);
    let (_vault_id, mock) = with_vault(&env, &s, 8_000);

    s.client.deploy_to_vault(&1_000_000, &0); // 1:1 reference

    mock.set_deposit_rate_bps(&9_500); // exactly on the tolerance line

    let shares_before = s.client.get_total_deployed_shares();
    let xlm_before = s.client.get_total_deployed_xlm();

    s.client.auto_deploy_liquidity(); // must NOT panic — the floor is inclusive

    let xlm_delta = s.client.get_total_deployed_xlm() - xlm_before;
    let shares_delta = s.client.get_total_deployed_shares() - shares_before;
    assert!(xlm_delta > 0, "should have deployed something");
    assert_eq!(shares_delta, xlm_delta * 9_500 / 10_000);
    assert_invariant(&s);
}

/// Kills `vault.rs:780:36` — the `*` in
/// `min_xlm_out = expected_xlm * (BPS_DENOMINATOR - MAX_REBALANCE_SLIPPAGE_BPS)
/// / BPS_DENOMINATOR`.
///
/// The mutation parses as `expected_xlm + ((10_000 - 500) / 10_000)`, and
/// that quotient truncates to 0 — so the mutant's floor is `expected_xlm`
/// ITSELF. In other words it silently demands 100% of the expected proceeds
/// where the contract means to tolerate a 5% shortfall.
///
/// Killing it needs a redeem landing strictly INSIDE that 5% band: at or
/// above `0.95 * expected_xlm` (original accepts) but below `expected_xlm`
/// (mutant rejects). Any healthier redeem clears both floors and the
/// mutation is invisible — which is exactly how the first version of this
/// test, returning 250% of expected, let it survive.
///
/// A partial redeem against a non-1:1 reference sets the band up: 5 shares
/// per XLM deployed, 1M shares redeemed => expected_xlm 200_000, so the
/// original floor is 190_000 and the mutant's is 200_000. A 1_940 bps
/// withdraw rate returns 194_000 — inside the band.
#[test]
fn ensure_liquidity_pulls_a_partial_tranche_priced_off_the_reference_ratio() {
    let env = new_env();
    let s = setup(&env);
    let (w1, _b1) = staked_wallet(&env, &s); // 100M
    let (_w2, _b2) = staked_wallet(&env, &s); // 200M total_staked
    let (_vault_id, mock) = with_vault(&env, &s, 8_000);

    // 5 shares per XLM reference: deployed_xlm 1M, deployed_shares 5M.
    mock.set_deposit_rate_bps(&50_000);
    s.client.deploy_to_vault(&1_000_000, &0);
    assert_eq!(s.client.get_total_deployed_shares(), 5_000_000);

    // Reserve just past liquid so the CLAIMS arm drives a small shortfall.
    // Forfeiting w1 leaves total_staked at 100M, so the 80% ceiling (80M)
    // still sits above deployed_xlm (1M) — the over-ceiling arm stays at 0
    // and does not mask the shortfall we are pinning.
    let liquid = s.client.get_liquid_balance(); // 199M
    let entitlement = liquid + 200_000;
    do_override(&s, &w1, &tx_hash(&env, 1), entitlement, TIER_C);
    assert_eq!(s.client.get_total_allocated(), entitlement);

    // Position came back 3% light — inside the 5% the contract tolerates.
    mock.set_rate_bps(&1_940);

    // shortfall 200_000 -> shares_needed = 200_000 * 5M / 1M = 1M (partial,
    // against 5M deployed) -> expected_xlm = 1M * 1M / 5M = 200_000, so the
    // real floor is 190_000 and the mutant's is 200_000.
    // xlm_received = 1M * 0.194 = 194_000: clears the real floor, fails the
    // mutant's.
    let received = s.client.ensure_liquidity();
    assert_eq!(received, 194_000);
    assert_invariant(&s);
}
