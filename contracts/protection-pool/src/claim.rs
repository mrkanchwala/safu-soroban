//! Claims: submit → activate (7d cooldown, either immediately or via
//! unlock_pending_claim once the 90-day time gate is met) → claim_stream
//! (45d vesting, FCFS-per-day daily outflow cap) → completed, or
//! cancel_claim (false-positive reversal, 365-day penalty lock only when
//! the stake had already been forfeited). Also owns the 2-of-2
//! (admin + coSigner) override request/approval flow, a separate escape
//! hatch that bypasses the oracle/time-gate path entirely.
//!
//! Ported against context/knowledge/smartcontract-soroban.md §1b
//! (research-ops repo) — verified line-by-line against SAFUPoolV8.sol,
//! not assumed from a mechanics summary.
//!
//! Auth model: `submit_claim` takes an explicit `caller: Address`
//! (oracle or admin), requires `caller.require_auth()`. Soroban's host
//! authenticates the exact function + argument tuple invoked — there is
//! no separate signed byte-blob to design; the call's own arguments ARE
//! the oracle's attested verdict. This is what closed the "payload shape"
//! open question the original stub left flagged.
//!
//! `cancel_pending_override`: V8's full function inventory (confirmed via
//! a full source read, 2026-07-14) includes this name but its exact
//! mechanics were not captured in the KB passes done so far — implemented
//! here as the conservative reading (admin or coSigner resets an
//! unexecuted override request back to unapproved). Flagged as an
//! approximation, not a verified port — confirm against the live V8
//! source before this ships to mainnet.

use soroban_sdk::{contractevent, Address, Bytes, BytesN, Env};
use soroban_sdk::token::TokenClient;
use soroban_sdk::xdr::ToXdr;

use crate::stake;
use crate::storage;
use crate::types::{
    Claim, ClaimStatus, OverrideRequest, StakeRecord, BPS_DENOMINATOR, CLAIM_WINDOW_SECONDS,
    COOLDOWN_LEDGERS, PENALTY_LOCK_LEDGERS, TIER_A_RATIO, TIER_B_RATIO, TIER_C_RATIO,
    TIER_BPS_DENOMINATOR, TIER_COVERAGE_BPS, TIME_GATE_LEDGERS, VESTING_LEDGERS,
};

// -----------------------------------------------------------------------
// Events — #[contractevent] pattern, see stake.rs for the same note.
// -----------------------------------------------------------------------

#[contractevent]
pub struct ClaimSubmitted {
    #[topic]
    pub wallet: Address,
    pub claim_id: BytesN<32>,
    pub entitlement: i128,
}

#[contractevent]
pub struct ClaimUnlocked {
    #[topic]
    pub wallet: Address,
    pub claim_id: BytesN<32>,
}

#[contractevent]
pub struct ClaimStreamed {
    #[topic]
    pub wallet: Address,
    pub amount: i128,
}

#[contractevent]
pub struct ClaimCancelled {
    #[topic]
    pub wallet: Address,
    pub claim_id: BytesN<32>,
}

#[contractevent]
pub struct OverrideExecuted {
    #[topic]
    pub wallet: Address,
    pub claim_id: BytesN<32>,
}

// -----------------------------------------------------------------------
// Helpers — exact formulas, per KB §1b.
// -----------------------------------------------------------------------

fn tier_ratio(tier: u32) -> i128 {
    match tier {
        1 => TIER_A_RATIO,
        2 => TIER_B_RATIO,
        3 => TIER_C_RATIO,
        _ => panic!("SAFU: invalid tier"),
    }
}

/// `stake × tier_ratio × TIER_COVERAGE_BPS / 10_000` — two knobs (ratio,
/// coverage-bps), not one flat multiplier. Panics on an invalid tier.
pub fn tier_cap(stake_amount: i128, tier: u32) -> i128 {
    stake_amount * tier_ratio(tier) * TIER_COVERAGE_BPS / TIER_BPS_DENOMINATOR
}

