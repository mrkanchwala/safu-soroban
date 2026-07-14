//! Admin: one-time init + oracle/coSigner rotation. Ported from V8's
//! constructor + `setOracle`/`setCoSigner`. The 2-of-2 override
//! request/approval flow lives in claim.rs instead (Task 3) — it operates
//! on Claim/OverrideRequest records and is conceptually part of the
//! claims subsystem, not admin config.

use soroban_sdk::{Address, Env};

use crate::storage;
use crate::types::StakeRecord;

/// Reinitialization guard (vuln checklist V6) — `has()` check before any
/// state is written, not just before returning early.
/// The Tranche 1 deploy-time `pool_cap` value (600,000 XLM, approximating
/// V8's 60 ETH cap) is documented in README.md, not as a dead constant
/// here — `pool_cap` is a plain `initialize` argument, never hardcoded
/// into contract logic, and stays admin-adjustable afterward via
/// `set_pool_cap` (mirrors V8's mutable `maxPoolSize`). A constant that's
/// never referenced by any code path belongs in deploy docs, not source.
pub fn initialize(
    env: &Env,
    admin: &Address,
    oracle: &Address,
    co_signer: &Address,
    xlm_token: &Address,
    pool_cap: i128,
) {
    if env.storage().instance().has(&crate::storage::DataKey::Admin) {
        panic!("SAFU: already initialized");
    }
    // V8 (S4 audit checklist item): oracle != coSigner enforced at
    // construction and at every setter. V8 constructor also requires
    // coSigner != owner (msg.sender) — missing from the first build pass,
    // found on full source read (2026-07-14).
    if oracle == co_signer {
        panic!("SAFU: oracle cannot equal coSigner");
    }
    if co_signer == admin {
        panic!("SAFU: coSigner must differ from admin");
    }
    if pool_cap <= 0 {
        panic!("SAFU: pool cap must be positive");
    }

    admin.require_auth();

    storage::set_admin(env, admin);
    storage::set_oracle(env, oracle);
    storage::set_co_signer(env, co_signer);
    storage::set_xlm_token(env, xlm_token);
    storage::set_pool_cap(env, pool_cap);
    storage::set_total_staked(env, 0);
    storage::set_total_allocated(env, 0);
    storage::set_total_stakers(env, 0);
    storage::set_paused(env, false);
    storage::bump_instance_ttl(env);
}

/// V8: transferOwnership override — new admin cannot equal coSigner. No
/// renounce equivalent is provided (V8 overrides renounceOwnership to
/// always revert — the absence of a renounce function here achieves the
/// same "cannot renounce" outcome by construction, not by omission).
pub fn transfer_admin(env: &Env, new_admin: &Address) {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let co_signer = storage::get_co_signer(env);
    if new_admin == &co_signer {
        panic!("SAFU: new admin cannot equal coSigner");
    }
    storage::set_admin(env, new_admin);
    storage::bump_instance_ttl(env);
}

/// V8: pause()/unpause() — Pausable circuit breaker, admin-only. Blocks
/// stake/withdraw/submit_claim/claim_stream/unlock_pending_claim/
/// approve_override while paused; emergency_exit (stake.rs) remains the
/// pause-time exit path. Missing entirely from the first two build
/// passes — found on full source read.
pub fn pause(env: &Env) {
    let admin = storage::get_admin(env);
    admin.require_auth();
    storage::set_paused(env, true);
}

pub fn unpause(env: &Env) {
    let admin = storage::get_admin(env);
    admin.require_auth();
    storage::set_paused(env, false);
}

/// V8: suspendStake/unsuspendStake — blocks payout eligibility, does NOT
/// block principal withdrawal (suspended stakers can still exit).
pub fn suspend_stake(env: &Env, wallet: &Address) {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let mut record: StakeRecord = storage::get_stake(env, wallet).expect("SAFU: no stake");
    if record.amount <= 0 {
        panic!("SAFU: no stake");
    }
    if record.withdrawn {
        panic!("SAFU: already withdrawn");
    }
    record.suspended = true;
    storage::set_stake(env, wallet, &record);
}

pub fn unsuspend_stake(env: &Env, wallet: &Address) {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let mut record: StakeRecord = storage::get_stake(env, wallet).expect("SAFU: no stake");
    if record.amount <= 0 {
        panic!("SAFU: no stake");
    }
    record.suspended = false;
    storage::set_stake(env, wallet, &record);
}

/// Mirrors V8's `setPoolSize` — admin-adjustable operational cap.
pub fn set_pool_cap(env: &Env, new_cap: i128) {
    let admin = storage::get_admin(env);
    admin.require_auth();

    if new_cap <= 0 {
        panic!("SAFU: pool cap must be positive");
    }
    // Don't allow shrinking below what's already staked — would make the
    // pool immediately "full" or leave min/max stake bounds inconsistent
    // with live state.
    if new_cap < storage::get_total_staked(env) {
        panic!("SAFU: pool cap below current total staked");
    }
    storage::set_pool_cap(env, new_cap);
    storage::bump_instance_ttl(env);
}

pub fn set_oracle(env: &Env, new_oracle: &Address) {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let co_signer = storage::get_co_signer(env);
    if new_oracle == &co_signer {
        panic!("SAFU: oracle cannot equal coSigner");
    }
    storage::set_oracle(env, new_oracle);
    storage::bump_instance_ttl(env);
}

pub fn set_co_signer(env: &Env, new_co_signer: &Address) {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let oracle = storage::get_oracle(env);
    if new_co_signer == &oracle {
        panic!("SAFU: coSigner cannot equal oracle");
    }
    // Found 2026-07-14 via a full re-read of V8's setCoSigner (line 935):
    // it checks `newCoSigner != owner()` too, not just != oracle. Combined
    // with transferOwnership's `newOwner != coSigner` check (already
    // matched below) and initialize's constructor check, V8 blocks
    // coSigner == admin from EVERY direction — it is a genuinely
    // unreachable state, not "allowed after construction" as an earlier
    // draft here assumed (that assumption was never verified against
    // source until this pass; the override flow's degenerate-case
    // handling below is V8's own defensive/dead code for a state that
    // provably cannot occur, kept for exact parity, not because it's
    // reachable).
    if new_co_signer == &admin {
        panic!("SAFU: coSigner cannot equal admin");
    }
    storage::set_co_signer(env, new_co_signer);
    storage::bump_instance_ttl(env);
}
