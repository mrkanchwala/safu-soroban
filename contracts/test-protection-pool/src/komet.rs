//! Komet's `create_contract` helper — VENDORED, not authored here.
//!
//! Copied verbatim from Runtime Verification's own integration example
//! (`src/tests/integration/data/soroban/contracts/test_cross_contract/src/komet.rs`
//! in `runtimeverification/komet`). Komet ships this as a small module you
//! copy into your test contract rather than as a published crate, which is
//! why it is a file here and not a dependency — there IS a `komet` crate on
//! crates.io, but it is an unrelated mailer library by a different author.
//!
//! Do not "improve" this file. It has to match what Komet's host expects.
//!
//! How it works: under Komet the `kasmer_create_contract` host function is
//! provided by the K semantics, so deployment happens inside the symbolic
//! environment where it can be reasoned about. Under a plain `cargo test`
//! the `#[cfg(test)]` branch falls back to the ordinary Soroban deployer, so
//! the same source compiles both ways.

use soroban_sdk::{Address, Bytes, Env};

#[cfg(not(test))]
use soroban_sdk::{FromVal, Val};

#[cfg(not(test))]
#[link(wasm_import_module = "env")]
extern "C" {
    fn kasmer_create_contract(addr_val: u64, hash_val: u64) -> u64;
}

#[cfg(not(test))]
pub fn create_contract(env: &Env, addr: Bytes, hash: Bytes) -> Address {
    unsafe {
        let res = kasmer_create_contract(addr.as_val().get_payload(), hash.as_val().get_payload());
        Address::from_val(env, &Val::from_payload(res))
    }
}

#[cfg(test)]
pub fn create_contract(env: &Env, addr: Bytes, hash: Bytes) -> Address {
    use soroban_sdk::BytesN;

    let addr: BytesN<32> = addr.try_into().unwrap();
    let hash: BytesN<32> = hash.try_into().unwrap();
    env.deployer()
        .with_current_contract(addr)
        .deploy_v2(hash, ())
}

// ---------------------------------------------------------------------------
// `kasmer_address_from_bytes` — the second Komet cheat function this harness
// needs, added 2026-09-01. It is how you construct an Address inside a
// `no_std` contract; there is no SDK route. `is_contract` selects between a
// contract address (1) and an account address (0).
//
// Same shape as `create_contract` above: the host function under Komet, the
// ordinary SDK path under `cargo test`.
// ---------------------------------------------------------------------------

#[cfg(not(test))]
#[link(wasm_import_module = "env")]
extern "C" {
    fn kasmer_address_from_bytes(addr_val: u64, is_contract: u64) -> u64;
}

#[cfg(not(test))]
pub fn address_from_bytes(env: &Env, addr: Bytes, is_contract: bool) -> Address {
    unsafe {
        let res = kasmer_address_from_bytes(
            addr.as_val().get_payload(),
            if is_contract { 1u64 } else { 0u64 },
        );
        Address::from_val(env, &Val::from_payload(res))
    }
}

#[cfg(test)]
pub fn address_from_bytes(env: &Env, addr: Bytes, _is_contract: bool) -> Address {
    use soroban_sdk::BytesN;
    let addr: BytesN<32> = addr.try_into().unwrap();
    Address::from_val(env, &addr.to_val())
}
