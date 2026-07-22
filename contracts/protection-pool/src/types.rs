//! Data model ported from `SAFUPoolV8.sol`, pool/claims mechanics only.
//! Yield-deployment fields (wstETH/Lido: `wstethDeployed`, `totalDeployed`,
//! `totalDeployedETH`) are deliberately excluded — Tranche 1 scope is the
//! core pool only, no yield venue. See context/knowledge/smartcontract-soroban.md
//! "scope boundary" note (research-ops repo) for why.

use soroban_sdk::{contracttype, Address, BytesN};

// -----------------------------------------------------------------------
// Time constants — Soroban has no native "days"; ledgers close ~5s apart.
// Mirrors DAY_IN_LEDGERS convention from the Soroban SDK reference used to
// build this contract (context/knowledge/smartcontract-soroban.md).
// -----------------------------------------------------------------------

pub const LEDGERS_PER_DAY: u32 = 17_280;
/// Real calendar time, for absolute-timestamp checks (hack_timestamp
/// sanity, claim window, daily-counter day boundaries) — distinct from
/// the LEDGERS_PER_DAY constants above, which drive duration/gate math
/// off `env.ledger().sequence()`. Soroban exposes both `sequence()` (a
/// ledger number) and `timestamp()` (real Unix seconds) on `env.ledger()`;
/// mixing them for their respective purposes is deliberate, not sloppy.
pub const SECONDS_PER_DAY: u64 = 86_400;
/// 30-day window after a hack within which the claim must be submitted,
/// expressed in real seconds (compared against `env.ledger().timestamp()`
/// and the oracle-supplied `hack_timestamp`, both Unix seconds).
pub const CLAIM_WINDOW_SECONDS: u64 = 30 * SECONDS_PER_DAY;
/// Shared percentage-math denominator (tier cap, stress cap, dynamic
/// outflow bps) — same value as STAKE_BPS_DENOMINATOR/TIER_BPS_DENOMINATOR
/// below, kept as its own constant since those two are domain-specific.
pub const BPS_DENOMINATOR: i128 = 10_000;

/// 90-day time gate before a stake is claim-eligible (V8: CLAIM_MIN_DAYS).
pub const TIME_GATE_LEDGERS: u32 = 90 * LEDGERS_PER_DAY;
/// 7-day cooldown between claim activation and first payout stream.
pub const COOLDOWN_LEDGERS: u32 = 7 * LEDGERS_PER_DAY;
/// 45-day linear vesting window, starting at cooldown end (V8: VESTING).
pub const VESTING_LEDGERS: u32 = 45 * LEDGERS_PER_DAY;
/// 365-day re-stake lock applied only on a false-positive cancel, never on
/// a genuine paid claim (that forfeiture is permanent — do not conflate).
pub const PENALTY_LOCK_LEDGERS: u32 = 365 * LEDGERS_PER_DAY;

/// Points burn-on-claim mechanism (locked 2026-07-22, task plan
/// `outputs/2026-07-22_task-plan-safu-points-burn-mechanism.md`, eng review
/// `outputs/2026-07-22_plan-eng-review-safu-points-burn-mechanism.md`).
///
/// Rule A: once a claim's 90-day time gate is met, the staker has this long
/// to actively call `approve_claim` (which burns their full lifetime points
/// balance and starts forfeiture/cooldown/vesting) before the reservation
/// expires back to the pool via the permissionless `expire_pending_approval`.
pub const APPROVE_WINDOW_LEDGERS: u32 = 100 * LEDGERS_PER_DAY;
/// Rule B: once approved and streaming, a single rolling inactivity clock
/// per claim (not per-day-siloed — the vesting model is one continuous
/// running total, so per-day tracking would need new data; a rolling
/// clock resetting on every `claim_stream` call is the simpler, correct
/// choice, per the eng review). If a claim goes this long with zero
/// `claim_stream` activity, whatever remains uncollected sweeps back to
/// the pool via the permissionless `expire_stale_claim`. Anchored at
/// `cooldown_ends_ledger` on approval, never at the approval moment itself
/// — the 7-day mandatory cooldown must never count as staker inactivity
/// (eng review blocker #2).
pub const COLLECTION_INACTIVITY_LEDGERS: u32 = 100 * LEDGERS_PER_DAY;