/// Admission-side daily cap on new-claim entitlement. Separate counter
/// from the payout-side daily outflow cap below.
pub fn stress_cap(env: &Env) -> i128 {
    let total_staked = storage::get_total_staked(env);
    if total_staked == 0 {
        return 0;
    }
    let total_allocated = storage::get_total_allocated(env);
    let utilization_bps = total_allocated * BPS_DENOMINATOR / total_staked;
    let rate_bps = if utilization_bps < 2_000 {
        2_500
    } else if utilization_bps < 5_000 {
        1_000
    } else {
        300
    };
    total_staked * rate_bps / BPS_DENOMINATOR
}

/// Payout-side daily outflow cap rate, as a function of pool utilization
/// against `base` (the caller passes `max(total_staked_now,
/// claim.total_staked_snapshot)` — the anti-manipulation guarantee this
/// protects depends on that exact base, not on this function).
pub fn dynamic_outflow_bps(env: &Env, base: i128) -> i128 {
    if base == 0 {
        return 100;
    }
    let total_allocated = storage::get_total_allocated(env);
    let utilization_bps = total_allocated * BPS_DENOMINATOR / base;
    if utilization_bps < 2_000 {
        500
    } else if utilization_bps < 5_000 {
        300
    } else {
        100
    }
}

fn current_day(env: &Env) -> u32 {
    (env.ledger().timestamp() / crate::types::SECONDS_PER_DAY) as u32
}

/// Claim id is derived on-chain from (wallet, tx_hash) — never trusted as
/// a caller-supplied value, including in the override flow.
fn compute_claim_id(env: &Env, wallet: &Address, tx_hash: &BytesN<32>) -> BytesN<32> {
    let mut buf: Bytes = wallet.to_xdr(env);
    buf.append(&Bytes::from_array(env, &tx_hash.to_array()));
    env.crypto().sha256(&buf).to_bytes()
}

// -----------------------------------------------------------------------
// Activation — shared by submit_claim's gate-met branch, unlock_pending_
// claim, and the override flow. Forfeits the stake, banks final points,
// snapshots total_staked BEFORE this claim's own decrement (per the
// corrected timing note in types.rs::Claim::total_staked_snapshot), and
// sets fresh cooldown/vesting deadlines from "now".
// -----------------------------------------------------------------------

fn activate_claim(env: &Env, stake_record: &mut StakeRecord, claim: &mut Claim) {
    let points = stake::compute_points_for_record(env, stake_record);
    if points > 0 {
        let banked = storage::get_points_balance(env, &claim.wallet);
        storage::set_points_balance(env, &claim.wallet, banked + points);
    }

    let stake_amount = stake_record.amount;
    stake_record.withdrawn = true;
    stake_record.amount = 0;

    let total_staked = storage::get_total_staked(env);
    claim.total_staked_snapshot = total_staked;
    storage::set_total_staked(env, total_staked - stake_amount);
    storage::set_total_stakers(env, storage::get_total_stakers(env).saturating_sub(1));

    let now_ledger = env.ledger().sequence();
    claim.stake = stake_amount;
    claim.cooldown_ends_ledger = now_ledger + COOLDOWN_LEDGERS;
    claim.vesting_ends_ledger = claim.cooldown_ends_ledger + VESTING_LEDGERS;
    claim.status = ClaimStatus::Active;
}

// -----------------------------------------------------------------------
// submit_claim
// -----------------------------------------------------------------------

