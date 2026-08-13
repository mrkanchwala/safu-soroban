//! Admin: one-time init + oracle/coSigner rotation. Ported from V8's
//! constructor + `setOracle`/`setCoSigner`. The 2-of-2 override
//! request/approval flow lives in claim.rs instead (Task 3) — it operates
//! on Claim/OverrideRequest records and is conceptually part of the
//! claims subsystem, not admin config.
//!
//! CONVERTED 2026-07-31: `panic!("SAFU: ...")` -> `Result<_, PoolError>`.
//! Every function that used to trap now returns `Err(PoolError::...)` on
//! the same conditions, in the same order, with no behavior change beyond
//! the caller now getting a typed error code instead of an opaque panic
//! message. See `error.rs` for the enum + `outputs/2026-07-31_plan-eng-review-
//! safu-soroban-typed-errors.md` (research-ops repo) for why.

use soroban_sdk::{Address, BytesN, Env};

use crate::error::PoolError;
use crate::storage;
use crate::types::{ClaimStatus, StakeRecord, APPROVE_WINDOW_LEDGERS};

/// Reinitialization guard (vuln checklist V6) — `has()` check before any
/// state is written, not just before returning early.
/// The Tranche 1 deploy-time `pool_cap` value (600,000 XLM, approximating
/// V8's 60 ETH cap) is documented in README.md, not as a dead constant
/// here — `pool_cap` is a plain `initialize` argument, never hardcoded
/// into contract logic, and stays admin-adjustable afterward via
/// `set_pool_cap` (mirrors V8's mutable `maxPoolSize`). A constant that's
/// never referenced by any code path belongs in deploy docs, not source.
///
/// CHANGED for T2/D1: takes `oracle_pubkey` as well as `oracle`. The oracle
/// has TWO identities in this contract — an `Address` (policy: auth, rate
/// limit, beneficiary guard, admin invariants) and an Ed25519 pubkey
/// (attestation: signature verification). Requiring both here is the
/// "startup invariant that both are set" from the D1 eng review: it removes
/// the window in which a deployment has a working oracle Address but no
/// attestation key. `submit_claim` still fails closed on a missing key
/// (`OraclePubKeyNotSet`) as defence in depth, but with this argument that
/// state is unreachable on a fresh deploy.
#[allow(clippy::too_many_arguments)]
pub fn initialize(
    env: &Env,
    admin: &Address,
    oracle: &Address,
    oracle_pubkey: &BytesN<32>,
    co_signer: &Address,
    xlm_token: &Address,
    pool_cap: i128,
) -> Result<(), PoolError> {
    if env.storage().instance().has(&crate::storage::DataKey::Admin) {
        return Err(PoolError::AlreadyInitialized);
    }
    // V8 (S4 audit checklist item): oracle != coSigner enforced at
    // construction and at every setter. V8 constructor also requires
    // coSigner != owner (msg.sender) — missing from the first build pass,
    // found on full source read (2026-07-14).
    if oracle == co_signer {
        return Err(PoolError::OracleEqualsCoSigner);
    }
    if co_signer == admin {
        return Err(PoolError::CoSignerEqualsAdmin);
    }
    if pool_cap <= 0 {
        return Err(PoolError::PoolCapNotPositive);
    }

    admin.require_auth();

    storage::set_admin(env, admin);
    storage::set_oracle(env, oracle);
    storage::set_oracle_pubkey(env, oracle_pubkey);
    storage::set_co_signer(env, co_signer);
    storage::set_xlm_token(env, xlm_token);
    storage::set_pool_cap(env, pool_cap);
    storage::set_total_staked(env, 0);
    storage::set_total_allocated(env, 0);
    storage::set_total_stakers(env, 0);
    storage::set_paused(env, false);
    storage::bump_instance_ttl(env);
    Ok(())
}

