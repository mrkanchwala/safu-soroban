//! T2 mutation-gap tests (2026-08-25).
//!
//! Closes gaps found by the first mutation campaign ever run against the
//! T1→T2 graduation. That campaign was `--in-diff` scoped to the T1→T2
//! contract diff and returned 404 mutants: 372 caught, 22 missed, 10
//! unviable. 19 of the 22 sat in `vault.rs`, the DeFindex yield module added
//! in T2 — every one of them on the capital-movement path.
//!
//! Each test below names the exact mutant(s) it kills by `file:line:col`, and
//! every one was verified by hand-injecting the mutation into the real source
//! and confirming the test fails, then restoring and checksumming. A test
//! that merely executes the line is not enough — it has to pin the value.

#![cfg(test)]

use soroban_sdk::testutils::Address as _;
use soroban_sdk::testutils::Events as _;
use soroban_sdk::{Address, Env};

use crate::error::PoolError;
use crate::test::common::*;
use crate::test::d2_vault_tests::{MockVault, MockVaultClient};
use crate::types::MAX_APPROVAL_WINDOW_SECONDS;

// Local, per the convention every other test module in this suite follows.
const ENTITLEMENT: i128 = 1_000_000;
const TIER_C: u32 = 3;

/// Local copy of d2_vault_tests' private setup helper — registers a mock
/// vault, wires it in, and opens deployment to `deploy_bps`.
fn with_vault<'a>(env: &'a Env, s: &Setup<'a>, deploy_bps: i128) -> (Address, MockVaultClient<'a>) {
    let vault_id = env.register(MockVault, ());
    let mock = MockVaultClient::new(env, &vault_id);
    mock.init(&s.token_id);
    s.client.set_vault(&vault_id);
    s.client.set_deploy_bps(&deploy_bps);
    (vault_id, mock)
}

// -----------------------------------------------------------------------
// vault.rs — set_deploy_bps bounds
// -----------------------------------------------------------------------

/// Kills `vault.rs:251:12: replace < with <=` and `replace < with ==`.
///
/// The guard is `if bps < 0 { Err(AmountNotPositive) }`. Zero is a LEGAL
/// setting — it closes deployment without unwinding. Both mutants reject it:
/// `<=` errors on zero directly, and `==` errors on zero while also letting a
/// NEGATIVE bps through to be written to storage. No existing test set bps to
/// zero, so the whole lower bound was unpinned.
#[test]
fn set_deploy_bps_accepts_exactly_zero() {
    let env = new_env();
    let s = setup(&env);

    s.client.set_deploy_bps(&0);

    assert_eq!(s.client.get_deploy_bps(), 0, "zero is a legal deploy_bps");
}

/// Kills `vault.rs:251:12: replace < with ==` independently of the zero case.
///
/// With `==`, a negative bps does not match and falls through the upper-bound
/// check to be written to storage. This asserts the typed error rather than
/// just "it failed".
#[test]
fn set_deploy_bps_rejects_negative_with_a_typed_error() {
    let env = new_env();
    let s = setup(&env);

    assert_eq!(
        s.client.try_set_deploy_bps(&-1),
        Err(Ok(PoolError::AmountNotPositive))
    );
    assert_ne!(s.client.get_deploy_bps(), -1, "a negative bps must never be stored");
}

// -----------------------------------------------------------------------
// vault.rs — yield_balance arithmetic
// -----------------------------------------------------------------------

/// Kills `vault.rs:188:37: replace + with -`.
///
/// `yield_balance` is `(liquid + deployed) - staked`. Every existing test
/// exercised it with `deployed == 0`, where `liquid + 0` and `liquid - 0` are
/// identical — so the operator was never observable. This forces deployed
/// principal to be large enough that the sign of the term decides the result.
#[test]
fn yield_balance_counts_deployed_principal_not_just_liquid() {
    let env = new_env();
    let s = setup(&env);
    let (_staker, _ben) = staked_wallet_amount(&env, &s, 1_000_000_000);

    // Surplus above staker principal — this is the yield the pool controls.
    let surplus = 100_000_000i128;
    s.token_admin.mint(&s.contract_id, &surplus);

    let (_vault, _mock) = with_vault(&env, &s, 8_000);
    let deployed = 800_000_000i128;
    s.client.deploy_to_vault(&deployed, &0);

    // Original: (liquid + deployed) - staked == surplus.
    // Mutant:   (liquid - deployed) - staked, which is deeply negative here
    //           and clamps to 0.
    assert_eq!(
        s.client.get_yield_balance(),
        surplus,
        "yield must count deployed principal, not net it out"
    );
    assert!(s.client.get_total_deployed_xlm() > 0, "test is vacuous without deployed principal");
}