/// Caller must literally be the oracle or the admin (this is the oracle's
/// backend service calling directly once its off-chain scanner finishes,
/// or the admin acting as a manual fallback — not a relayer forwarding a
/// third party's signed blob). Validation order matches KB §1b exactly.
pub fn submit_claim(
    env: &Env,
    caller: &Address,
    wallet: &Address,
    tx_hash: &BytesN<32>,
    entitlement: i128,
    tier: u32,
    hack_timestamp: u64,
) -> BytesN<32> {
    storage::require_not_paused(env);

    let admin = storage::get_admin(env);
    let oracle = storage::get_oracle(env);
    if caller != &oracle && caller != &admin {
        panic!("SAFU: caller must be oracle or admin");
    }
    caller.require_auth();

    if entitlement <= 0 {
        panic!("SAFU: entitlement must be positive");
    }
    tier_ratio(tier); // panics on invalid tier

    let mut stake_record: StakeRecord = storage::get_stake(env, wallet).expect("SAFU: no stake");
    if stake_record.amount <= 0 {
        panic!("SAFU: no active stake");
    }
    if stake_record.withdrawn {
        panic!("SAFU: stake already withdrawn");
    }
    if stake_record.suspended {
        panic!("SAFU: stake suspended");
    }
    if stake_record.claim_active {
        panic!("SAFU: claim already active for this stake");
    }

    let cap = tier_cap(stake_record.amount, tier);
    if entitlement > cap {
        panic!("SAFU: entitlement exceeds tier cap");
    }

    let now_ts = env.ledger().timestamp();
    if hack_timestamp > now_ts {
        panic!("SAFU: hack timestamp in the future");
    }
    if hack_timestamp < stake_record.staked_at_timestamp {
        panic!("SAFU: hack predates stake");
    }
    if now_ts > hack_timestamp + CLAIM_WINDOW_SECONDS {
        panic!("SAFU: claim window expired");
    }

    let total_staked = storage::get_total_staked(env);
    let total_allocated = storage::get_total_allocated(env);
    if total_allocated + entitlement > total_staked {
        panic!("SAFU: insolvent");
    }

    let day = current_day(env);
    let (day_entitlement, day_count) = storage::get_daily_entitlement(env, day);
    if day_entitlement + entitlement > stress_cap(env) {
        panic!("SAFU: daily stress cap exceeded");
    }

    let is_oracle_caller = caller == &oracle;
    if is_oracle_caller {
        let total_stakers = storage::get_total_stakers(env);
        let limit = (total_stakers / 10).max(1);
        if day_count >= limit {
            panic!("SAFU: oracle daily claim-count limit reached");
        }
    }

    let claim_id = compute_claim_id(env, wallet, tx_hash);
    if let Some(existing) = storage::get_claim(env, &claim_id) {
        if existing.status != ClaimStatus::Unused {
            panic!("SAFU: claim already exists");
        }
    }

    storage::set_daily_entitlement(
        env,
        day,
        day_entitlement + entitlement,
        day_count + if is_oracle_caller { 1 } else { 0 },
    );
    storage::set_total_allocated(env, total_allocated + entitlement);

    stake_record.claim_active = true;

    let now_ledger = env.ledger().sequence();
    let gate_met = now_ledger.saturating_sub(stake_record.staked_at_ledger) >= TIME_GATE_LEDGERS;

    let mut claim = Claim {
        wallet: wallet.clone(),
        tx_hash: tx_hash.clone(),
        hack_timestamp,
        entitlement,
        streamed: 0,
        stake: stake_record.amount,
        cooldown_ends_ledger: 0,
        vesting_ends_ledger: 0,
        total_staked_snapshot: 0,
        tier,
        status: ClaimStatus::PendingTime,
    };

    if gate_met {
        activate_claim(env, &mut stake_record, &mut claim);
    }

    storage::set_stake(env, wallet, &stake_record);
    storage::set_claim(env, &claim_id, &claim);
    storage::bump_instance_ttl(env);

    ClaimSubmitted {
        wallet: wallet.clone(),
        claim_id: claim_id.clone(),
        entitlement,
    }
    .publish(env);

    claim_id
}

// -----------------------------------------------------------------------
// unlock_pending_claim — permissionless by design (staker, oracle, or
// admin can all call it once the time gate is met; no require_auth).
// -----------------------------------------------------------------------

pub fn unlock_pending_claim(env: &Env, claim_id: &BytesN<32>) {
    storage::require_not_paused(env);

    let mut claim: Claim = storage::get_claim(env, claim_id).expect("SAFU: no such claim");
    if claim.status != ClaimStatus::PendingTime {
        panic!("SAFU: claim not pending");
    }

    let mut stake_record: StakeRecord =
        storage::get_stake(env, &claim.wallet).expect("SAFU: no stake");
    let now_ledger = env.ledger().sequence();
    if now_ledger < stake_record.staked_at_ledger + TIME_GATE_LEDGERS {
        panic!("SAFU: time gate not yet met");
    }

    activate_claim(env, &mut stake_record, &mut claim);

    storage::set_stake(env, &claim.wallet, &stake_record);
    storage::set_claim(env, claim_id, &claim);
    storage::bump_instance_ttl(env);

    ClaimUnlocked {
        wallet: claim.wallet.clone(),
        claim_id: claim_id.clone(),
    }
    .publish(env);
}