// -----------------------------------------------------------------------
// Stake bounds — dynamic, as basis points of the configurable pool cap,
// not fixed amounts. Decided 2026-07-14 (user): V8's fixed ETH bounds
// (STAKE_MIN=0.01, STAKE_MAX=0.75, MAX_POOL_ETH=60) imply MAX = exactly
// 1.25% of pool cap and MIN = 0.0167% of pool cap — a per-staker
// concentration limit (max) plus a spam-resistance floor (min), not
// arbitrary numbers. Recomputing these as basis-points-of-pool-cap
// instead of fixed amounts makes the same design portable across chains
// with different pool sizes (EVM/Solana/BNB relaunches) without
// re-deriving bounds by hand each time — this is meant to be the reusable
// base pattern, not a Soroban-only fix.
// MAX_STAKE_BPS reproduces V8's ratio exactly (125/10_000 = 1.25%).
// MIN_STAKE_BPS is a clean round number close to V8's actual 0.0167% —
// at a 60 XLM-equivalent pool this is 0.012 vs V8's 0.01, a deliberate
// approximation for a clean constant rather than reproducing a fraction.
// Confirm with user before mainnet if exact reproduction matters.
// -----------------------------------------------------------------------

pub const MIN_STAKE_BPS: i128 = 2; // 0.02% of pool cap
pub const MAX_STAKE_BPS: i128 = 125; // 1.25% of pool cap — matches V8 exactly
pub const STAKE_BPS_DENOMINATOR: i128 = 10_000;

// -----------------------------------------------------------------------
// Tier / coverage
// -----------------------------------------------------------------------

/// Coverage = stake × tier_ratio × TIER_COVERAGE_BPS / 10_000. Corrected
/// 2026-07-14 (full source read): V8 keeps ratio and coverage-percentage
/// as TWO separate knobs (`_tierCap`), not one flat multiplier — an admin
/// can lower TIER_COVERAGE_BPS pool-wide without touching the ratios. An
/// earlier draft collapsed these into one number per tier, silently
/// losing that adjustability — fixed here, not carried forward.
/// Tier encoding matches V8: 1=A, 2=B, 3=C (not 0-indexed).
pub const TIER_A_RATIO: i128 = 15;
pub const TIER_B_RATIO: i128 = 10;
pub const TIER_C_RATIO: i128 = 5;
pub const TIER_COVERAGE_BPS: i128 = 10_000; // 100% of ratio, admin-adjustable
pub const TIER_BPS_DENOMINATOR: i128 = 10_000;

// -----------------------------------------------------------------------
// Claim state machine — full 6 states, ported from V8's Claim.status.
// Corrected 2026-07-14 eng review: earlier research had only captured a
// simplified active/forfeited binary; V8's actual enum is richer and a
// 2-state model would silently collapse real transitions.
// -----------------------------------------------------------------------

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ClaimStatus {
    Unused = 0,
    Active = 1,
    Completed = 2,
    Cancelled = 3,
    Reserved = 4,
    PendingTime = 5,
    /// NEW 2026-07-22 (Rule A): 90-day gate met, reservation live, nothing
    /// forfeited yet — waiting on the staker's own `approve_claim` call
    /// within `APPROVE_WINDOW_LEDGERS`. Replaces the old behavior where
    /// meeting the time gate auto-forfeited the stake with no staker
    /// action required.
    AwaitingApproval = 6,
    /// NEW 2026-07-22: terminal state for a reservation that lapsed
    /// without staker action — either Rule A (never approved in time) or
    /// Rule B (approved, then went inactive too long during collection).
    /// Same downstream handling either way: release whatever's still
    /// reserved, stop further action. Distinct from `Cancelled`, which
    /// implies an admin false-positive judgment call and its associated
    /// penalty-lock logic — an expiry is neither party's fault.
    Expired = 7,
}

// -----------------------------------------------------------------------
// StakeRecord — ported from V8's StakeRecord struct, minus yield fields.
// -----------------------------------------------------------------------

