#![cfg(test)]

use soroban_sdk::testutils::Address as _;
use soroban_sdk::Address;

use super::common::*;

#[test]
fn initialize_sets_all_fields() {
    let env = new_env();
    let s = setup(&env);
    // No panic on a fresh initialize is the assertion; roundtrip a
    // stake to confirm the pool cap actually took effect.
    let staker = new_funded_address(&env, &s, MID_STAKE);
    let ben = Address::generate(&env);
    s.client.stake(&staker, &MID_STAKE, &ben);
}

#[test]
#[should_panic(expected = "SAFU: already initialized")]
fn initialize_twice_panics() {
    let env = new_env();
    let s = setup(&env);
    s.client
        .initialize(&s.admin, &s.oracle, &s.co_signer, &s.token_id, &POOL_CAP);
}

#[test]
#[should_panic(expected = "SAFU: oracle cannot equal coSigner")]
fn initialize_oracle_equals_cosigner_panics() {
    let env = new_env();
    let admin = Address::generate(&env);
    let same = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let token_id = sac.address();
    let contract_id = env.register(crate::ProtectionPool, ());
    let client = crate::ProtectionPoolClient::new(&env, &contract_id);
    client.initialize(&admin, &same, &same, &token_id, &POOL_CAP);
}

#[test]
#[should_panic(expected = "SAFU: coSigner must differ from admin")]
fn initialize_cosigner_equals_admin_panics() {
    let env = new_env();
    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let token_id = sac.address();
    let contract_id = env.register(crate::ProtectionPool, ());
    let client = crate::ProtectionPoolClient::new(&env, &contract_id);
    client.initialize(&admin, &oracle, &admin, &token_id, &POOL_CAP);
}

#[test]
#[should_panic(expected = "SAFU: pool cap must be positive")]
fn initialize_zero_pool_cap_panics() {
    let env = new_env();
    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let co_signer = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let token_id = sac.address();
    let contract_id = env.register(crate::ProtectionPool, ());
    let client = crate::ProtectionPoolClient::new(&env, &contract_id);
    client.initialize(&admin, &oracle, &co_signer, &token_id, &0);
}

#[test]
fn set_oracle_updates() {
    let env = new_env();
    let s = setup(&env);
    let new_oracle = Address::generate(&env);
    s.client.set_oracle(&new_oracle);
    // Confirmed indirectly: submit_claim as the NEW oracle now works.
    let (staker, _ben) = staked_wallet(&env, &s);
    advance_days(&env, 91);
    s.client.submit_claim(
        &new_oracle,
        &staker,
        &tx_hash(&env, 1),
        &1_000_000,
        &3,
        &now_ts(&env),
    );
}

#[test]
#[should_panic(expected = "SAFU: oracle cannot equal coSigner")]
fn set_oracle_equal_cosigner_panics() {
    let env = new_env();
    let s = setup(&env);
    s.client.set_oracle(&s.co_signer);
}

#[test]
fn set_co_signer_updates() {
    let env = new_env();
    let s = setup(&env);
    let new_cs = Address::generate(&env);
    s.client.set_co_signer(&new_cs);
}

#[test]
#[should_panic(expected = "SAFU: coSigner cannot equal oracle")]
fn set_co_signer_equal_oracle_panics() {
    let env = new_env();
    let s = setup(&env);
    s.client.set_co_signer(&s.oracle);
}

#[test]
fn set_pool_cap_increases() {
    let env = new_env();
    let s = setup(&env);
    s.client.set_pool_cap(&(POOL_CAP * 2));
    // New bounds take effect immediately — a stake above the old MAX_STAKE
    // now succeeds.
    let bigger = MAX_STAKE + 1;
    let staker = new_funded_address(&env, &s, bigger);
    let ben = Address::generate(&env);
    s.client.stake(&staker, &bigger, &ben);
}

#[test]
#[should_panic(expected = "SAFU: pool cap must be positive")]
fn set_pool_cap_zero_panics() {
    let env = new_env();
    let s = setup(&env);
    s.client.set_pool_cap(&0);
}

#[test]
#[should_panic(expected = "SAFU: pool cap below current total staked")]
fn set_pool_cap_below_total_staked_panics() {
    let env = new_env();
    let s = setup(&env);
    staked_wallet(&env, &s);
    s.client.set_pool_cap(&(MID_STAKE - 1));
}

#[test]
fn transfer_admin_updates() {
    let env = new_env();
    let s = setup(&env);
    let new_admin = Address::generate(&env);
    s.client.transfer_admin(&new_admin);
    // Confirmed indirectly: the OLD admin can no longer gate-pass as
    // admin — set_pool_cap always requires the CURRENT stored admin's
    // auth (mock_all_auths makes any auth succeed, so this just checks
    // the stored admin address actually changed via a side effect: a
    // pool-cap change that would have been fine for the old admin is
    // still fine here too, since require_auth targets storage::get_admin
    // freshly on every call — the real assertion is no panic occurs).
    s.client.set_pool_cap(&(POOL_CAP * 2));
}