// -----------------------------------------------------------------------
// claim_stream — pull-payment, staker-authorized (the ONE claims
// function the staker/wallet itself authorizes; everything else is
// oracle/admin/permissionless).
// -----------------------------------------------------------------------

pub fn claim_stream(env: &Env, claim_id: &BytesN<32>, beneficiary: &Address) -> i128 {
    storage::require_not_paused(env);

    let mut claim: Claim = storage::get_claim(env, claim_id).expect("SAFU: no such claim");
    claim.wallet.require_auth();

    if claim.status != ClaimStatus::Active {
        panic!("SAFU: claim not active");
    }
    let now_ledger = env.ledger().sequence();
    if now_ledger < claim.cooldown_ends_ledger {
        panic!("SAFU: cooldown not passed");
    }
    if claim.streamed >= claim.entitlement {
        panic!("SAFU: fully streamed");
    }

    let stake_record: StakeRecord =
        storage::get_stake(env, &claim.wallet).expect("SAFU: no stake");
    let expected_hash = env.crypto().sha256(&beneficiary.to_xdr(env)).to_bytes();
    if expected_hash != stake_record.beneficiary_hash {
        panic!("SAFU: wrong beneficiary");
    }

    let elapsed_end = now_ledger.min(claim.vesting_ends_ledger);
    let elapsed = elapsed_end.saturating_sub(claim.cooldown_ends_ledger) as i128;
    let vested_total = claim.entitlement * elapsed / (VESTING_LEDGERS as i128);
    let claimable = vested_total - claim.streamed;
    if claimable <= 0 {
        panic!("SAFU: nothing vested yet");
    }

    let day = current_day(env);
    let daily_outflow_so_far = storage::get_daily_outflow(env, day);
    let cap_base = storage::get_total_staked(env).max(claim.total_staked_snapshot);
    let bps = dynamic_outflow_bps(env, cap_base);
    let cap = cap_base * bps / BPS_DENOMINATOR;
    let available_today = (cap - daily_outflow_so_far).max(0);
    let transfer_amount = claimable.min(available_today);
    if transfer_amount <= 0 {
        panic!("SAFU: daily outflow cap reached, try again tomorrow");
    }

    claim.streamed += transfer_amount;
    if claim.streamed >= claim.entitlement {
        claim.status = ClaimStatus::Completed;
    }
    storage::set_daily_outflow(env, day, daily_outflow_so_far + transfer_amount);
    storage::set_claim(env, claim_id, &claim);
    storage::bump_instance_ttl(env);

    ClaimStreamed {
        wallet: claim.wallet.clone(),
        amount: transfer_amount,
    }
    .publish(env);

    let token = TokenClient::new(env, &storage::get_xlm_token(env));
    token.transfer(&env.current_contract_address(), beneficiary, &transfer_amount);

    transfer_amount
}

// -----------------------------------------------------------------------
// cancel_claim — admin-only, false-positive reversal.
// -----------------------------------------------------------------------

