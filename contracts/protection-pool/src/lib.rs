//! SAFU ProtectionPool — Soroban port of `SAFUPoolV8.sol`, Tranche 1 (MVP)
//! scope only: staking, points/tier/claim mechanics, payout streaming,
//! on-chain solvency invariant. Yield-deployment (wstETH/Lido) is
//! deliberately excluded — see context/knowledge/smartcontract-soroban.md
//! in the research-ops repo for the full mechanics map and the eng review
//! that locked this scope boundary (2026-07-14).
//!
//! Build status: scaffold + types + storage done. stake.rs has the full
//! stake/withdraw implementation. claim.rs and admin.rs are structural
//! stubs pending the next build pass — see the task list in the DD session
//! that created this (research-ops, 2026-07-14) for what's left.
//!
//! Local toolchain note: `stellar-cli`/`soroban-cli`, the `wasm32` target,
//! and `cargo-fuzz` were not installed as of 2026-07-14 — this crate has
//! not been compiled yet. Treat all of it as reviewed-on-paper, not
//! verified, until `cargo build --target wasm32v1-none` actually runs.

#![no_std]

mod admin;
mod claim;
mod stake;
mod storage;
mod types;

use soroban_sdk::{contract, contractimpl, Address, Env};

#[contract]
pub struct ProtectionPool;

#[contractimpl]
impl ProtectionPool {
    /// One-time initialization. Guarded against re-invocation (vuln
    /// checklist V6) — see admin.rs.
    pub fn initialize(
        env: Env,
        admin: Address,
        oracle: Address,
        co_signer: Address,
        xlm_token: Address,
        pool_cap: i128,
    ) {
        admin::initialize(&env, &admin, &oracle, &co_signer, &xlm_token, pool_cap);
    }

    pub fn set_oracle(env: Env, new_oracle: Address) {
        admin::set_oracle(&env, &new_oracle);
    }

    pub fn set_co_signer(env: Env, new_co_signer: Address) {
        admin::set_co_signer(&env, &new_co_signer);
    }

    pub fn set_pool_cap(env: Env, new_cap: i128) {
        admin::set_pool_cap(&env, new_cap);
    }

    // -- stake / withdraw --

    pub fn stake(env: Env, staker: Address, amount: i128, beneficiary: Address) {
        stake::stake(&env, &staker, amount, &beneficiary);
    }

    pub fn withdraw(env: Env, staker: Address, beneficiary: Address) {
        stake::withdraw(&env, &staker, &beneficiary);
    }

    // -- claims (stubs — Task 3) --
    // submit_claim / claim_stream / cancel_claim / request_override /
    // approve_override land here once claim.rs is implemented.
}
