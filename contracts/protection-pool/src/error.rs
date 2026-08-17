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

    // -- claim.rs / D1 on-chain Ed25519 oracle verification (73-79) --
    //
    // APPENDED at 73, never renumbered into 1-72. Error codes are public
    // ABI (skills.stellar.org/skills/smart-contracts/development.md) — a
    // client already matching on `Insolvent = 49` must keep matching on it
    // across this upgrade.
    //
    // Deliberate gap, documented rather than hidden: there is NO error code
    // for "signature did not verify." `env.crypto().ed25519_verify` returns
    // `()` and traps on mismatch (soroban-sdk 27.0.0 `crypto.rs:152`); there
    // is no `try_` variant and a host trap cannot be recovered in-guest. The
    // four codes below exist precisely so every *recoverable* failure
    // reports as a typed error and only a genuine cryptographic mismatch
    // reaches the opaque trap — see `verify_oracle_signature` in claim.rs
    // for the ordering that guarantees this, and note the trap lands before
    // any storage write, so a rejected signature is state-safe.
    /// `deadline` had already passed at submission time.
    SignatureExpired = 73,
    /// Oracle attestation pubkey absent from instance storage. The oracle
    /// claim path is fail-closed until admin sets it; the admin path is
    /// unaffected.
    OraclePubKeyNotSet = 74,
    /// This exact approval payload was revoked by admin before submission.
    ApprovalRevoked = 75,
    /// `deadline` is further out than `MAX_APPROVAL_WINDOW_SECONDS`. Bounds
    /// how long one signed approval can stay live, and is what makes the
    /// revocation TTL provably outlive every legal deadline — see the
    /// const-assert in types.rs.
    SignatureDeadlineTooFar = 76,

    // -- vault.rs / D2 DeFindex vault integration (80-92) --
    //
    // (Named `vault.rs`, not `yield.rs` — `yield` is a reserved Rust keyword
    // and cannot be a module name; see lib.rs. Comment corrected 2026-08-17,
    // 7a audit Finding 6, along with the range: 93-99 is now the pause gate,
    // so D2's block ends at 92.)
    //
    // APPENDED at 80, never renumbered into 1-76. Starts at 80 rather than 77
    // so the 73-79 block stays reserved for the D1 signature layer — error
    // codes are public ABI and a client matching on `Insolvent = 49` must keep
    // matching on it across every upgrade.
    //
    // 80 is the one that matters operationally. Before D2, `total_staked` was
    // identical to the contract's real XLM balance by construction, so every
    // outbound transfer was guaranteed to have funds behind it. Once XLM can
    // sit in the vault the two diverge, and a transfer could fail against a
    // real balance the accounting number knows nothing about. Left unhandled
    // that surfaces as an opaque SAC host trap — the same
    // indistinguishable-failure shape flagged on the scanner's ChainAbuse
    // path. Every outbound transfer therefore pre-checks liquidity and
    // reports this typed code instead, so a staker learns "retry after
    // rebalance" rather than seeing a trap.
    /// Contract's liquid XLM balance is below what this transfer needs.
    /// Principal is not lost — it is in the yield vault. Admin calls
    /// `provide_liquidity` to redeem, then the caller retries.
    InsufficientLiquidity = 80,
    /// No vault address configured. Every yield path is fail-closed until
    /// admin calls `set_vault`; the pool simply holds XLM until then.
    VaultNotSet = 81,
    /// Deploying this much would exceed `total_staked * deploy_bps`.
    DeployExceedsCeiling = 82,
    /// Deploying this much would leave liquid XLM below `total_allocated`,
    /// i.e. would put already-reserved claim entitlements into the vault.
    DeployBreachesAllocation = 83,
    /// Refused to repoint `Vault` while shares are still held at the old
    /// one — the accounting would say deployed while the new vault holds
    /// nothing, stranding the position outside contract logic.
    VaultChangeWhileDeployed = 84,
    /// Redeem request exceeds the shares this contract actually holds.
    RedeemExceedsDeployed = 85,
    /// Amount/share argument was zero or negative.
    AmountNotPositive = 86,
    /// `deploy_bps` above `MAX_DEPLOY_BPS`.
    DeployBpsTooHigh = 87,
    /// No treasury address configured for yield extraction.
    TreasuryNotSet = 88,
    /// Nothing is deployed, so there is nothing to redeem or extract.
    NothingDeployed = 89,
    /// Requested yield withdrawal exceeds the excess above staker principal.
    ExceedsYieldBalance = 90,
    /// Vault minted fewer shares than the caller's `min_shares_out` floor.
    MinSharesNotMet = 91,
    /// Redemption returned less XLM than the caller's `min_xlm_out` floor.
    MinAmountNotMet = 92,

    // -- shared / pause gate (93-99) --
    //
    // APPENDED at 93, never renumbered into 1-92. Error codes are public ABI.
    //
    // ADDED 2026-08-17 (7a audit, Finding 5). `storage::require_not_paused`
    // was the single surviving `panic!("SAFU: paused")` from the 2026-07-31
    // typed-error conversion — the only bare panic left in production code.
    // It sits on a load-bearing gate (`stake`, `set_beneficiary`, `withdraw`,
    // `submit_claim`, `unlock_pending_claim`, `approve_claim`, `claim_stream`,
    // `approve_override`, `deploy_to_vault` all route through it), so a
    // paused pool reported as an opaque host trap that clients could not
    // match on — and which fuzzing frameworks read as a crash rather than an
    // expected rejection (Veridise Soroban checklist: prefer
    // `panic_with_error!`/typed errors precisely so fuzzers can tell the
    // difference).
    /// Pool is paused. `stake::emergency_exit` and `vault::provide_liquidity`
    /// remain callable by design — they are the pause-time escape path.
    Paused = 93,
}