/// V8: transferOwnership override — new admin cannot equal coSigner. No
/// renounce equivalent is provided (V8 overrides renounceOwnership to
/// always revert — the absence of a renounce function here achieves the
/// same "cannot renounce" outcome by construction, not by omission).
pub fn transfer_admin(env: &Env, new_admin: &Address) -> Result<(), PoolError> {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let co_signer = storage::get_co_signer(env);
    if new_admin == &co_signer {
        return Err(PoolError::NewAdminEqualsCoSigner);
    }
    storage::set_admin(env, new_admin);
    storage::bump_instance_ttl(env);
    Ok(())
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
///
/// CHANGED 2026-07-22 (eng review, bug 5 / suspend upgrade): previously
/// only blocked a brand-new `submit_claim` — a claim already filed
/// (PendingTime/AwaitingApproval) still activated on schedule, and an
/// already-Active claim kept streaming, completely unaffected by a
/// suspension applied after the fact. `claim.rs`'s `approve_claim` and
/// `claim_stream` now both check `suspended` directly, so this same
/// `suspended = true` flag now also freezes an in-progress claim.
///
/// CORRECTED 2026-07-22 (adversarial /audit-chain re-review, same day):
/// the original fix above was still dead on arrival — this function's
/// pre-existing `if record.withdrawn { panic! }` guard meant admin could
/// NEVER reach `suspend_stake` on a stake that had already forfeited via
/// `approve_claim`/an override, which is exactly the Active/streaming
/// case the upgrade was meant to reach. `withdrawn` serves double duty in
/// this contract (a genuinely-done voluntary exit, OR forfeiture from an
/// in-progress claim) — the guard needs to allow the second case and
/// still block the first. `active_claim_id` is what actually
/// distinguishes them: `Some` means a live claim still exists (still
/// suspendable), `None` means the wallet is genuinely finished (nothing
/// left to freeze).
pub fn suspend_stake(env: &Env, wallet: &Address) -> Result<(), PoolError> {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let mut record: StakeRecord = storage::get_stake(env, wallet).ok_or(PoolError::NoStake)?;
    if record.amount <= 0 {
        return Err(PoolError::NoStake);
    }
    if record.withdrawn && record.active_claim_id.is_none() {
        return Err(PoolError::AlreadyWithdrawn);
    }
    record.suspended = true;
    storage::set_stake(env, wallet, &record);
    Ok(())
}

/// CHANGED 2026-07-22 (eng review blocker #1): takes an optional
/// `claim_id` so admin can reset whichever new deadline clock (Rule A's
/// `approve_deadline_ledger` or Rule B's `last_collected_ledger`) was
/// running against this wallet's claim while suspended — otherwise a
/// staker could lose their entitlement to Rule A/B expiry purely because
/// admin froze them, through no fault of their own. Resets the relevant
/// clock to "now" (extends the full window fresh) rather than trying to
/// credit back exact suspended duration — simpler, and errs in the
/// staker's favor. A no-op on the clock if `claim_id` is `None`, the
/// claim doesn't exist, or it's in neither AwaitingApproval nor Active
/// (nothing to reset) — admin can still unsuspend freely in those cases.
///
/// CORRECTED 2026-07-22 (adversarial /audit-chain re-review, same day):
/// the original version never checked `claim.wallet == wallet` — an
/// admin passing a mismatched claim_id would reset an unrelated wallet's
/// deadline instead of (or as well as) the one actually being
/// unsuspended. Only exploitable by the admin key itself (already fully
/// trusted throughout this contract — cancel_claim, suspend_stake, etc.
/// all assume a non-malicious admin), so LOW severity, but cheap and
/// worth closing: skip the reset silently on a mismatch rather than
/// acting on the wrong wallet's claim.
pub fn unsuspend_stake(
    env: &Env,
    wallet: &Address,
    claim_id: Option<BytesN<32>>,
) -> Result<(), PoolError> {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let mut record: StakeRecord = storage::get_stake(env, wallet).ok_or(PoolError::NoStake)?;
    if record.amount <= 0 {
        return Err(PoolError::NoStake);
    }
    record.suspended = false;
    storage::set_stake(env, wallet, &record);

    if let Some(id) = claim_id {
        if let Some(mut claim) = storage::get_claim(env, &id) {
            if &claim.wallet != wallet {
                return Ok(());
            }
            let now_ledger = env.ledger().sequence();
            match claim.status {
                ClaimStatus::AwaitingApproval => {
                    claim.approve_deadline_ledger = now_ledger + APPROVE_WINDOW_LEDGERS;
                    storage::set_claim(env, &id, &claim);
                }
                ClaimStatus::Active => {
                    claim.last_collected_ledger = now_ledger;
                    storage::set_claim(env, &id, &claim);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Mirrors V8's `setPoolSize` — admin-adjustable operational cap.
pub fn set_pool_cap(env: &Env, new_cap: i128) -> Result<(), PoolError> {
    let admin = storage::get_admin(env);
    admin.require_auth();

    if new_cap <= 0 {
        return Err(PoolError::PoolCapNotPositive);
    }
    // Don't allow shrinking below what's already staked — would make the
    // pool immediately "full" or leave min/max stake bounds inconsistent
    // with live state.
    if new_cap < storage::get_total_staked(env) {
        return Err(PoolError::PoolCapBelowTotalStaked);
    }
    storage::set_pool_cap(env, new_cap);
    storage::bump_instance_ttl(env);
    Ok(())
}

pub fn set_oracle(env: &Env, new_oracle: &Address) -> Result<(), PoolError> {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let co_signer = storage::get_co_signer(env);
    if new_oracle == &co_signer {
        return Err(PoolError::OracleEqualsCoSigner);
    }
    storage::set_oracle(env, new_oracle);
    storage::bump_instance_ttl(env);
    Ok(())
}

/// Rotates the oracle's Ed25519 ATTESTATION key. Separate from `set_oracle`
/// (which rotates the policy `Address`) — the two are distinct identities,
/// see `storage::DataKey::OraclePubKey`.
///
/// Admin-only, same guard shape as `set_oracle`. Note what is deliberately
/// NOT enforced: nothing checks that this key belongs to the same entity as
/// the current oracle `Address`. It cannot be — an Ed25519 pubkey and a
/// Stellar `Address` have no on-chain relationship to compare, and a
/// contract-type oracle address has no pubkey at all. Keeping them
/// consistent is an operational responsibility.
///
/// Rotation hazard, flagged rather than solved: signatures already in
/// flight under the previous key stop verifying the moment this lands, and
/// (per Blocker 3) they fail as an opaque host trap rather than a named
/// error. Rotate during a quiet window and re-sign anything outstanding. A
/// grace window accepting either key is the obvious mitigation but is
/// deliberately out of D1 scope — it widens the attestation surface and
/// belongs with the T3 hardening pass, not a testnet deliverable.
pub fn set_oracle_pubkey(env: &Env, new_pubkey: &BytesN<32>) -> Result<(), PoolError> {
    let admin = storage::get_admin(env);
    admin.require_auth();

    storage::set_oracle_pubkey(env, new_pubkey);
    storage::bump_instance_ttl(env);
    Ok(())
}

pub fn set_co_signer(env: &Env, new_co_signer: &Address) -> Result<(), PoolError> {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let oracle = storage::get_oracle(env);
    if new_co_signer == &oracle {
        return Err(PoolError::CoSignerEqualsOracle);
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
        return Err(PoolError::CoSignerEqualsAdmin);
    }
    storage::set_co_signer(env, new_co_signer);
    storage::bump_instance_ttl(env);
    Ok(())
}