// -----------------------------------------------------------------------
// vault.rs — withdraw_yield double gate + accumulator
// -----------------------------------------------------------------------

/// Kills FOUR mutants in one exercise:
///   `vault.rs:631:15: replace > with >=`  and  `replace > with ==`
///   `vault.rs:635:85: replace + with -`   and  `replace + with *`
///
/// Line 631 is the second half of V8's double gate — the check the source's
/// own comment calls "what stops treasury withdrawals from eating staker
/// principal". It ALLOWS `amount == liquid_balance`; both comparison mutants
/// reject it. Line 635 accumulates into `total_extracted_yield`, and no test
/// had ever read that counter back, so `+` could become `-` or `*` unseen.
///
/// NOTE: `vault.rs:631` was additionally being SUPPRESSED by a stale
/// `.cargo/mutants.toml` entry — `"replace > with >= in withdraw"` is a
/// substring match and `withdraw_yield` contains `withdraw`. The exclusion
/// was written about `stake.rs`'s `withdraw`. Anchor corrected 2026-08-25.
#[test]
fn withdraw_yield_allows_exactly_the_liquid_balance_and_accumulates_it() {
    let env = new_env();
    let s = setup(&env);
    let treasury = Address::generate(&env);
    s.client.set_treasury(&treasury);

    // No stakers, so every stroop the pool holds is withdrawable yield and
    // `amount == yield_balance == liquid_balance` exactly.
    let surplus = 50_000_000i128;
    s.token_admin.mint(&s.contract_id, &surplus);
    assert_eq!(s.client.get_yield_balance(), surplus);
    assert_eq!(s.client.get_liquid_balance(), surplus);

    s.client.withdraw_yield(&surplus);

    assert_eq!(
        s.client.get_total_extracted_yield(),
        surplus,
        "the extracted-yield counter must accumulate by exactly the amount sent"
    );
    assert_eq!(s.client.get_liquid_balance(), 0);
}

// -----------------------------------------------------------------------
// claim.rs — revoke_approval window boundaries
// -----------------------------------------------------------------------

/// Kills `claim.rs:1237:15: replace > with >=`.
///
/// The guard is `if now_ts > deadline { SignatureExpired }`, so a deadline
/// landing exactly on the current timestamp is still VALID. The `>=` mutant
/// rejects it. `submit_claim` got its exact-boundary test when the approval
/// window was added; `revoke_approval` never got the same treatment — the
/// same sibling-function asymmetry that produced 7 of T3's 9 misses.
#[test]
fn revoke_approval_accepts_a_deadline_exactly_at_now() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    let txh = tx_hash(&env, 7);
    let hack = now_ts(&env);
    let deadline = now_ts(&env); // exactly on the boundary

    s.client.revoke_approval(
        &s.admin, &staker, &txh, &ENTITLEMENT, &TIER_C, &hack, &deadline,
    );
}

/// Kills `claim.rs:1240:17: replace > with >=`.
///
/// The guard is `if deadline > now_ts + MAX_APPROVAL_WINDOW_SECONDS`, so a
/// deadline exactly one full window out is the LAST valid value. The `>=`
/// mutant rejects it, shortening the usable window by one second with no test
/// to notice.
#[test]
fn revoke_approval_accepts_a_deadline_exactly_at_the_window_edge() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    let txh = tx_hash(&env, 8);
    let hack = now_ts(&env);
    let deadline = now_ts(&env) + MAX_APPROVAL_WINDOW_SECONDS; // exactly the edge

    s.client.revoke_approval(
        &s.admin, &staker, &txh, &ENTITLEMENT, &TIER_C, &hack, &deadline,
    );
}

// -----------------------------------------------------------------------
// vault.rs — deploy_to_vault liquidity gate + share-delta accounting
// -----------------------------------------------------------------------

/// Kills `vault.rs:413:15: replace < with ==`.
///
/// The guard is `if liquid < amount { InsufficientLiquidity }`. With `==`, a
/// request for strictly MORE than the liquid balance no longer matches and
/// falls through to the ceiling and allocation checks, which report a
/// different reason. Existing tests only asserted that over-deployment
/// failed, never WHY, so the two were interchangeable.
#[test]
fn deploy_over_the_liquid_balance_reports_insufficient_liquidity_specifically() {
    let env = new_env();
    let s = setup(&env);
    let (_staker, _ben) = staked_wallet_amount(&env, &s, 1_000_000_000);
    let (_vault, _mock) = with_vault(&env, &s, 8_000);

    // Strictly greater than liquid, and also over the ceiling — so the
    // mutant reaches a DIFFERENT typed error rather than succeeding.
    assert_eq!(
        s.client.try_deploy_to_vault(&2_000_000_000, &0),
        Err(Ok(PoolError::InsufficientLiquidity))
    );
}

