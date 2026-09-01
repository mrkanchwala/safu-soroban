//! Komet property tests for `ProtectionPool`'s solvency invariant.
//!
//! Komet does not scan a contract. It runs property tests written as a
//! SECOND contract calling the ones under test:
//!   * `komet test -C .`  fuzzes each property over many random inputs
//!   * `komet prove run`  symbolically executes them, establishing the
//!                        property for ALL inputs rather than sampled ones
//!
//! WHAT IS ACTUALLY UNDER TEST, STATED PRECISELY
//! `komet-harness`, not `protection-pool` — but the harness `#[path]`-includes
//! the REAL `stake.rs`, `claim.rs`, `storage.rs`, `vault.rs`, `admin.rs`,
//! `types.rs` and `error.rs`. Those are shared files, not copies, so the logic
//! cannot drift. The only difference is the entrypoint: `komet_init` in place
//! of `__constructor`.
//!
//! That indirection is forced, not chosen. Komet deploys through
//! `kasmer_create_contract(address, wasm_hash)`, which cannot pass constructor
//! arguments, and `ProtectionPool.__constructor` needs six. Gating the
//! constructor behind a cargo feature was tried first and REJECTED: it changed
//! the optimised production Wasm hash (`2cec7e74…` → `024b4078…`) even with
//! the feature OFF, because `#[contractimpl]` emits a different contract spec.
//! A harness that alters the artifact you deploy is worse than no harness.
//!
//! `komet-mock-token` is here because `stake()` performs a real transfer
//! (`stake.rs:190`) and Komet has no cheat function for balances.
//!
//! WHAT MAY BE CLAIMED FROM A PASS
//! That the solvency invariant holds for the pool's own accounting logic,
//! across the input space explored. NOT that the deployed artifact is proved
//! byte-for-byte, and NOT anything about the real XLM SAC. Cite it with those
//! bounds attached.

#![no_std]

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Bytes, Env, Symbol};

mod komet;

#[contract]
pub struct TestProtectionPool;

mod pool {
    soroban_sdk::contractimport!(
        file = "../komet-harness/target/wasm32v1-none/release/komet_harness.wasm"
    );
}

mod token {
    soroban_sdk::contractimport!(
        file = "../komet-mock-token/target/wasm32v1-none/release/komet_mock_token.wasm"
    );
}

// Fixed 32-byte addresses, following RV's own example convention.
const POOL_ADDR: &[u8; 32] = b"protection_pool_________________";
const TOKEN_ADDR: &[u8; 32] = b"mock_xlm_token__________________";
const STAKER_ADDR: &[u8; 32] = b"staker_one______________________";
const ADMIN_ADDR: &[u8; 32] = b"admin___________________________";
const ORACLE_ADDR: &[u8; 32] = b"oracle__________________________";
const COSIGNER_ADDR: &[u8; 32] = b"co_signer_______________________";

const POOL_KEY: Symbol = symbol_short!("pool");
const TOKEN_KEY: Symbol = symbol_short!("token");

/// 600,000 XLM in stroops — the Tranche 1/2 deploy value, so the cap the
/// properties run against is the real one.
const POOL_CAP: i128 = 6_000_000_000_000;

/// Funded generously so a stake fails on pool rules, never on funds. A
/// property that silently passes because every transfer reverted proves
/// nothing.
const STAKER_FUNDING: i128 = 100_000_000_000_000;

#[contractimpl]
impl TestProtectionPool {
    /// Komet compiles each contract in `kasmer.json` and passes their Wasm
    /// hashes here in that order: harness first, mock token second.
    pub fn init(env: Env, pool_hash: Bytes, token_hash: Bytes) {
        let pool_addr = komet::create_contract(&env, Bytes::from_array(&env, POOL_ADDR), pool_hash);
        let token_addr =
            komet::create_contract(&env, Bytes::from_array(&env, TOKEN_ADDR), token_hash);

        let staker = Self::addr(&env, STAKER_ADDR);

        // Fund the staker before anything else, so transfers are not the
        // reason a property holds.
        let token_client = token::Client::new(&env, &token_addr);
        token_client.mint(&staker, &STAKER_FUNDING);

        // Stands in for `__constructor`, delegating to the same
        // `admin::initialize`.
        let pool_client = pool::Client::new(&env, &pool_addr);
        pool_client.komet_init(
            &Self::addr(&env, ADMIN_ADDR),
            &Self::addr(&env, ORACLE_ADDR),
            &soroban_sdk::BytesN::from_array(&env, &[7u8; 32]),
            &Self::addr(&env, COSIGNER_ADDR),
            &token_addr,
            &POOL_CAP,
        );

        env.storage().instance().set(&POOL_KEY, &pool_addr);
        env.storage().instance().set(&TOKEN_KEY, &token_addr);
    }

    fn addr(env: &Env, raw: &[u8; 32]) -> Address {
        komet::address_from_bytes(env, Bytes::from_array(env, raw), false)
    }

    fn pool(env: &Env) -> pool::Client {
        pool::Client::new(env, &env.storage().instance().get(&POOL_KEY).unwrap())
    }

    /// **The solvency invariant.** Allocated liabilities must never exceed
    /// staked assets, whatever was staked. This is the property that makes
    /// the pool able to pay its claims.
    pub fn test_solvency_invariant(env: Env, amount: i128) -> bool {
        let client = Self::pool(&env);
        let staker = Self::addr(&env, STAKER_ADDR);

        // A negative amount says nothing about solvency; rejecting malformed
        // input is a separate property the unit suite covers.
        if amount <= 0 {
            return true;
        }

        let _ = client.try_stake(&staker, &amount, &staker);

        // Must hold whether the stake was admitted or refused.
        client.get_total_allocated() <= client.get_total_staked()
    }

    /// Staking credits the pool by exactly the amount staked — no drift, no
    /// double count. The accounting solvency rests on.
    pub fn test_stake_accounting_is_exact(env: Env, amount: i128) -> bool {
        let client = Self::pool(&env);
        let staker = Self::addr(&env, STAKER_ADDR);

        if amount <= 0 {
            return true;
        }

        let before = client.get_total_staked();
        if client.try_stake(&staker, &amount, &staker).is_err() {
            // Refused: the total must be untouched.
            return client.get_total_staked() == before;
        }
        client.get_total_staked() == before + amount
    }

    /// Neither total may go negative — the shape an overflow or unchecked
    /// subtraction would produce.
    pub fn test_totals_never_negative(env: Env, amount: i128) -> bool {
        let client = Self::pool(&env);

        if amount > 0 {
            let staker = Self::addr(&env, STAKER_ADDR);
            let _ = client.try_stake(&staker, &amount, &staker);
        }

        client.get_total_staked() >= 0 && client.get_total_allocated() >= 0
    }
}
