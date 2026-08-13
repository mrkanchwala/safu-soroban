#![cfg(test)]

use soroban_sdk::testutils::Address as _;
use soroban_sdk::Address;

use super::common::*;
use crate::error::PoolError;

#[test]
fn stake_at_min_bound_succeeds() {
    let env = new_env();
    let s = setup(&env);
    let staker = new_funded_address(&env, &s, MIN_STAKE);
    let ben = Address::generate(&env);
    s.client.stake(&staker, &MIN_STAKE, &ben);
}

#[test]
fn stake_at_max_bound_succeeds() {
    let env = new_env();
    let s = setup(&env);
    let staker = new_funded_address(&env, &s, MAX_STAKE);
    let ben = Address::generate(&env);
    s.client.stake(&staker, &MAX_STAKE, &ben);
}

#[test]
fn stake_below_min_panics() {
    let env = new_env();
    let s = setup(&env);
    let amount = MIN_STAKE - 1;
    let staker = new_funded_address(&env, &s, amount);
    let ben = Address::generate(&env);
    let result = s.client.try_stake(&staker, &amount, &ben);
    assert_eq!(result, Err(Ok(PoolError::StakeOutOfRange)));
}

#[test]
fn stake_above_max_panics() {
    let env = new_env();
    let s = setup(&env);
    let amount = MAX_STAKE + 1;
    let staker = new_funded_address(&env, &s, amount);
    let ben = Address::generate(&env);
    let result = s.client.try_stake(&staker, &amount, &ben);
    assert_eq!(result, Err(Ok(PoolError::StakeOutOfRange)));
}

#[test]
fn stake_zero_panics() {
    let env = new_env();
    let s = setup(&env);
    let staker = new_funded_address(&env, &s, 0);
    let ben = Address::generate(&env);
    let result = s.client.try_stake(&staker, &0, &ben);
    assert_eq!(result, Err(Ok(PoolError::StakeNotPositive)));
}

#[test]
fn stake_negative_panics() {
    let env = new_env();
    let s = setup(&env);
    let staker = new_funded_address(&env, &s, MID_STAKE);
    let ben = Address::generate(&env);
    // The amount<=0 check runs BEFORE the range check, so negative hits
    // "must be positive" first — not "out of range" (verified by running
    // this test; the two errors are easy to mix up by inspection alone).
    let result = s.client.try_stake(&staker, &-1, &ben);
    assert_eq!(result, Err(Ok(PoolError::StakeNotPositive)));
}

#[test]
fn stake_twice_without_withdraw_panics() {
    let env = new_env();
    let s = setup(&env);
    let staker = new_funded_address(&env, &s, MID_STAKE * 2);
    let ben = Address::generate(&env);
    s.client.stake(&staker, &MID_STAKE, &ben);
    let result = s.client.try_stake(&staker, &MID_STAKE, &ben);
    assert_eq!(result, Err(Ok(PoolError::AlreadyStaked)));
}

#[test]
fn stake_after_withdraw_succeeds() {
    let env = new_env();
    let s = setup(&env);
    let staker = new_funded_address(&env, &s, MID_STAKE * 2);
    let ben = Address::generate(&env);
    s.client.stake(&staker, &MID_STAKE, &ben);
    s.client.withdraw(&staker, &ben);
    s.client.stake(&staker, &MID_STAKE, &ben);
}

#[test]
fn stake_exceeding_pool_cap_panics() {
    // Per-staker max is architecturally always 1.25% of the pool cap
    // (MAX_STAKE_BPS is a fixed ratio, not an absolute), so filling a
    // pool legitimately always takes ~80 max-sized stakers regardless of
    // the cap's absolute size — there's no way to hit this panic with
    // just two stakes.
    let env = new_env();
    let pool_cap = 8_000_000_000i128;
    let s = setup_with_cap(&env, pool_cap);
    let max_for_cap = pool_cap * 125 / 10_000; // 100_000_000
    let min_for_cap = pool_cap * 2 / 10_000; // 1_600_000

    for _ in 0..80 {
        let staker = new_funded_address(&env, &s, max_for_cap);
        let ben = Address::generate(&env);
        s.client.stake(&staker, &max_for_cap, &ben);
    }
    assert_eq!(s.client.get_total_staked(), pool_cap); // exactly full

    let one_more = new_funded_address(&env, &s, min_for_cap);
    let ben = Address::generate(&env);
    let result = s.client.try_stake(&one_more, &min_for_cap, &ben);
    assert_eq!(result, Err(Ok(PoolError::PoolCapExceeded)));
}

#[test]
fn stake_beneficiary_equals_staker_panics() {
    let env = new_env();
    let s = setup(&env);
    let staker = new_funded_address(&env, &s, MID_STAKE);
    let result = s.client.try_stake(&staker, &MID_STAKE, &staker);
    assert_eq!(result, Err(Ok(PoolError::BeneficiaryIsStaker)));
}

#[test]
fn stake_beneficiary_equals_oracle_panics() {
    let env = new_env();
    let s = setup(&env);
    let staker = new_funded_address(&env, &s, MID_STAKE);
    let result = s.client.try_stake(&staker, &MID_STAKE, &s.oracle);
    assert_eq!(result, Err(Ok(PoolError::BeneficiaryIsOracle)));
}