/// Kills `vault.rs:452:72: replace - with +`.
///
/// `shares_gained` is a DELTA: `balance_after - shares_before`. On a pool's
/// first-ever deploy `shares_before` is 0, where `after - 0` and `after + 0`
/// are identical — which is every existing deploy test. A second deploy makes
/// the operator observable for the first time.
#[test]
fn repeated_deploys_measure_shares_as_a_delta_not_a_sum() {
    let env = new_env();
    let s = setup(&env);
    let (_staker, _ben) = staked_wallet_amount(&env, &s, 1_000_000_000);
    let (_vault, _mock) = with_vault(&env, &s, 8_000);

    let tranche = 400_000_000i128;
    s.client.deploy_to_vault(&tranche, &0);
    s.client.deploy_to_vault(&tranche, &0);

    // Mock mints 1:1, so two equal tranches must total exactly 2x.
    assert_eq!(
        s.client.get_total_deployed_shares(),
        tranche * 2,
        "second deploy must add only its own delta, not re-add the prior balance"
    );
}

// -----------------------------------------------------------------------
// vault.rs — redeem slippage floor + shortfall detection
// -----------------------------------------------------------------------

/// Kills `vault.rs:513:21: replace < with <=`.
///
/// The floor is `if xlm_received < min_xlm_out { MinAmountNotMet }`, so
/// receiving EXACTLY the minimum is a success. The `<=` mutant rejects it,
/// silently making the bound exclusive.
#[test]
fn redeem_accepts_proceeds_landing_exactly_on_the_min_out_floor() {
    let env = new_env();
    let s = setup(&env);
    let (_staker, _ben) = staked_wallet_amount(&env, &s, 1_000_000_000);
    let (_vault, _mock) = with_vault(&env, &s, 8_000);
    s.client.deploy_to_vault(&800_000_000, &0);

    // Rate is 1:1, so redeeming N shares returns exactly N XLM. Asking for
    // exactly N as the floor puts us precisely on the boundary.
    let tranche = 100_000_000i128;
    s.client.provide_liquidity(&tranche, &tranche);
}

/// Kills all THREE surviving mutants on `vault.rs:520:21`:
///   `replace < with ==`, `replace < with >`, `replace < with <=`
///
/// Line 520 gates the `DeploymentShortfall` event — the contract's only
/// detection of a venue losing staker principal. The event was emitted but
/// asserted NOWHERE, so the comparison was free to become anything, including
/// a full inversion.
///
/// Rather than decode XDR, this compares event-count DELTAS between a par
/// redemption and a loss redemption, which are otherwise identical. Original:
/// par emits nothing, loss emits one, so loss == par + 1. `==` and `<=` both
/// fire on par; `>` fires on neither. Every mutant breaks the relation.
#[test]
fn deployment_shortfall_fires_on_a_loss_and_stays_silent_at_par() {
    let env = new_env();
    let s = setup(&env);
    let (_staker, _ben) = staked_wallet_amount(&env, &s, 1_000_000_000);
    let (_vault, mock) = with_vault(&env, &s, 8_000);
    s.client.deploy_to_vault(&800_000_000, &0);

    let tranche = 100_000_000i128;

    // Par: proceeds exactly equal proportional principal.
    mock.set_rate_bps(&10_000);
    let before_par = env.events().all().events().len();
    s.client.provide_liquidity(&tranche, &0);
    let par_delta = env.events().all().events().len() - before_par;

    // Loss: the venue returns less than principal.
    mock.set_rate_bps(&9_000);
    let before_loss = env.events().all().events().len();
    s.client.provide_liquidity(&tranche, &0);
    let loss_delta = env.events().all().events().len() - before_loss;

    assert_eq!(
        loss_delta,
        par_delta + 1,
        "a loss must emit exactly one DeploymentShortfall that par does not"
    );
}

/// Kills `vault.rs:413:15: replace < with <=`.
///
/// Requesting EXACTLY the liquid balance passes the liquidity gate — it is
/// the ceiling, one check later, that refuses it. The `<=` mutant short-
/// circuits at the liquidity gate instead, so the caller is told the pool is
/// illiquid when in fact it is over its deployment ceiling. Both paths fail,
/// which is why no existing test noticed; only the typed error separates them.
///
/// The exact-equality case is deliberately asserted through the ceiling
/// rather than through a success, because with `MAX_DEPLOY_BPS = 8_000` a
/// deploy of the full liquid balance can never clear the ceiling: liquid
/// always exceeds remaining room by at least 20% of staked principal.
#[test]
fn deploying_exactly_the_liquid_balance_is_refused_by_the_ceiling_not_the_liquidity_gate() {
    let env = new_env();
    let s = setup(&env);
    let (_staker, _ben) = staked_wallet_amount(&env, &s, 1_000_000_000);
    let (_vault, _mock) = with_vault(&env, &s, 8_000);

    let liquid = s.client.get_liquid_balance();
    assert_eq!(liquid, 1_000_000_000, "precondition: nothing deployed yet");

    assert_eq!(
        s.client.try_deploy_to_vault(&liquid, &0),
        Err(Ok(PoolError::DeployExceedsCeiling)),
        "exactly-liquid must clear the liquidity gate and be stopped by the ceiling"
    );
}