pub fn cancel_claim(env: &Env, claim_id: &BytesN<32>) {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let mut claim: Claim = storage::get_claim(env, claim_id).expect("SAFU: no such claim");
    if claim.status != ClaimStatus::Active && claim.status != ClaimStatus::PendingTime {
        panic!("SAFU: claim not cancellable");
    }

    let unstreamed = claim.entitlement - claim.streamed;
    let total_allocated = storage::get_total_allocated(env);
    storage::set_total_allocated(env, (total_allocated - unstreamed).max(0));

    let mut stake_record: StakeRecord =
        storage::get_stake(env, &claim.wallet).expect("SAFU: no stake");

    // Active: the stake was already forfeited at activation — restore it
    // and apply the 365-day penalty lock. PendingTime: the stake was
    // never forfeited (no agency over the oracle's submission) — nothing
    // to restore, no penalty, just unblock withdrawal below.
    if claim.status == ClaimStatus::Active {
        stake_record.withdrawn = false;
        stake_record.amount = claim.stake;
        stake_record.penalty_locked_until_ledger = env.ledger().sequence() + PENALTY_LOCK_LEDGERS;
        storage::set_total_staked(env, storage::get_total_staked(env) + claim.stake);
        storage::set_total_stakers(env, storage::get_total_stakers(env) + 1);
    }

    stake_record.claim_active = false;
    storage::set_stake(env, &claim.wallet, &stake_record);

    claim.status = ClaimStatus::Cancelled;
    storage::set_claim(env, claim_id, &claim);
    storage::bump_instance_ttl(env);

    ClaimCancelled {
        wallet: claim.wallet.clone(),
        claim_id: claim_id.clone(),
    }
    .publish(env);
}

// -----------------------------------------------------------------------
// 2-of-2 override — admin + coSigner, each submits identical params once;
// the second matching call executes. Bypasses the oracle/time-gate path
// entirely; the multi-sig itself is the security gate.
// -----------------------------------------------------------------------

pub fn approve_override(
    env: &Env,
    caller: &Address,
    wallet: &Address,
    tx_hash: &BytesN<32>,
    entitlement: i128,
    tier: u32,
) {
    storage::require_not_paused(env);

    let admin = storage::get_admin(env);
    let co_signer = storage::get_co_signer(env);
    if caller != &admin && caller != &co_signer {
        panic!("SAFU: caller must be admin or coSigner");
    }
    caller.require_auth();

    if entitlement <= 0 {
        panic!("SAFU: entitlement must be positive");
    }
    tier_ratio(tier);

    let claim_id = compute_claim_id(env, wallet, tx_hash);

    let mut req: OverrideRequest =
        storage::get_override(env, &claim_id).unwrap_or(OverrideRequest {
            wallet: wallet.clone(),
            tx_hash: tx_hash.clone(),
            entitlement,
            tier,
            owner_approved: false,
            co_signer_approved: false,
        });

    if req.entitlement != entitlement || req.tier != tier {
        panic!("SAFU: override params mismatch with pending request");
    }

    if caller == &admin {
        req.owner_approved = true;
    }
    if caller == &co_signer {
        req.co_signer_approved = true;
    }

    // Degenerate case: admin and coSigner are the same address — one
    // approval suffices.
    let ready = (req.owner_approved && req.co_signer_approved) || co_signer == admin;

    if ready {
        execute_override(env, &claim_id, &req);
        // Reset so a stale approved state can't replay after execution.
        storage::set_override(
            env,
            &claim_id,
            &OverrideRequest {
                wallet: wallet.clone(),
                tx_hash: tx_hash.clone(),
                entitlement,
                tier,
                owner_approved: false,
                co_signer_approved: false,
            },
        );
    } else {
        storage::set_override(env, &claim_id, &req);
    }
}