#[contracttype]
#[derive(Clone, Debug)]
pub struct StakeRecord {
    /// sha256(beneficiary) — plaintext beneficiary address is never stored.
    /// Decided 2026-07-14: sha256 not keccak256 (Soroban-native, consistent
    /// with the Ed25519 oracle-auth decision; no cross-chain hash-matching
    /// requirement exists for this field).
    pub beneficiary_hash: BytesN<32>,
    pub amount: i128,
    pub staked_at_ledger: u32,
    /// Real Unix timestamp at stake time — used ONLY for the absolute
    /// hack_timestamp sanity check in submit_claim ("hack can't predate
    /// the stake"). staked_at_ledger remains the source of truth for all
    /// duration/gate math (time gate, cooldown, vesting, penalty lock).
    pub staked_at_timestamp: u64,
    /// Set by cancel_claim on a false-positive reversal; blocks withdraw
    /// for PENALTY_LOCK_LEDGERS. 0 if never penalized.
    pub penalty_locked_until_ledger: u32,
    pub withdrawn: bool,
    /// Admin can block payout eligibility; does NOT block principal withdrawal.
    pub suspended: bool,
    /// CHANGED 2026-07-22 (eng review, bug 2 fix): was a bare `claim_active:
    /// bool`. A bool alone can't distinguish "re-executing/correcting THIS
    /// SAME claim" from "wallet already has a DIFFERENT claim in flight" —
    /// `execute_override` needs that distinction to enforce the one-wallet-
    /// one-claim invariant, which a bool structurally cannot express.
    /// `Some(claim_id)` while any claim is open against this stake (blocks
    /// withdrawal, same as the old bool); `None` once terminal
    /// (Completed/Cancelled/Expired) or never claimed.
    pub active_claim_id: Option<BytesN<32>>,
}

// -----------------------------------------------------------------------
// Claim — ported from V8's Claim struct.
// -----------------------------------------------------------------------

#[contracttype]
#[derive(Clone, Debug)]
pub struct Claim {
    pub wallet: Address,
    pub tx_hash: BytesN<32>,
    /// Ledger timestamp of the hack event, validated at submit_claim.
    pub hack_timestamp: u64,
    /// Total approved payout.
    pub entitlement: i128,
    /// Already streamed to the beneficiary so far.
    pub streamed: i128,
    /// Stake amount captured at submit_claim — used by cancel_claim to
    /// restore total_staked on a false-positive reversal.
    pub stake: i128,
    pub cooldown_ends_ledger: u32,
    pub vesting_ends_ledger: u32,
    /// CORRECTED 2026-07-14 (second pass, full source read): NOT "fixed at
    /// end of cooldown" as an earlier note claimed. Activation happens
    /// synchronously — either inside submit_claim itself (gate already
    /// met) or inside unlock_pending_claim (gate met later) — and
    /// cooldown_ends/vesting_ends are set as FUTURE deadlines starting
    /// from that same activation moment, not preconditions for it. The
    /// snapshot is total_staked read immediately before THIS claim's own
    /// forfeiture decrement, in that same call. Still matters for the
    /// same reason: get the timing wrong and the outflow cap's
    /// anti-manipulation guarantee breaks.
    pub total_staked_snapshot: i128,
    /// Assessed by the oracle at claim time, included in its signed
    /// verdict — never re-derivable/forgeable client-side.
    pub tier: u32,
    pub status: ClaimStatus,
    /// NEW 2026-07-22 (Rule A): set when entering `AwaitingApproval`
    /// (`now + APPROVE_WINDOW_LEDGERS`). Checked by both `approve_claim`
    /// (must call before this) and the permissionless `expire_pending_approval`
    /// (may sweep after this). Unused (0) before that state is reached.
    pub approve_deadline_ledger: u32,
    /// NEW 2026-07-22 (Rule B): set to `cooldown_ends_ledger` — not the
    /// approval ledger — the moment a claim activates (eng review blocker
    /// #2: the mandatory 7-day cooldown must never count as staker
    /// inactivity). Updated to `now` on every successful `claim_stream`.
    /// Checked by the permissionless `expire_stale_claim`. Unused (0)
    /// before activation.
    pub last_collected_ledger: u32,
}

// -----------------------------------------------------------------------
// OverrideRequest — 2-of-2 (oracle + coSigner) override flow.
// -----------------------------------------------------------------------

#[contracttype]
#[derive(Clone, Debug)]
pub struct OverrideRequest {
    pub wallet: Address,
    pub tx_hash: BytesN<32>,
    pub entitlement: i128,
    pub tier: u32,
    /// The actual approving address, not a bare bool — found via
    /// adversarial review (2026-07-14, Solodit "operator retains power
    /// after removal" pattern class): a bool can't tell a CURRENT
    /// admin/coSigner's approval apart from a STALE one left over from
    /// before a set_co_signer/transfer_admin rotation. Readiness is
    /// derived by comparing these against the CURRENT admin/coSigner at
    /// execution time, so a rotation automatically invalidates an old
    /// approval instead of letting it silently carry forward.
    pub owner_approver: Option<Address>,
    pub co_signer_approver: Option<Address>,
}
