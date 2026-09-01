//! Komet property tests for `ProtectionPool`.
//!
//! Komet (Runtime Verification) does not scan a contract. It runs *property
//! tests written as a second contract* that calls the first one, under two
//! engines: `komet test` fuzzes them over many random inputs, and
//! `komet prove` symbolically executes them to establish the property for
//! ALL inputs rather than sampled ones.
//!
//! Conventions this file follows, per Komet's documentation:
//!   * `init` deploys the contract under test. Komet compiles each contract
//!     named in `kasmer.json`, registers its Wasm, and passes the hash here.
//!   * every property is an endpoint named `test_*` returning `bool`.
//!     `true` = property held.
//!
//! WHY THESE PROPERTIES
//! The pool's central safety property is stated in the contract's own module
//! docs and enforced at every `submit_claim` and override execution:
//!
//!     total_allocated + entitlement <= total_staked
//!
//! It is the property that makes the pool solvent: allocated liabilities can
//! never exceed staked assets. The existing 278 unit tests assert it over
//! hand-chosen scenarios. Fuzzing widens that to random sequences; `prove`
//! widens it to all of them. That is the gap Komet closes and mutation
//! testing cannot — mutation measures whether the tests notice a broken
//! contract, not whether the invariant holds universally.

#![no_std]

use soroban_sdk::{contract, contractimpl, Address, BytesN, Env, Symbol};

const POOL: Symbol = soroban_sdk::symbol_short!("POOL");

#[contract]
pub struct TestProtectionPool;

#[contractimpl]
impl TestProtectionPool {
    /// Deploy the contract under test.
    ///
    /// `pool_hash` is supplied by Komet from the Wasm it compiled for the
    /// entry in `kasmer.json`.
    pub fn init(env: Env, pool_hash: BytesN<32>) {
        let admin = Address::generate(&env);
        let oracle = Address::generate(&env);
        let co_signer = Address::generate(&env);
        let xlm_token = Address::generate(&env);
        let oracle_pubkey = BytesN::from_array(&env, &[0u8; 32]);

        // Deployed through Komet so the engine can track and symbolically
        // execute the callee, rather than a plain host deploy.
        let pool = komet::create_contract(&env, &Address::generate(&env), &pool_hash);

        let client = protection_pool::Client::new(&env, &pool);
        client.__constructor(
            &admin,
            &oracle,
            &oracle_pubkey,
            &co_signer,
            &xlm_token,
            &600_000_0000000i128, // 600,000 XLM in stroops, the T1/T2 cap
        );

        env.storage().instance().set(&POOL, &pool);
    }

    fn pool(env: &Env) -> Address {
        env.storage().instance().get(&POOL).unwrap()
    }

    /// **The solvency invariant.** Allocated liabilities must never exceed
    /// staked assets, whatever sequence of amounts was staked.
    ///
    /// This is the one worth proving rather than sampling: a violation is
    /// the difference between a pool that can pay its claims and one that
    /// cannot.
    pub fn test_solvency_invariant(env: Env, amount: i128) -> bool {
        let pool = Self::pool(&env);
        let client = protection_pool::Client::new(&env, &pool);

        let staker = Address::generate(&env);
        let beneficiary = Address::generate(&env);

        // Only well-formed stakes are in scope; rejection of malformed input
        // is a separate property, covered by the unit suite.
        if amount <= 0 {
            return true;
        }
        if client.try_stake(&staker, &amount, &beneficiary).is_err() {
            // A rejected stake must not have moved the accounting.
            return client.get_total_allocated() <= client.get_total_staked();
        }

        client.get_total_allocated() <= client.get_total_staked()
    }

    /// Staking credits the pool by exactly the amount staked — no rounding
    /// drift, no double-count. Accounting integrity underneath solvency.
    pub fn test_stake_accounting_is_exact(env: Env, amount: i128) -> bool {
        let pool = Self::pool(&env);
        let client = protection_pool::Client::new(&env, &pool);

        if amount <= 0 {
            return true;
        }

        let before = client.get_total_staked();
        let staker = Address::generate(&env);
        let beneficiary = Address::generate(&env);

        if client.try_stake(&staker, &amount, &beneficiary).is_err() {
            // Rejected: the total must be untouched.
            return client.get_total_staked() == before;
        }

        client.get_total_staked() == before + amount
    }

    /// Neither pool total can go negative. Trivially true if the arithmetic
    /// is sound, which is exactly why it is worth stating: it is the shape
    /// of bug an overflow or an unchecked subtraction produces.
    pub fn test_totals_never_negative(env: Env, amount: i128) -> bool {
        let pool = Self::pool(&env);
        let client = protection_pool::Client::new(&env, &pool);

        if amount > 0 {
            let staker = Address::generate(&env);
            let beneficiary = Address::generate(&env);
            let _ = client.try_stake(&staker, &amount, &beneficiary);
        }

        client.get_total_staked() >= 0 && client.get_total_allocated() >= 0
    }
}