/// Kills `vault.rs:606:21: replace > with >=`.
///
/// The guard is `if yield_amount > 0 { token.transfer(treasury, yield) }`.
/// With `>=`, a par redemption transfers ZERO to treasury — economically
/// inert, but it emits a real token-transfer event, so the two are
/// distinguishable. Nothing asserted treasury traffic on the zero-yield path.
///
/// Compares event deltas between a par extraction and a gaining one, which
/// differ by exactly that transfer.
#[test]
fn extract_yield_makes_no_treasury_transfer_when_there_is_no_yield() {
    let env = new_env();
    let s = setup(&env);
    let (_staker, _ben) = staked_wallet_amount(&env, &s, 1_000_000_000);
    let (vault_id, mock) = with_vault(&env, &s, 8_000);
    let treasury = Address::generate(&env);
    s.client.set_treasury(&treasury);
    s.client.deploy_to_vault(&800_000_000, &0);

    let tranche = 400_000_000i128;

    // NOTE: the test env's event buffer is per-INVOCATION, not cumulative, so
    // the absolute count after each call is that call's own event set. An
    // earlier draft subtracted a "before" reading and was silently wrong —
    // the intervening `mint` had already reset the buffer.

    // Par: principal comes home, nothing is owed to treasury.
    mock.set_rate_bps(&10_000);
    assert_eq!(s.client.extract_yield(&tranche, &0), 0);
    let par_events = env.events().all().events().len();

    // Gain: the excess above principal is transferred.
    mock.set_rate_bps(&11_000);
    s.token_admin.mint(&vault_id, &tranche);
    assert!(s.client.extract_yield(&tranche, &0) > 0);
    let gain_events = env.events().all().events().len();

    assert_eq!(
        gain_events,
        par_events + 1,
        "a zero-yield extraction must not touch the token contract at all"
    );
}

// -----------------------------------------------------------------------
// vault.rs:375 — authorize_withdraw: NOT CLOSED, and deliberately not faked
// -----------------------------------------------------------------------
//
// `vault.rs:375:5: replace authorize_withdraw with ()` survives, and no test
// in this file closes it. Stubbing the entire function body leaves all 250
// tests passing.
//
// `authorize_withdraw` is real, necessary production code — it supplies the
// invoker-contract authorization for the DeFindex vault's withdraw-side
// `from.require_auth()`, whose shape was verified against the live testnet
// vault on 2026-08-14. Its removal would break a real redemption on chain.
//
// FOUR approaches were tried and all fail, for one structural reason: exposing
// this authorization requires auth mocking to be OFF, but turning it off also
// breaks the ADMIN's own `require_auth` — `Address::generate()` yields a
// CONTRACT-type address, which can only authorize via a `__check_auth`
// implementation it does not have. The test then dies before it ever reaches
// the vault call, identically with and without the mutation, so it cannot
// distinguish them.
//
//   1. The existing suite — `common::new_env()` calls `env.mock_all_auths()`,
//      which DOES satisfy this authorization.
//   2. `mock_auths` scoped to the admin's root invocation with
//      `sub_invokes: &[]`. Passed both ways: `mock_auths` constrains ADDRESS
//      auth but leaves invoker-contract auth mocked.
//   3. Asserting on `env.auths()`. Byte-identical with and without the stub —
//      only the admin's root entry is recorded; invoker-contract auth never
//      appears there at all.
//   4. `env.set_auths` with a hand-built `SorobanAuthorizationEntry`, under
//      both `SorobanCredentials::Address` and `::SourceAccount`. `set_auths`
//      genuinely does override `mock_all_auths` (an empty set makes the call
//      fail), but the admin's auth is then unsatisfiable per the above.
//
// The only remaining route is an account-type (G-strkey) admin plus a parallel
// setup path — a change to the test harness itself, not a new test.
//
// Disposition: this is a HARNESS limitation, not an equivalent mutant. It is
// deliberately left unexcluded so it keeps reporting as a survivor rather
// than being silently absorbed. Closing it needs integration-level coverage
// against a real vault, which T3's testnet deploy already schedules.
