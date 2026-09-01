//! Minimal SAC-shaped token, for the Komet harness only.
//!
//! `stake()` performs a real transfer (`stake.rs:190`:
//! `token.transfer(staker, contract, amount)`), so verifying it needs a token
//! contract present in Komet's symbolic ledger. Komet provides no cheat
//! function for balances — the documented set is only
//! `set_ledger_sequence`, `set_ledger_timestamp`, `create_contract` and
//! `address_from_bytes` — so the balances have to come from a contract.
//!
//! WHY A MOCK RATHER THAN THE REAL SAC
//! The Stellar Asset Contract is a host-native contract, not a Wasm one that
//! Komet can compile from a `kasmer.json` path. This implements only what the
//! pool actually calls, with real balance arithmetic, so a transfer that
//! should fail does fail.
//!
//! WHAT THIS MEANS FOR ANY RESULT
//! Properties proved here concern the POOL's accounting, not the token's.
//! That is the right boundary: the solvency invariant
//! (`total_allocated <= total_staked`) is a statement about the pool's own
//! bookkeeping, and it must hold regardless of which conforming token sits
//! underneath. A result must not be cited as saying anything about the real
//! XLM SAC.
//!
//! Deliberately NOT a faithful SAC: no allowances, no admin controls, no
//! authorization checks. Adding them would widen what has to be trusted
//! without widening what is proved.

#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env};

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Balance(Address),
}

#[contracterror]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum TokenError {
    InsufficientBalance = 1,
    NegativeAmount = 2,
}

#[contract]
pub struct MockToken;

#[contractimpl]
impl MockToken {
    /// Credit an address. Stands in for funding a test staker; there is no
    /// admin check because nothing being proved depends on one.
    pub fn mint(env: Env, to: Address, amount: i128) {
        if amount <= 0 {
            return;
        }
        let key = DataKey::Balance(to);
        let current: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        env.storage().persistent().set(&key, &(current + amount));
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Balance(id))
            .unwrap_or(0)
    }

    /// Real arithmetic, so an underfunded transfer genuinely fails — that is
    /// the behaviour the pool's error paths are checked against.
    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) -> Result<(), TokenError> {
        if amount < 0 {
            return Err(TokenError::NegativeAmount);
        }
        let from_key = DataKey::Balance(from);
        let from_bal: i128 = env.storage().persistent().get(&from_key).unwrap_or(0);
        if from_bal < amount {
            return Err(TokenError::InsufficientBalance);
        }
        let to_key = DataKey::Balance(to);
        let to_bal: i128 = env.storage().persistent().get(&to_key).unwrap_or(0);

        env.storage().persistent().set(&from_key, &(from_bal - amount));
        env.storage().persistent().set(&to_key, &(to_bal + amount));
        Ok(())
    }
}