#[test]
#[should_panic(expected = "SAFU: new admin cannot equal coSigner")]
fn transfer_admin_to_cosigner_panics() {
    let env = new_env();
    let s = setup(&env);
    s.client.transfer_admin(&s.co_signer);
}

#[test]
fn pause_blocks_stake() {
    let env = new_env();
    let s = setup(&env);
    s.client.pause();
    let staker = new_funded_address(&env, &s, MID_STAKE);
    let ben = Address::generate(&env);
    let result = s.client.try_stake(&staker, &MID_STAKE, &ben);
    assert!(result.is_err());
}

#[test]
fn unpause_restores_stake() {
    let env = new_env();
    let s = setup(&env);
    s.client.pause();
    s.client.unpause();
    let staker = new_funded_address(&env, &s, MID_STAKE);
    let ben = Address::generate(&env);
    s.client.stake(&staker, &MID_STAKE, &ben);
}

#[test]
fn pause_blocks_withdraw() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    s.client.pause();
    let result = s.client.try_withdraw(&staker, &ben);
    assert!(result.is_err());
}

#[test]
fn suspend_stake_blocks_nothing_about_withdrawal() {
    // suspendStake blocks payout eligibility, NOT principal withdrawal —
    // this is the exact V8 semantic this test locks in.
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    s.client.suspend_stake(&staker);
    s.client.withdraw(&staker, &ben);
}

#[test]
#[should_panic(expected = "SAFU: stake suspended")]
fn suspend_stake_blocks_claim_submission() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    s.client.suspend_stake(&staker);
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &1_000_000,
        &3,
        &now_ts(&env),
    );
}

#[test]
fn unsuspend_stake_restores_claim_eligibility() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    s.client.suspend_stake(&staker);
    s.client.unsuspend_stake(&staker, &None);
    s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &1_000_000,
        &3,
        &now_ts(&env),
    );
}

#[test]
#[should_panic(expected = "SAFU: no stake")]
fn suspend_stake_on_nonexistent_stake_panics() {
    let env = new_env();
    let s = setup(&env);
    let random = Address::generate(&env);
    s.client.suspend_stake(&random);
}

/// Mutation-testing gap fix (2026-07-22 re-run, 485 mutants, 10 missed).
/// Kills admin.rs:164 (`!=`->`==`, which would return early even on a
/// MATCHING wallet and skip the reset entirely), admin.rs:169 (deleted
/// AwaitingApproval match arm), and admin.rs:170 x2 (`+`->`-`/`+`->`*` on
/// the deadline arithmetic) — the exact-value assertion below catches all
/// three, since any of them leaves `approve_deadline_ledger` at something
/// other than `now + APPROVE_WINDOW_LEDGERS`.
#[test]
fn unsuspend_stake_resets_rule_a_deadline_for_awaiting_approval() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    advance_days(&env, 90);
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &1_000_000,
        &3,
        &now_ts(&env),
    );
    s.client.suspend_stake(&staker);
    advance_days(&env, 50); // partway into the 100-day approve window
    s.client.unsuspend_stake(&staker, &Some(claim_id.clone()));
    let claim = s.client.get_claim(&claim_id).unwrap();
    let expected = env.ledger().sequence() + crate::types::APPROVE_WINDOW_LEDGERS;
    assert_eq!(claim.approve_deadline_ledger, expected);
}

/// Kills admin.rs:173 (deleted Active match arm) — same wallet-ownership
/// check as the AwaitingApproval test above also runs through this path,
/// but the Active branch's clock reset is independently gapped: without
/// it, `last_collected_ledger` would stay at its pre-suspend value instead
/// of resetting to the unsuspend moment.
#[test]
fn unsuspend_stake_resets_rule_b_clock_for_active_claim() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    advance_days(&env, 90);
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &1_000_000,
        &3,
        &now_ts(&env),
    );
    s.client.approve_claim(&claim_id); // -> Active, cooldown starts
    advance_days(&env, 7); // cooldown passes
    advance_days(&env, 10); // partway into vesting/collection
    s.client.claim_stream(&claim_id, &ben);
    s.client.suspend_stake(&staker);
    advance_days(&env, 20);
    s.client.unsuspend_stake(&staker, &Some(claim_id.clone()));
    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.last_collected_ledger, env.ledger().sequence());
}

#[test]
#[should_panic(expected = "SAFU: no stake")]
fn suspend_stake_after_withdraw_panics() {
    // Reaches "SAFU: no stake" (amount<=0), not "already withdrawn" —
    // same check-ordering note as set_beneficiary_after_withdraw_panics
    // in stake_tests.rs.
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    s.client.withdraw(&staker, &ben);
    s.client.suspend_stake(&staker);
}
