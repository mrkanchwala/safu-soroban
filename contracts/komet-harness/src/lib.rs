//! Verification harness for Komet — a deployable stand-in for
//! `ProtectionPool` that exists solely so its properties can be checked.
//!
//! WHY THIS CRATE EXISTS
//! Komet deploys the contract under test through its `kasmer_create_contract`
//! host function, whose whole signature is `(address, wasm_hash)`. It cannot
//! pass constructor arguments, and none of the four documented cheat
//! functions offers a variant that can. `ProtectionPool.__constructor` takes
//! six arguments, so it cannot be deployed inside Komet's symbolic
//! environment at all.
//!
//! WHY IT IS A SEPARATE CRATE RATHER THAN A FEATURE FLAG
//! The obvious fix — gate `__constructor` behind a cargo feature and add a
//! plain initializer — was built, measured, and REJECTED. Adding even a
//! disabled `#[cfg(...)]` function to `ProtectionPool` changed the optimised
//! Wasm hash from `2cec7e74…` to `024b4078…` with the feature switched OFF,
//! because `#[contractimpl]` emits a different contract spec. Verified by
//! building twice (deterministic) and by stashing the edit (hash returned).
//! A verification harness that alters the artifact you deploy is worse than
//! no harness, so the production contract is left untouched, byte for byte.
//!
//! HOW DRIFT IS PREVENTED
//! Every line of logic below is the REAL source, pulled in by `#[path]` —
//! not copied. `admin.rs`, `claim.rs`, `stake.rs`, `storage.rs`, `types.rs`,
//! `vault.rs` and `error.rs` are the same files `protection-pool` compiles.
//! Editing them changes both. Only this entrypoint file differs, and the
//! difference is exactly one thing: `komet_init` in place of
//! `__constructor`, delegating to the identical `admin::initialize`.
//!
//! WHAT MAY AND MAY NOT BE CLAIMED FROM A RESULT HERE
//! A property proved here holds for the logic in those shared modules, which
//! is where the solvency invariant lives. It does NOT establish anything
//! about the constructor path, and it is not a proof about the deployed
//! artifact byte-for-byte. Any result cited externally must say so.

#![no_std]

use soroban_sdk::{contract, contractimpl, Address, BytesN, Env};

// The real implementation — shared files, not copies.
#[path = "../../protection-pool/src/error.rs"]
mod error;
#[path = "../../protection-pool/src/types.rs"]
mod types;
#[path = "../../protection-pool/src/storage.rs"]
mod storage;
#[path = "../../protection-pool/src/admin.rs"]
mod admin;
#[path = "../../protection-pool/src/vault.rs"]
mod vault;
#[path = "../../protection-pool/src/stake.rs"]
mod stake;
#[path = "../../protection-pool/src/claim.rs"]
mod claim;

use error::PoolError;

#[contract]
pub struct KometHarness;

#[contractimpl]
impl KometHarness {
    /// Stands in for `__constructor`. Same arguments, same delegate, called
    /// by the test contract after deployment instead of during it.
    #[allow(clippy::too_many_arguments)]
    pub fn komet_init(
        env: Env,
        admin_addr: Address,
        oracle: Address,
        oracle_pubkey: BytesN<32>,
        co_signer: Address,
        xlm_token: Address,
        pool_cap: i128,
    ) -> Result<(), PoolError> {
        admin::initialize(
            &env,
            &admin_addr,
            &oracle,
            &oracle_pubkey,
            &co_signer,
            &xlm_token,
            pool_cap,
        )
    }

    // Only the surface the properties exercise is re-exported. Each one is a
    // straight delegate to the shared module, identical to the body in
    // `protection-pool/src/lib.rs`.

    pub fn stake(
        env: Env,
        staker: Address,
        amount: i128,
        beneficiary: Address,
    ) -> Result<(), PoolError> {
        stake::stake(&env, &staker, amount, &beneficiary)
    }

    pub fn withdraw(env: Env, staker: Address, beneficiary: Address) -> Result<(), PoolError> {
        stake::withdraw(&env, &staker, &beneficiary)
    }

    pub fn get_total_staked(env: Env) -> i128 {
        storage::get_total_staked(&env)
    }

    pub fn get_total_allocated(env: Env) -> i128 {
        storage::get_total_allocated(&env)
    }

    pub fn get_total_stakers(env: Env) -> u32 {
        storage::get_total_stakers(&env)
    }
}
