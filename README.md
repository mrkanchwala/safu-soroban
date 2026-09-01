# SAFU ProtectionPool: Soroban

[![CI](https://github.com/mrkanchwala/safu-soroban/actions/workflows/ci.yml/badge.svg)](https://github.com/mrkanchwala/safu-soroban/actions/workflows/ci.yml)

`ProtectionPool` is a Soroban (Stellar smart contracts, Rust → WASM) loss-
socialization pool, built for SCF #44 and delivered in three tranches:

- **Tranche 1** — the core mechanics: pool accounting, points, tiers, claims
  and the payout stream. Deployed to testnet, approved 2026-08-08.
- **Tranche 2** — on-chain Ed25519 oracle verification, plus the yield layer:
  idle pool capital is deployed into Blend through a DeFindex vault, wired to
  the live contract via `set_vault`. Merged and live on testnet; approved
  2026-08-26.
- **Tranche 3** — mainnet. Closes the three operational gaps Tranches 1 and 2
  deliberately left open: an on-chain admission retry queue, permissionless
  bidirectional liquidity rebalancing, and an atomic pool+vault deploy. Code
  merged 2026-09-01; mainnet deploy follows the SCF-funded audit.

**Scope note:** this repo is the on-chain `ProtectionPool` contract only.
SAFU's fraud-detection scanner, the system that decides whether a given
transaction qualifies as a wallet drain, is a separate, proprietary asset.
Its code, logic, and signal weights are not included, referenced, or
reproduced anywhere in this repository.

**Status:** Tranche 3 code merged 2026-09-01; Tranche 2 is the code currently
live on Stellar testnet. **278 unit tests pass on the merged tree.**

**The full-workspace mutation regression against the merged Tranche 3 tree has
run (2026-09-01): 805 mutants — 782 caught, 22 unviable, 1 survivor, zero
timeouts, and zero new contract defects.** The single survivor is the
already-documented `vault.rs:418` `authorize_withdraw` mutant, unobservable
because the test environment auto-approves authorization; it is the same mutant
recorded at the pre-merge line 375. Earlier scoped campaigns are kept as history:
Tranche 2 diff 400 mutants / 389 caught / 1 survivor, and a Tranche 3 diff
campaign of 141 mutants with 140 of 140 viable killed. See `TESTING.md` §3 for
the methodology and the survivor's full triage.

Compiles to WASM via
`stellar contract build --optimize`, `/audit-chain` +
`/cso` security passes both PASS (0 CRIT/HIGH/MEDIUM — see §7 of
`TESTING.md`),
fuzzed 151,398 runs with zero crashes. **Live contract ID:**
`CDTXVIA4TSQ6PY76VFD4BBW4R4UMGSE5HTBNAMASAPRYRNV37DBDJJBB` (see
"Testnet deployment (Tranche 2, current)" below). **Error handling:** every public entrypoint
returns `Result<T, PoolError>` via a typed `#[contracterror]` enum
(`src/error.rs`) rather than raw panics. Converted 2026-07-31, see
"Error handling" below.

## Testnet deployment (Tranche 2, current)

**This is the live contract.** Deployed and initialized on Stellar testnet
2026-08-20, carrying the merged Tranche 2 code: on-chain Ed25519 oracle
verification (D1), the DeFindex yield vault integration (D2), and the audit
fixes from the combined `/audit-chain` + `/cso` pass.

- **Contract ID:** `CDTXVIA4TSQ6PY76VFD4BBW4R4UMGSE5HTBNAMASAPRYRNV37DBDJJBB`
  ([Stellar Expert](https://stellar.expert/explorer/testnet/contract/CDTXVIA4TSQ6PY76VFD4BBW4R4UMGSE5HTBNAMASAPRYRNV37DBDJJBB))
- **Deploy tx:** [`e146fed40e89...`](https://stellar.expert/explorer/testnet/tx/e146fed40e894d77ba89a9febc6cbb1e0fd6ce4092363c3df2055ea00b4b3d2a)
- **Initialize tx:** [`a10ff8fffcdb...`](https://stellar.expert/explorer/testnet/tx/a10ff8fffcdb618398ea2e946891ed52bf8c0503a5632130d466c9973c2a36d3)
- **WASM hash:** `62ca8a24acf4fdb262ae479587924fb36bf5604421b895a0b8b7accfb5eaed3a`
  — the Tranche 2 build. The current tree hashes differently since the
  Tranche 3 merge; see "Building" for both values and why.
- **Yield vault (D2):** `CCSS44GDUI4TDTLX2XAGPWVVDOZPBTGSFLIBT6DXYLGU74ACF76EE5HZ`,
  SAFU's own DeFindex vault with `VaultFee = 0`, wired via `set_vault`
- **Pool cap:** 600,000 XLM (`6_000_000_000_000` stroops), unchanged from
  Tranche 1 (see "Deploy-time arguments" below).
- **XLM asset:** native testnet SAC
  `CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC`
- **Oracle:** an AWS KMS-held Ed25519 key. The contract verifies its
  signature on-chain; the private key exists only inside KMS and every
  signing call is logged. Admin, co-signer and treasury are fresh testnet
  identities. Public addresses only, no private keys are shared here or
  anywhere in this repository.

Unlike the Tranche 1 deployment below, this pool carries **real activity**: a
staked fleet, approved claims, and a live yield-vault position, all
independently checkable on Stellar Expert.

### Verify a payout yourself

Claims reach `Active` and then wait out a contract-enforced 7-day cooldown
before any XLM can move. That cooldown is deliberately **not** shortened for
demonstration purposes, so rather than asking anyone to take a recording on
faith, [`reviewer-kit/`](reviewer-kit/) contains a script and the testnet keys
to trigger and verify a real payout directly against this contract, on your own
schedule. See [`reviewer-kit/README.md`](reviewer-kit/README.md).

## Testnet deployment (Tranche 1, superseded)

Kept for the record. This was the Tranche 1 deliverable, deployed and
initialized 2026-07-29, and it is **no longer the active contract**. It does
not contain the Tranche 2 code and carries none of the Tranche 2 activity.

- **Contract ID:** `CCQT2VRONZTE5ODBNM3XAQWUPQRLKGMU4MMLA2JK6HJHJMK34Q7ZFTGJ`
  ([Stellar Expert](https://stellar.expert/explorer/testnet/contract/CCQT2VRONZTE5ODBNM3XAQWUPQRLKGMU4MMLA2JK6HJHJMK34Q7ZFTGJ))
- **Deploy tx:** [`eec5cadee7b7...`](https://stellar.expert/explorer/testnet/tx/eec5cadee7b7f836479c0131a1e666fd6f2a07affe835b5c6b9b97e7fe0822dd)
- **Initialize tx:** [`7c37662f87df...`](https://stellar.expert/explorer/testnet/tx/7c37662f87df89397e66927fd4e4355cca2eb9aa27d9f870b0ed08f0b6bd82b6)

Verified live immediately after initialization: `get_total_staked()`
returns `0`, `is_paused()` returns `false`.

## Protection pool or Stellar integration? (SCF #44 reviewer response)

SAFU's Tranche 1 deliverable, the ProtectionPool contract, is a
protection pool, not a staking protocol: participants deposit XLM
directly into the pool, a wallet-drain event triggers a tiered on-chain
payout, and the deposited principal funds it. Stellar runs on the
Stellar Consensus Protocol, not proof-of-stake, so deposits are coverage
contributions rather than network stakes. No external protocol is
called from this contract.

Tranche 2 adds the Stellar integration: SAFU deploys idle pool capital
into Blend via DeFindex, and the yield generated becomes protocol
revenue rather than depositor yield. T1 is the protection mechanism; T2
is where the integration with an existing Stellar DeFi protocol happens.

## Building

```bash
cargo check --package protection-pool          # fast type/logic check
cargo test --package protection-pool           # 278 unit tests
cargo build --package protection-pool --release --target wasm32v1-none   # plain, unoptimized WASM
stellar contract build --optimize              # optimized — this is what was deployed
```

The **WASM hash published under "Testnet deployment" below is produced by
`stellar contract build --optimize`**, not by the plain `cargo build` line
above — the two emit different artifacts, and only the optimized one
matches the deployed contract.

**There are two hashes, and they differ on purpose:**

- `62ca8a24acf4fdb262ae479587924fb36bf5604421b895a0b8b7accfb5eaed3a` — the
  **Tranche 2 build**, and what is actually deployed at `CDTXVIA4…`.
  Reproducible from tag `t2-scf-submission-2026-08-20`; verified 2026-08-25.
- `2cec7e749d46b96a392be85dd284b8a988261ab7716b2218c7a5f39bbe2162db` — what
  **the current tree** builds to as of 2026-09-01, after the Tranche 3 merge
  and the `soroban-sdk` 27.0.6 lockfile bump. Not deployed anywhere yet; the
  Tranche 3 mainnet deploy follows the SCF-funded audit.

The testnet deployment is deliberately **not** being re-pointed at the current
tree. `CDTXVIA4…` is cited in the approved Tranche 2 submission and in the
Tranche 3 form's contract-address list, so it stays as the record of what was
reviewed rather than moving mid-programme.

Full testing methodology (coverage, mutation testing, fuzzing, static
analysis, manual audit passes, security reviews): see `TESTING.md`.

Requires: `rustc`/`cargo`, the `wasm32v1-none` target
(`rustup target add wasm32v1-none`), and `stellar-cli`
(`brew install stellar-cli`, or see
[developers.stellar.org](https://developers.stellar.org)).

### Resource-cost profiling

```bash
cargo test --package protection-pool profiling -- --nocapture
```

Measures CPU-instruction and memory cost per hot-path entrypoint via
`soroban-sdk`'s built-in budget tracker (`env.cost_estimate().budget()`),
local only, no deploy. Results as of 2026-07-14 (native-Rust test-host
execution; real WASM costs run somewhat higher, see the caveat in
`src/test/profiling_tests.rs`):

| Entrypoint | CPU instructions | Memory (bytes) |
|---|---|---|
| `stake` | 465,824 | 160,601 |
| `withdraw` | 494,378 | 163,572 |
| `submit_claim` (immediate activation) | 476,538 | 151,656 |
| `approve_override` (execution branch) | 396,624 | 130,903 |
| `claim_stream` (partial vesting, hits the daily-outflow-cap path) | 589,472 | 216,761 |

All comfortably under 1% of Soroban's typical ~100M-instruction
per-transaction CPU budget, no efficiency concerns found at this
profiling depth. `claim_stream` is the most expensive entrypoint, which
tracks with it doing the most computation (vesting math + daily-outflow-
cap math + a token transfer) in one call.

### Fuzzing

```bash
cd contracts/protection-pool
cargo +nightly fuzz run fuzz_solvency -- -max_total_time=300
cargo +nightly fuzz run fuzz_override -- -max_total_time=300
```

Two targets: `fuzz_solvency` (the core invariant) and `fuzz_override`
(the 2-of-2 admin+coSigner escape hatch). Requires nightly Rust
(`rustup toolchain install nightly`) and `cargo-fuzz`
(`cargo install cargo-fuzz`). **On macOS (Apple Silicon), this currently
crashes at libFuzzer's own startup** (`flockfile`/`vfprintf`, before any
fuzz iteration runs), a host ASan/libc incompatibility confirmed
unrelated to this contract, reproduced independently on 2026-07-14 and
again on 2026-07-22. Two working options: `Dockerfile.fuzz`
(`docker build -f Dockerfile.fuzz -t safu-fuzz . && docker run --rm
safu-fuzz`), or run natively on any Linux host (no Docker needed there,
since the incompatibility is macOS-specific).

**Current Tranche 2 result — 2026-08-17, Docker on the team's Linux
host, run against the exact commit deployed to testnet:**
`fuzz_solvency` 71,034 runs / `fuzz_override` 80,364 runs = **151,398
fuzzed action-sequences, zero crashes, zero artifacts, zero
solvency-invariant violations.** This is the figure quoted at the top of
this README and in the Tranche 2 submission.

Three earlier campaigns ran against pre-Tranche-2 code (2026-07-14,
07-15 and 07-22, **133,672 runs**, also all clean); they are kept in
`TESTING.md` §4 as history rather than as the current claim.

### Deploy-time arguments

`__constructor(admin, oracle, oracle_pubkey, co_signer, xlm_token, pool_cap)`

Pass these as constructor arguments **on the deploy itself**, e.g.
`stellar contract deploy --wasm <path> -- --admin <G...> --oracle <G...>
--oracle_pubkey <hex32> --co_signer <G...> --xlm_token <C...> --pool_cap
6000000000000`. There is no separate initialization call.

**CHANGED 2026-08-17 (7a audit, Finding 3): this was `initialize`, a separate
entrypoint.** It authorized the `admin` **argument** handed to it, and the
only thing preventing a second call was the `AlreadyInitialized` guard,
which stops re-initialization but not being *first*. Between the deploy
transaction and the legitimate init transaction, anyone watching the chain
could call `initialize` naming themselves admin. The T1 deployment recorded
above used two separate transactions, so that window was real, not
hypothetical. `__constructor` runs inside the deploy invocation, so the gap
no longer exists and a failed validation aborts the deployment atomically
rather than leaving a half-configured contract. Note this could not have been
retrofitted later: a contract deployed without a constructor can never gain
one, which is why it changed before D4 rather than after.

**`oracle_pubkey`** was added by Tranche 2 / D1 and was previously missing
from this line (Finding 6). The oracle has two distinct identities: an
`Address` (policy: auth, rate limit, beneficiary guard, admin invariants) and
a 32-byte Ed25519 pubkey (attestation: `ed25519_verify` over the signed claim
approval). Both are required at construction so a deployment can never hold a
working oracle Address with no attestation key. Rotate with
`set_oracle_identity(new_oracle, new_pubkey)` . Prefer it over `set_oracle` +
`set_oracle_pubkey` in sequence, which leaves a window where the two
identities disagree and every oracle-path claim fails closed (see `admin.rs`).

`pool_cap` is a plain argument, never hardcoded into contract logic, and
stays admin-adjustable afterward via `set_pool_cap`. The intended Tranche 1
deploy value: **600,000 XLM** = `6_000_000_000_000` stroops
(1 XLM = 10,000,000 stroops).

## Error handling

Every fallible public entrypoint returns `Result<T, PoolError>`, a typed
`#[contracterror]` enum (`src/error.rs`, 77 variants), instead of
`panic!`, the standard modern Soroban convention. Callers get a typed
error code, not just an opaque host trap. Converted 2026-07-31 from an
earlier all-`panic!` version: same validation conditions, same order,
same business logic, verified via a full line-by-line diff review (zero
logic drift) plus a fresh 485-mutant re-run (§3 of `TESTING.md`). The
conversion changed the failure *signal* only. Test callers use the
SDK-generated `try_X()` client methods; the plain `X()` methods still
auto-unwrap/panic on `Err`, so no caller ergonomics changed either.

## Contract layout

| File | Role |
|---|---|
| `src/lib.rs` | Public entrypoints (`#[contractimpl]`), thin wrappers over the modules below, plus read-only view functions |
| `src/types.rs` | Data model (`StakeRecord`, `Claim`, `OverrideRequest`, `ClaimStatus`) and all tuning constants |
| `src/storage.rs` | Storage key layout + TTL-bump helpers (see below) |
| `src/admin.rs` | Init, oracle/coSigner/admin rotation, pause, suspend |
| `src/stake.rs` | Stake, withdraw, `setBeneficiary`, `emergencyExit`, points computation |
| `src/claim.rs` | Full claim lifecycle (submit → activate → stream → complete/cancel) + the 2-of-2 admin+coSigner override escape hatch + D1 on-chain Ed25519 oracle-approval verification |
| `src/vault.rs` | D2 yield layer (Tranche 2): DeFindex vault deployment, redemption, bidirectional rebalancing, yield extraction |
| `src/error.rs` | Typed `#[contracterror] PoolError` enum — 77 variants, codes are public ABI and never renumbered |
| `src/test/` | 278 unit tests across 13 modules plus `common.rs`: by mechanic (`admin_tests`, `stake_tests`, `claim_tests`, `override_tests`, `solvency_tests`), by tranche feature (`d1_signature_tests`, `d2_vault_tests`, `t3_flags_tests`), mutation-gap regressions (`mutation_gap_tests`, `t2_mutation_gap_tests`), plus `profiling_tests` (resource-cost budgets), `pool_demo_tests`, and `blend_scenario_tests` (the SCF reviewer-response scenario) |
| `fuzz/` | `cargo-fuzz` targets for the solvency invariant and claim state machine — the depth-based compensating control alongside Komet property verification (`TESTING.md` §5b) |

## Storage model

Soroban gives three storage tiers with different TTL/cost semantics. This
contract's placement, and why:

| Tier | Used for | Rationale |
|---|---|---|
| `instance()` | Pool-wide globals: `admin`, `oracle`, `co_signer`, `xlm_token`, `pool_cap`, `total_staked`, `total_allocated`, `total_stakers`, `paused`, the daily outflow-cap counters (`daily_outflow`/`last_outflow_day`), the daily admission-cap counters (`daily_entitlement_total`/`last_entitlement_day`/`daily_claim_count`) | Every call already loads all of `instance()`. Never put per-user or unbounded data here. All of these are fixed-size scalars. |
| `persistent()` | Per-staker (`Stake(Address)`), per-claim (`ClaimRec(BytesN<32>)`), per-override-request (`Override(BytesN<32>)`), banked points (`PointsBalance(Address)`) | Distributed across separate keys, not grown as one struct. Bounded per-entity storage, archived and restorable via TTL bumps. |
| `temporary()` | `RevokedApproval(BytesN<32>)` — D1 oracle-approval revocations, keyed by the sha256 of the approval payload | A revocation is only meaningful until the approval's own `deadline` passes; after that `SignatureExpired` rejects the approval regardless, so expiry here is safe. TTL is sized by `types::REVOCATION_TTL_LEDGERS` and guarded by a compile-time `const _: () = assert!` that the TTL outlives every legal deadline. |

**On struct field sizing:** reviewed 2026-07-14. Sub-word bit-packing has no
real equivalent here. Soroban's storage cost is driven by read/write *count*,
TTL-extension frequency, and total serialized entry size, not sub-word
bit-packing, and every field in `StakeRecord`/`Claim`/`OverrideRequest`
is already at its minimum meaningful width: `i128` for token amounts
(stroops can exceed `u64` for a large pool), `u32` for ledger sequences
and `u64` for Unix timestamps (Soroban's own native return types;
narrowing either would just add conversion code for no storage saving).
`tier: u32` could theoretically be a byte, but Soroban has no native
`U8` `Val` type to switch to without wrapping overhead that would cost
more than the few bytes it'd save. Nothing changed here as a result;
documented so this doesn't get re-asked next audit pass.

**TTL is never a security mechanism.** The 90-day claim-eligibility gate, the
7-day cooldown, the 45-day vesting window, the 30-day claim-submission
window, and the 365-day false-positive penalty lock are all enforced by
comparing an explicit stored ledger-sequence (or Unix-timestamp) deadline in
contract logic, never by relying on a storage entry's TTL/archival state.
Anyone can extend anyone's TTL via `ExtendFootprintTTLOp` with no contract
auth required, so TTL expiry answers "is this data still cheaply readable,"
never "is this deadline still active."

Full `DataKey` enum (`src/storage.rs`):

```rust
pub enum DataKey {
    // instance(): pool-wide globals
    Admin, Oracle, OraclePubKey, CoSigner, XlmToken, PoolCap,
    TotalStaked, TotalAllocated, TotalStakers, Paused,
    DailyOutflow, LastOutflowDay,
    DailyEntitlementTotal, LastEntitlementDay, DailyClaimCount,
    // instance(): D2 yield layer (Tranche 2)
    Vault, Treasury, DeployBps,
    TotalDeployedShares, TotalDeployedXlm, TotalExtractedYield,
    // persistent(): per-entity
    Stake(Address), ClaimRec(BytesN<32>), Override(BytesN<32>),
    PointsBalance(Address),
    // temporary(): self-expiring
    RevokedApproval(BytesN<32>),
}
```

## Mechanics

- **Tier assessed off-chain by the oracle at claim time** (not stake-amount
  banded), coverage cap = `stake × tier_ratio × TIER_COVERAGE_BPS / 10_000`,
  ratio and coverage-percentage kept as two independently-adjustable knobs.
- **90-day time gate** on claim eligibility. Points accrue while staked and
  bank on exit, but they are **not** permanent: `approve_claim` burns the
  wallet's entire lifetime points balance as the staker-gated cost of
  activating a claim (added 2026-07-22; `claim.rs`, `points_burned` on the
  `ClaimApproved` event).
- **Dynamic outflow cap** on payout streaming (5%/3%/1% by pool
  utilization), first-come-first-served per calendar day with automatic
  carry-forward, not a queue.
- **Separate admission-side stress cap** (25%/10%/3% by utilization) plus an
  oracle-only per-day claim-count rate limit.
- **On-chain solvency invariant**, checked at every `submit_claim` and every
  override execution: `total_allocated + entitlement ≤ total_staked`.
- **Forfeiture is immediate and permanent** on claim activation. The
  365-day re-stake lock is a narrower, separate penalty that only applies
  when `cancel_claim` reverses a false positive, never to a genuine paid
  claim.
- **`StakeRecord.amount` is never zeroed by claim-triggered forfeiture**:
  only `withdrawn = true` is set. Only the *voluntary* `withdraw()` path
  zeroes `amount`. (An earlier draft had this backwards, which cascaded into
  two further bugs before being caught and fixed; see git history.)

## Design choices

- **Auth:** Soroban's native `require_auth()` on the exact function+argument
  tuple. The oracle's authorization *is* the call itself, so there is no
  separate signed byte-blob to design or verify.
- **Beneficiary hash:** `sha256`. Soroban-native, and no cross-chain
  hash-matching requirement exists for this field.
- **Stake bounds:** dynamic basis-points of the (admin-adjustable) pool cap
  rather than fixed amounts — a constant 1.25%-of-pool ratio, portable across
  future chain deploys with different pool sizes.
- **No failed-payout-rescue bucket and no `revokedApprovals` list.** Native
  XLM has no wrap/unwrap step and no trustline-failure mode on the payout
  path, and Soroban's native auth already handles replay and nonces without a
  custom revocation list. (Yield was out of Tranche 1 scope and arrived in
  Tranche 2.)

## Known open items before mainnet

- **Outflow cap deviates from the SCF #44 submitted grant text**, which
  commits Tranche 1 to a flat "2%/day" payout cap. What's built here is
  a dynamic 5%/3%/1% cap — a deliberate choice to build the real mechanic
  rather than the grant text's simplified description. This section is the
  disclosure: SCF reviewed the Tranche 1
  deliverables against this repository, returned no comments, and approved
  the tranche on 2026-08-08.
- **Two static-analysis tools that would normally apply here do not.**
  Neither is a contract defect, and both are worth stating plainly rather
  than leaving implied. Fuzzing is deliberately **not** in this list: it
  covers the current Tranche 2 code, see the assurance paragraph below.
  - **Certora Sunbeam cannot run against this contract.** Evaluated
    2026-09-01: `cvlr-soroban`, Certora's own Soroban spec library, pins
    `soroban-sdk = "22"` at its current default-branch HEAD, and this contract
    is on `soroban-sdk` 27.0.6 — the two cannot coexist in one dependency
    graph. Established by building Certora's own tutorial first. Full account
    in `TESTING.md` §5c. **Komet is the symbolic-verification tool that does
    work here and it has run and passed** — 3 properties, 100 examples each,
    `TESTING.md` §5b.
  - **`cargo-scout-audit` cannot analyse the crate at all.** Scout 0.3.16
    builds against `wasm32-unknown-unknown`; `soroban-sdk` 27 refuses that
    target on Rust 1.82+ and requires `wasm32v1-none`, which this contract
    correctly uses. Scout prints a `0 Critical / 0 Medium / 0 Minor` summary
    row **beside `build failed`**. That row is vacuous, no detector ran, and
    it must not be read as a pass.

  What assurance does rest on: **278 unit tests** (all passing as of
  2026-09-01), **151,398 fuzzed action-sequences against this exact
  Tranche 2 code** (2026-08-17, both targets, zero crashes and zero
  solvency-invariant violations, see "Fuzzing" above), **the full-workspace
  mutation regression on the merged Tranche 3 tree** (2026-09-01 — 805
  mutants, 782 caught, 1 documented survivor, zero new defects), **Komet
  symbolic property verification** (3 properties, 100 examples each, all
  passed — `TESTING.md` §5b), the coverage figures recorded in `TESTING.md`
  §2, and a manual adversarial review checklist. See `TESTING.md` for
  methodology.
- **The contract is not upgradeable.** There is no upgrade authority
  anywhere in it. This is deliberate and is a real positive against
  OWASP SC10, but the tradeoff is explicit: there is no post-deployment
  bug-fix path, so a defect found after mainnet deployment requires a
  migration to a new contract rather than a patch. Confirm this posture is
  still wanted before the mainnet deploy.
- **Residual oracle risk, stated precisely.** A compromised oracle key
  cannot unilaterally cause a payout: `deadline` is bounded in both
  directions, `hack_timestamp` cannot be future-dated, `entitlement` is
  capped by the victim's own stake times their tier, the claim id is the
  nonce, and decisively **the victim must call `approve_claim` themselves**
  with the payout going only to the beneficiary hash committed at stake
  time. Two exposures survive that and belong in the Tranche 3 threat model
  rather than being left implicit in "cannot cause a payout":
  - **Griefing / availability.** A compromised oracle can set
    `active_claim_id`, which blocks both `withdraw` and `emergency_exit`,
    including the pause-time escape hatch. Bounded to roughly 10% of
    stakers per day and remediable by an admin `cancel_claim`, but note
    that nothing permissionless clears a `PendingTime` claim
    (`expire_pending_approval` requires `AwaitingApproval`,
    `expire_stale_claim` requires `Active`).
  - **Collusive drain, not unilateral theft.** Inflated claims that a
    colluding or credulous victim approves. Capped by the stress cap,
    solvency check, cooldown, vesting and outflow cap, and the victim burns
    their entire lifetime points balance. Slow, observable and bounded,
    materially better than a direct vault drain, but it is the real
    exposure and it is architectural, not incidental.
- **`cancel_pending_override`'s mechanics were re-verified from first
  principles** (they were previously an unconfirmed guess) — see the
  `src/claim.rs` module doc comment for the full account of what was wrong
  and what was fixed.
- **✅ CLOSED IN TRANCHE 3 (`006daf3`, merged `1d9469f`) — on-chain admission
  retry queue.** Kept here as the disclosure trail; the description below is
  what Tranches 1–2 shipped and what T3 replaced it with.
  **Was:** a `submit_claim` rejected for capacity (`DailyStressCapExceeded` /
  `OracleDailyClaimLimitReached`) wrote no state and had no automatic
  retry. The underlying incident stays genuine and re-submittable
  once capacity frees up, but nothing currently tracks or re-attempts it
  automatically. Planned for mainnet: **on-chain** queue state (not
  off-chain) that pins the score/tier/entitlement at the moment of
  rejection (not re-derived on retry, so a later resubmission reflects the
  original assessment), publicly readable the same way every other claim
  field already is, with a permissionless function anyone can call to
  re-admit it once capacity frees up, or expire it if the claim window
  runs out first.
- **✅ CLOSED IN TRANCHE 3 (`006daf3`) — permissionless bidirectional
  rebalancing via `ensure_liquidity`/`auto_deploy_liquidity`, with a
  contract-computed 5% slippage bound (`MAX_REBALANCE_SLIPPAGE_BPS`) and
  nothing caller-controlled.**
  **Was:** liquidity rebalancing shipped simple for testnet.
  `claim_stream` payouts relied on a manual admin `provide_liquidity`
  call if too much of the pool is deployed into the D2 yield vault when a
  payout is due. Testnet validates the core deposit→vault→yield mechanism
  first, on the simpler path. Held off for mainnet: a permissionless
  `ensure_liquidity()` redesign where the contract computes its own
  shortfall and slippage bound (nothing caller-controlled), so rebalancing
  doesn't require an admin to notice and act by hand.
- **✅ CLOSED IN TRANCHE 3 (`006daf3`) — `scripts/atomic_pool_vault_deploy.sh`
  bundles pool deploy → vault deploy → `set_vault` into one operator-triggered
  step.** Deliberately still a script, not the pool's constructor, for the
  coupling reason given below.
  **Was:** a manual operator step. Deploy a DeFindex vault via its factory, then
  `set_vault`, run as a separate step right after pool deployment so each
  piece is independently verifiable. Held off for mainnet: bundle pool
  deploy → vault deploy → `set_vault` into one atomic, operator-triggered
  deploy script, so a new deployment self-sets-up its vault with no manual
  per-step judgment calls. Deliberately not baked into the pool contract's
  own constructor. A cross-contract call to DeFindex's factory from
  `initialize()` would couple the pool's own deployability to DeFindex's
  factory being live and correctly configured at that exact moment, and
  DeFindex is Stellar-specific, which would not generalize to a future
  chain deployment.

## Blend/YieldBlox illustrative scenario (SCF #44 reviewer response)

Addresses the SCF #44 reviewer comment asking for "a Blend exploit
analysis with simulations showing how the protocol would have helped in
a real incident."

**Disclosure (read before anything else):**
- This is a simulation against a real historical incident, **not** an
  existing or pending relationship with Blend/YieldBlox. SAFU has no live
  protocol-level pool product, and Blend/YieldBlox is not a SAFU
  depositor or partner.
- No scanner detection logic is used, implied, or reproduced anywhere
  here. Whether a transaction is "drain-shaped" is asserted as a labeled
  fixture input in the tests below, never a computed scanner verdict.
  SAFU's actual scanner (what decides whether a real transaction fires)
  is proprietary and lives entirely off-chain, in a separate private repo.
- Only the public entitlement formula (`entitlement = min(stake ×
  tier_ratio, loss)`, 15x/10x/5x by tier, already SAFU's own published
  protocol mechanic) is exercised, run through the real, audited on-chain
  `submit_claim` → `approve_claim` → `claim_stream` entrypoints, not a
  re-derivation or approximation.
- This section demonstrates the payout mechanism only. It is not this
  repo's answer to whether SAFU is fundamentally a staking product or a
  Stellar integration (a separate reviewer comment); that question is
  addressed in the section above.

**Incident:** Blend/YieldBlox, Stellar, oracle manipulation, real tx
`3e81a3f7b6e17cc22d0a1f33e9dcf90e5664b125b9e61f108b8d2f082f2d4657`
(independently verified against Horizon 2026-07-22), ~$10.8M loss,
publicly reported, not proprietary.

**What the tests demonstrate** (`src/test/blend_scenario_tests.rs`,
run with `cargo test --package protection-pool blend_scenario --
--nocapture`): a depositor at the contract's own real `MAX_STAKE` bound
(1.25% of pool cap, "$1M" in this scenario's illustrative $-mapping,
pool cap = "$100M") submits a claim carrying Blend/YieldBlox's real tx
hash. Tier only changes the entitlement passed in; the real on-chain
`tier_cap` check is what actually accepts or would reject it, not a mock:

| Tier | Ratio | Coverage cap (deposit × ratio) | Entitlement (capped at $10.8M loss) | % of real loss covered |
|------|-------|------------------------------|--------------------------------------|------------------------|
| A | 15x | $15.0M | $10.8M | 100% |
| B | 10x | $10.0M | $10.0M | 92.6% |
| C | 5x | $5.0M | $5.0M | 46.3% |

A fourth test, `ordinary_transaction_never_becomes_a_claim`, is the
negative control: an ordinary transaction on the same kind of fixture
pool/participant never becomes a claim at all, no `submit_claim` call,
the deposit stays fully claim-eligible, nothing is ever earmarked for
payout. The point is discernment, not "everything pays out."

## Licence

**Apache-2.0.** Full text in [`LICENSE`](LICENSE); attribution in
[`NOTICE`](NOTICE); declared as `license = "Apache-2.0"` in the workspace
`Cargo.toml` and inherited by `contracts/protection-pool`.

Chosen 2026-08-19 for Stellar Community Fund award #44, whose Build Award
submission criteria require "a clear plan to open-source" any smart contracts
a project includes. Apache-2.0 is OSI-approved, so that requirement is
satisfied outright rather than by interpretation.

The SAFU EVM contracts are **not** SCF-funded, live in a separate repository,
and remain under BUSL-1.1. BUSL is source-available rather than open source,
and was deliberately not carried across to this repository. Visibility alone
does not make code open source, and prior to this licence being added the
absence of any LICENSE file left the repository defaulting to
all-rights-reserved despite being public.

## Full technical reference

The complete mechanics map, SDK reference, vulnerability checklist, and
audit history for this contract live in the SAFU team's
internal ops repo: `context/knowledge/smartcontract-soroban.md`. This
README is the standalone summary for anyone reading this repo on its own.
