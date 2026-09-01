//! ⚠️ STATUS 2026-09-01: INCOMPLETE — DOES NOT RUN. PARKED DELIBERATELY.
//!
//! What works and is worth keeping: Komet itself installs and runs on a
//! GitHub runner in ~3m18s (`kup install komet`, v0.1.89), the CLI surface is
//! known (`run`/`kast`/`test`/`prove`/`prove-raw`; note `komet prove run`,
//! and `komet test -C <dir>`), `src/komet.rs` is vendored verbatim from RV's
//! own example, and the properties below express the right invariants.
//!
//! FOUR KNOWN BLOCKERS, none yet solved — do not assume this compiles:
//!
//!  1. THE POOL WILL NOT DEPLOY. `komet::create_contract` ends in
//!     `deploy_v2(hash, ())` — empty constructor args. `__constructor` takes
//!     six (admin, oracle, oracle_pubkey, co_signer, xlm_token, pool_cap).
//!     RV's example contract has no constructor; this one does. Needs a way
//!     to pass constructor args, or a deploy path that does.
//!  2. `Address::from_string_bytes` BELOW IS NOT A REAL SDK FUNCTION. It was
//!     written from assumption and never compiled. Needs a genuine way to
//!     construct an `Address` in a `no_std` contract.
//!  3. THE DEEP ONE — `stake()` MOVES TOKENS, so it needs a live XLM token
//!     contract deployed inside the symbolic environment, and probably a
//!     DeFindex vault stand-in too. RV's worked example verifies an adder
//!     with zero dependencies. This is a test HARNESS of several cooperating
//!     contracts, not a test file. That is the multi-session part.
//!  4. The `contractimport!` path below is a guess — where Komet writes the
//!     compiled Wasm has not been confirmed.
//!
//! Parked because Komet gates nothing: not the code freeze, not the Audit
//! Bank application, not Tranche 3. The contract's assurance rests on 278
//! unit tests, 98.40% coverage, three mutation campaigns with zero contract
//! defects, and 141,646 fuzz runs. Komet would be an addition, not a
//! dependency. Resume from blocker 1.

//! Komet property tests for `ProtectionPool`.
//!
//! Komet does not scan a contract. It runs property tests written as a
//! SECOND contract that calls the one under test:
//!   * `komet test -C .`   fuzzes each property over many random inputs
//!   * `komet prove run`   symbolically executes them, establishing the
//!                         property for ALL inputs rather than sampled ones
//!
//! Structure follows Runtime Verification's own `test_cross_contract`
//! example exactly:
//!   * `kasmer.json` names each contract to compile; Komet passes their Wasm
//!     hashes to `init` as `Bytes`, in that order
//!   * the deployed address is a fixed 32-byte literal
//!   * every property is an endpoint named `test_*` returning `bool`
//!   * inputs outside the property's scope return `true` early — that is the
//!     example's own convention for "this input says nothing", not a way of
//!     hiding failures
//!
//! WHY THIS PROPERTY
//! The pool's central safety property, enforced at every `submit_claim` and
//! override execution:
//!
//!     total_allocated + entitlement <= total_staked
//!
//! It is what makes the pool solvent — allocated liabilities can never exceed
//! staked assets. The 278 unit tests already assert it over hand-chosen
//! scenarios. Fuzzing widens that to random sequences; `prove` widens it to
//! all of them. That is the gap mutation testing structurally cannot close:
//! mutation measures whether the tests notice a broken contract, not whether
//! the invariant holds universally.

#![no_std]

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Bytes, Env, Symbol};

mod komet;

#[contract]
pub struct TestProtectionPool;

// Built by Komet from the `kasmer.json` entry. `wasm32v1-none` is the only
// target `soroban-sdk` 27 accepts — the older `wasm32-unknown-unknown` used
// in RV's example is rejected outright on Rust 1.82+.
mod protection_pool {
    soroban_sdk::contractimport!(
        file = "../../target/wasm32v1-none/release/protection_pool.wasm"
    );
}

const POOL_ADDR: &[u8; 32] = b"protection_pool_________________";
const POOL_KEY: Symbol = symbol_short!("pool");

#[contractimpl]
impl TestProtectionPool {
    /// Deploy and configure the pool. Komet passes the Wasm hash of each
    /// contract listed in `kasmer.json`, in order.
    pub fn init(env: Env, pool_hash: Bytes) {
        let pool = komet::create_contract(&env, Bytes::from_array(&env, POOL_ADDR), pool_hash);
        env.storage().instance().set(&POOL_KEY, &pool);
    }

    fn pool(env: &Env) -> Address {
        env.storage().instance().get(&POOL_KEY).unwrap()
    }

    /// **The solvency invariant.** Allocated liabilities must never exceed
    /// staked assets, whatever was staked.
    pub fn test_solvency_invariant(env: Env, amount: i128) -> bool {
        let client = protection_pool::Client::new(&env, &Self::pool(&env));

        // Rejection of malformed input is a separate property, covered by the
        // unit suite; a negative amount says nothing about solvency.
        if amount <= 0 {
            return true;
        }

        let staker = Address::from_string_bytes(&Bytes::from_array(&env, POOL_ADDR));
        let _ = client.try_stake(&staker, &amount, &staker);

        // Must hold whether or not the stake was admitted.
        client.get_total_allocated() <= client.get_total_staked()
    }

    /// Staking credits the pool by exactly the amount staked — no rounding
    /// drift, no double count. The accounting solvency rests on.
    pub fn test_stake_accounting_is_exact(env: Env, amount: i128) -> bool {
        let client = protection_pool::Client::new(&env, &Self::pool(&env));

        if amount <= 0 {
            return true;
        }

        let before = client.get_total_staked();
        let staker = Address::from_string_bytes(&Bytes::from_array(&env, POOL_ADDR));

        if client.try_stake(&staker, &amount, &staker).is_err() {
            // Rejected: the total must be untouched.
            return client.get_total_staked() == before;
        }
        client.get_total_staked() == before + amount
    }

    /// Neither pool total may go negative. Trivial if the arithmetic is
    /// sound — which is exactly why it is worth stating, since it is the
    /// shape an overflow or unchecked subtraction produces.
    pub fn test_totals_never_negative(env: Env, amount: i128) -> bool {
        let client = protection_pool::Client::new(&env, &Self::pool(&env));

        if amount > 0 {
            let staker = Address::from_string_bytes(&Bytes::from_array(&env, POOL_ADDR));
            let _ = client.try_stake(&staker, &amount, &staker);
        }

        client.get_total_staked() >= 0 && client.get_total_allocated() >= 0
    }
}