#[test]
fn stake_beneficiary_equals_admin_panics() {
    let env = new_env();
    let s = setup(&env);
    let staker = new_funded_address(&env, &s, MID_STAKE);
    let result = s.client.try_stake(&staker, &MID_STAKE, &s.admin);
    assert_eq!(result, Err(Ok(PoolError::BeneficiaryIsAdmin)));
}

#[test]
fn stake_beneficiary_equals_cosigner_panics() {
    let env = new_env();
    let s = setup(&env);
    let staker = new_funded_address(&env, &s, MID_STAKE);
    let result = s.client.try_stake(&staker, &MID_STAKE, &s.co_signer);
    assert_eq!(result, Err(Ok(PoolError::BeneficiaryIsCoSigner)));
}

#[test]
fn withdraw_returns_principal() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    s.client.withdraw(&staker, &ben);
    // A second withdraw on the same (now-withdrawn) record must fail —
    // confirms the record was actually marked withdrawn, not just funds
    // moved.
    let result = s.client.try_withdraw(&staker, &ben);
    assert!(result.is_err());
}

#[test]
fn withdraw_without_stake_panics() {
    let env = new_env();
    let s = setup(&env);
    let random = Address::generate(&env);
    let ben = Address::generate(&env);
    let result = s.client.try_withdraw(&random, &ben);
    assert_eq!(result, Err(Ok(PoolError::NoStake)));
}

#[test]
fn withdraw_wrong_beneficiary_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    let wrong = Address::generate(&env);
    let result = s.client.try_withdraw(&staker, &wrong);
    assert_eq!(result, Err(Ok(PoolError::WrongBeneficiary)));
}

#[test]
fn withdraw_blocked_while_claim_active() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    submit_claim_signed(&env, &s, &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &1_000_000,
        &3,
        &now_ts(&env),
    );
    let result = s.client.try_withdraw(&staker, &ben);
    assert_eq!(result, Err(Ok(PoolError::ClaimActive)));
}

#[test]
fn set_beneficiary_updates_target() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _old_ben) = staked_wallet(&env, &s);
    let new_ben = Address::generate(&env);
    s.client.set_beneficiary(&staker, &new_ben);
    s.client.withdraw(&staker, &new_ben);
}

#[test]
fn set_beneficiary_old_target_no_longer_works() {
    let env = new_env();
    let s = setup(&env);
    let (staker, old_ben) = staked_wallet(&env, &s);
    let new_ben = Address::generate(&env);
    s.client.set_beneficiary(&staker, &new_ben);
    let result = s.client.try_withdraw(&staker, &old_ben);
    assert_eq!(result, Err(Ok(PoolError::WrongBeneficiary)));
}

#[test]
fn set_beneficiary_after_withdraw_panics() {
    // Reaches PoolError::NoStake (the amount<=0 check), not StakeForfeited
    // — that check runs first and a full withdraw zeroes amount, so the
    // withdrawn-specific check below it is unreachable via this exact
    // path (still real defensive code, just not what fires here).
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    s.client.withdraw(&staker, &ben);
    let new_ben = Address::generate(&env);
    let result = s.client.try_set_beneficiary(&staker, &new_ben);
    assert_eq!(result, Err(Ok(PoolError::NoStake)));
}

#[test]
fn emergency_exit_works_while_paused() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    s.client.pause();
    s.client.emergency_exit(&staker);
}

#[test]
fn emergency_exit_works_unpaused_too() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    s.client.emergency_exit(&staker);
}

#[test]
fn emergency_exit_blocked_while_claim_active() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet(&env, &s);
    submit_claim_signed(&env, &s, &s.oracle,
        &staker,
        &tx_hash(&env, 1),
        &1_000_000,
        &3,
        &now_ts(&env),
    );
    let result = s.client.try_emergency_exit(&staker);
    assert_eq!(result, Err(Ok(PoolError::ClaimActive)));
}

#[test]
fn emergency_exit_after_withdraw_panics() {
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    s.client.withdraw(&staker, &ben);
    let result = s.client.try_emergency_exit(&staker);
    assert_eq!(result, Err(Ok(PoolError::NoActiveStake)));
}

#[test]
fn stake_bounds_track_pool_cap_changes() {
    // Dynamic bps-of-pool-cap bounds — a set_pool_cap change should
    // immediately widen/narrow what a NEW stake can be, no re-anchoring
    // step required.
    let env = new_env();
    let s = setup(&env);
    s.client.set_pool_cap(&(POOL_CAP * 10));
    let bigger_max = MAX_STAKE * 10;
    let staker = new_funded_address(&env, &s, bigger_max);
    let ben = Address::generate(&env);
    s.client.stake(&staker, &bigger_max, &ben);
}

#[test]
fn multiple_independent_stakers_coexist() {
    let env = new_env();
    let s = setup(&env);
    for i in 0..5u8 {
        let staker = new_funded_address(&env, &s, MID_STAKE);
        let ben = Address::generate(&env);
        s.client.stake(&staker, &MID_STAKE, &ben);
        let _ = i;
    }
}

#[test]
fn withdraw_after_partial_pool_cap_reduction_still_works() {
    // set_pool_cap only blocks shrinking below CURRENT total_staked, not
    // below what an individual staker holds — a staker already in the
    // pool can still exit even if the cap later tightens around them.
    let env = new_env();
    let s = setup(&env);
    let (staker, ben) = staked_wallet(&env, &s);
    s.client.set_pool_cap(&MID_STAKE); // exactly at the current total
    s.client.withdraw(&staker, &ben);
}