fn execute_override(env: &Env, claim_id: &BytesN<32>, req: &OverrideRequest) {
    let existing = storage::get_claim(env, claim_id);
    if let Some(ref c) = existing {
        if c.status == ClaimStatus::Completed {
            panic!("SAFU: claim already completed");
        }
        let unstreamed = c.entitlement - c.streamed;
        let total_allocated = storage::get_total_allocated(env);
        storage::set_total_allocated(env, (total_allocated - unstreamed).max(0));
    }

    let mut stake_record: StakeRecord =
        storage::get_stake(env, &req.wallet).expect("SAFU: no stake");

    // If a prior (now-superseded) claim already forfeited this stake, the
    // basis for the tier cap / restoration bookkeeping is that claim's
    // recorded stake amount, not the (now-zeroed) StakeRecord.amount.
    let original_stake_amount = if stake_record.withdrawn {
        existing.as_ref().map(|c| c.stake).unwrap_or(0)
    } else {
        stake_record.amount
    };
    if original_stake_amount <= 0 {
        panic!("SAFU: no stake amount to base override on");
    }

    let cap = tier_cap(original_stake_amount, req.tier);
    if req.entitlement > cap {
        panic!("SAFU: entitlement exceeds tier cap");
    }

    // If this stake was already forfeited (a same-claim-id re-execution —
    // e.g. resetting cooldown with identical terms), that principal left
    // total_staked on the FIRST execution and is now earmarked reserve
    // capacity for exactly this claim, not fresh pool capacity being
    // drawn down again. Add it back for this check only, so re-execution
    // is evaluated against the same pool view the first execution saw —
    // found via a failing test (a same-params re-approval incorrectly
    // reported insolvent because total_staked no longer included this
    // stake's own already-forfeited principal).
    let total_staked = storage::get_total_staked(env);
    let effective_total_staked = if stake_record.withdrawn {
        total_staked + original_stake_amount
    } else {
        total_staked
    };
    let total_allocated = storage::get_total_allocated(env);
    if total_allocated + req.entitlement > effective_total_staked {
        panic!("SAFU: insolvent");
    }
    storage::set_total_allocated(env, total_allocated + req.entitlement);

    let mut claim = Claim {
        wallet: req.wallet.clone(),
        tx_hash: req.tx_hash.clone(),
        hack_timestamp: env.ledger().timestamp(),
        entitlement: req.entitlement,
        streamed: 0,
        stake: original_stake_amount,
        cooldown_ends_ledger: 0,
        vesting_ends_ledger: 0,
        total_staked_snapshot: 0,
        tier: req.tier,
        status: ClaimStatus::Active,
    };

    if !stake_record.withdrawn {
        activate_claim(env, &mut stake_record, &mut claim);
    } else {
        let now_ledger = env.ledger().sequence();
        claim.cooldown_ends_ledger = now_ledger + COOLDOWN_LEDGERS;
        claim.vesting_ends_ledger = claim.cooldown_ends_ledger + VESTING_LEDGERS;
        claim.total_staked_snapshot = storage::get_total_staked(env);
    }
    stake_record.claim_active = true;

    storage::set_stake(env, &req.wallet, &stake_record);
    storage::set_claim(env, claim_id, &claim);
    storage::bump_instance_ttl(env);

    OverrideExecuted {
        wallet: req.wallet.clone(),
        claim_id: claim_id.clone(),
    }
    .publish(env);
}

/// APPROXIMATED, not verified against source (see module doc comment) —
/// admin or coSigner resets an unexecuted override request back to
/// unapproved, so a wrong submission doesn't stay stuck forever pending
/// the other party's approval of bad params.
pub fn cancel_pending_override(
    env: &Env,
    caller: &Address,
    wallet: &Address,
    tx_hash: &BytesN<32>,
) {
    let admin = storage::get_admin(env);
    let co_signer = storage::get_co_signer(env);
    if caller != &admin && caller != &co_signer {
        panic!("SAFU: caller must be admin or coSigner");
    }
    caller.require_auth();

    let claim_id = compute_claim_id(env, wallet, tx_hash);
    let req: OverrideRequest =
        storage::get_override(env, &claim_id).expect("SAFU: no pending override");

    // NOT `req.owner_approved && req.co_signer_approved` — execute_override
    // always resets both flags to false right after executing (replay
    // guard), so that condition can never actually observe "already
    // executed"; it would just silently no-op on a stale request instead
    // of telling the caller why. Check the Claim record's real status
    // instead — execute_override always leaves it Active or Completed.
    if let Some(existing) = storage::get_claim(env, &claim_id) {
        if existing.status == ClaimStatus::Active || existing.status == ClaimStatus::Completed {
            panic!("SAFU: override already executed");
        }
    }
    let _ = req; // existence already confirmed via storage::get_override above

    // Fully cleared, not reset-in-place — a corrected resubmission with
    // different entitlement/tier must be treated as genuinely fresh, not
    // compared against these now-abandoned values.
    storage::remove_override(env, &claim_id);
}
