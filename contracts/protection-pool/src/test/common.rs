//! Shared test setup. Env and Client are kept as separate local bindings
//! at each call site (not bundled into one struct); a struct can't hold
//! both an owned Env and a Client borrowing that same Env without being
//! self-referential. `new_env()` + `setup(&env)` is the standard Soroban
//! pattern for avoiding that.

#![cfg(test)]

use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{Address, BytesN, Env};
use std::{println, string::{String, ToString}, thread, time::Duration};

use crate::types::LEDGERS_PER_DAY;
use crate::{ProtectionPool, ProtectionPoolClient};

pub const STROOPS_PER_XLM: i128 = 10_000_000;

/// 10,000 XLM in stroops (7dp): a clean round pool cap for test math.
pub const POOL_CAP: i128 = 100_000_000_000;
/// 2 bps of POOL_CAP: 2 XLM.
pub const MIN_STAKE: i128 = 20_000_000;
/// 125 bps of POOL_CAP: 125 XLM (matches V8's exact 1.25% ratio).
pub const MAX_STAKE: i128 = 1_250_000_000;
/// Comfortably within [MIN_STAKE, MAX_STAKE]: the default stake size
/// used by tests that don't care about boundary behavior.
pub const MID_STAKE: i128 = 100_000_000;

pub fn new_env() -> Env {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|li| {
        li.sequence_number = 1_000_000;
        li.timestamp = 1_700_000_000;
    });
    env
}

pub struct Setup<'a> {
    pub client: ProtectionPoolClient<'a>,
    pub admin: Address,
    pub oracle: Address,
    pub co_signer: Address,
    pub token_admin: StellarAssetClient<'a>,
    pub token_id: Address,
}

pub fn setup(env: &Env) -> Setup<'_> {
    setup_with_cap(env, POOL_CAP)
}

pub fn setup_with_cap(env: &Env, pool_cap: i128) -> Setup<'_> {
    let admin = Address::generate(env);
    let oracle = Address::generate(env);
    let co_signer = Address::generate(env);

    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let token_id = sac.address();
    let token_admin = StellarAssetClient::new(env, &token_id);

    let contract_id = env.register(ProtectionPool, ());
    let client = ProtectionPoolClient::new(env, &contract_id);
    client.initialize(&admin, &oracle, &co_signer, &token_id, &pool_cap);

    Setup {
        client,
        admin,
        oracle,
        co_signer,
        token_admin,
        token_id,
    }
}

pub fn new_funded_address(env: &Env, s: &Setup, funding: i128) -> Address {
    let addr = Address::generate(env);
    s.token_admin.mint(&addr, &funding);
    addr
}

/// Stakes MID_STAKE from a freshly funded, freshly generated staker with
/// a freshly generated beneficiary. Returns (staker, beneficiary), the
/// shape most claim/stream tests start from.
pub fn staked_wallet(env: &Env, s: &Setup) -> (Address, Address) {
    staked_wallet_amount(env, s, MID_STAKE)
}

pub fn staked_wallet_amount(env: &Env, s: &Setup, amount: i128) -> (Address, Address) {
    let staker = new_funded_address(env, s, amount);
    let beneficiary = Address::generate(env);
    s.client.stake(&staker, &amount, &beneficiary);
    (staker, beneficiary)
}

pub fn advance_ledgers(env: &Env, n: u32) {
    env.ledger().with_mut(|li| {
        li.sequence_number += n;
        li.timestamp += (n as u64) * 5;
    });
}

pub fn advance_days(env: &Env, days: u32) {
    advance_ledgers(env, days * LEDGERS_PER_DAY);
}

pub fn tx_hash(env: &Env, seed: u8) -> BytesN<32> {
    BytesN::from_array(env, &[seed; 32])
}

pub fn now_ts(env: &Env) -> u64 {
    env.ledger().timestamp()
}

// -----------------------------------------------------------------------
// Presentation helpers -- shared by pool_demo_tests.rs and
// blend_scenario_tests.rs's video-recordable demo tests. Not used by the
// plain assertion-only unit tests, which don't need readable output.
// -----------------------------------------------------------------------

/// Paces printed output for video recording -- without this, `cargo test
/// -- --nocapture` dumps the whole lifecycle instantly, too fast to
/// follow on screen.
pub fn pause() {
    thread::sleep(Duration::from_millis(500));
}

/// Whole-XLM, comma-separated amount for readable presentation output
/// (e.g. `500_000_000_000` stroops -> "50,000"). Demo amounts are round
/// XLM multiples, so integer division here is exact.
pub fn xlm(stroops: i128) -> String {
    let whole = stroops / STROOPS_PER_XLM;
    let digits = whole.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    grouped.chars().rev().collect()
}

/// Real Tranche 1 deploy value (README "Deploy-time arguments"): 600,000
/// XLM, matching the live testnet contract's actual pool cap. Used by
/// video-recordable demo tests so their numbers match the real
/// deployment, not the smaller default `POOL_CAP` other unit tests use.
pub const DEMO_POOL_CAP: i128 = 6_000_000_000_000;

pub fn max_stake(pool_cap: i128) -> i128 {
    pool_cap * 125 / 10_000
}

pub fn print_solvency(s: &Setup<'_>, label: &str) {
    let staked = s.client.get_total_staked();
    let allocated = s.client.get_total_allocated();
    println!(
        "         Pool solvency [{}]: allocated {} XLM <= staked {} XLM -> {}",
        label,
        xlm(allocated),
        xlm(staked),
        if allocated <= staked { "OK" } else { "VIOLATION" }
    );
    pause();
}
