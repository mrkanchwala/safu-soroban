//! Typed contract errors — replaces the previous `panic!("SAFU: ...")`
//! string-based failure path. One flat enum for the whole contract (not
//! one per module): Soroban's error surface is a single discriminant
//! space per deployed contract, so splitting by module would only add
//! `From`/enum-of-enums plumbing without changing the on-chain ABI shape.
//! Variant names are module-prefixed-in-spirit via grouping/comments
//! below, not via separate types, so the flat numbering stays a single
//! source of truth. See `outputs/2026-07-31_plan-eng-review-safu-soroban-typed-errors.md`
//! (research-ops repo) for why this conversion was done and the
//! module-by-module rollout order (admin -> stake -> claim).

use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum PoolError {
    // -- shared / cross-module (1-9) --
    NoStake = 1,
    AlreadyWithdrawn = 2,
    PoolCapNotPositive = 3,

    // -- admin.rs (10-19) --
    AlreadyInitialized = 10,
    OracleEqualsCoSigner = 11,
    CoSignerEqualsAdmin = 12,
    NewAdminEqualsCoSigner = 13,
    PoolCapBelowTotalStaked = 14,
    CoSignerEqualsOracle = 15,

    // -- stake.rs (20-39) --
    StakeNotPositive = 20,
    StakeOutOfRange = 21,
    AlreadyStaked = 22,
    PoolCapExceeded = 23,
    BeneficiaryIsStaker = 24,
    BeneficiaryIsOracle = 25,
    BeneficiaryIsAdmin = 26,
    BeneficiaryIsCoSigner = 27,
    StakeForfeited = 28,
    ClaimActive = 29,
    PenaltyLockActive = 30,
    WrongBeneficiary = 31,
    NoActiveStake = 32,

    // -- claim.rs (40-79) --
    InvalidTier = 40,
    CallerNotOracleOrAdmin = 41,
    EntitlementNotPositive = 42,
    StakeSuspended = 43,
    ClaimAlreadyActiveForStake = 44,
    EntitlementExceedsTierCap = 45,
    HackTimestampInFuture = 46,
    HackPredatesStake = 47,
    ClaimWindowExpired = 48,
    Insolvent = 49,
    DailyStressCapExceeded = 50,
    OracleDailyClaimLimitReached = 51,
    ClaimAlreadyExists = 52,
    NoSuchClaim = 53,
    ClaimNotPending = 54,
    TimeGateNotMet = 55,
    ClaimNotAwaitingApproval = 56,
    ApprovalWindowExpired = 57,
    ApprovalWindowNotExpired = 58,
    ClaimNotActive = 59,
    ClaimFullyStreamed = 60,
    ClaimNotStale = 61,
    CooldownNotPassed = 62,
    NothingVested = 63,
    DailyOutflowCapReached = 64,
    ClaimNotCancellable = 65,
    CallerNotAdminOrCoSigner = 66,
    OverrideParamsMismatch = 67,
    ClaimAlreadyCompleted = 68,
    WalletHasDifferentActiveClaim = 69,
    NoStakeAmountForOverride = 70,
    NoPendingOverride = 71,
    CallerNotAdmin = 72,
}
