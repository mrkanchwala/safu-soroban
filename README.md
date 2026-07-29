# SAFU ProtectionPool — Soroban

Soroban (Stellar smart contracts, Rust → WASM) port of `SAFUPoolV8.sol`, the
live Ethereum mainnet contract at
[`0xa170f0937DEc353C1806eaC0c3d559524d458641`](https://etherscan.io/address/0xa170f0937DEc353C1806eaC0c3d559524d458641).
Built for SCF #44 Tranche 1 — the core pool/points/tier/claim/payout-stream
mechanics, ported wholesale from V8. Yield deployment (Lido wstETH on V8,
DeFindex/Blend planned for Tranche 2) is deliberately excluded from this
scope.

**Scope note:** this repo is the on-chain `ProtectionPool` contract only.
SAFU's fraud-detection scanner, the system that decides whether a given
transaction qualifies as a wallet drain, is a separate, proprietary asset.
Its code, logic, and signal weights are not included, referenced, or
reproduced anywhere in this repository.

**Status:** core mechanics complete and tested (178 unit tests, 97.47%
line coverage, 100% of catchable mutants killed — both confirmed
2026-07-22 against the current code, see `TESTING.md` for the full
methodology), compiles to WASM
(`cargo build --release --target wasm32v1-none`), `/audit-chain` +
`/cso` security passes both PASS (0 CRIT/HIGH/MEDIUM — see `audits/`),
**deployed to Stellar testnet** — contract ID
`CCQT2VRONZTE5ODBNM3XAQWUPQRLKGMU4MMLA2JK6HJHJMK34Q7ZFTGJ` (see
"Testnet deployment" below).

## Testnet deployment

Deployed and initialized on Stellar testnet, 2026-07-29:

- **Contract ID:** `CCQT2VRONZTE5ODBNM3XAQWUPQRLKGMU4MMLA2JK6HJHJMK34Q7ZFTGJ`
  ([Stellar Expert](https://stellar.expert/explorer/testnet/contract/CCQT2VRONZTE5ODBNM3XAQWUPQRLKGMU4MMLA2JK6HJHJMK34Q7ZFTGJ))
- **Deploy tx:** [`eec5cadee7b7...`](https://stellar.expert/explorer/testnet/tx/eec5cadee7b7f836479c0131a1e666fd6f2a07affe835b5c6b9b97e7fe0822dd)
- **Initialize tx:** [`7c37662f87df...`](https://stellar.expert/explorer/testnet/tx/7c37662f87df89397e66927fd4e4355cca2eb9aa27d9f870b0ed08f0b6bd82b6)
- **Pool cap:** 600,000 XLM (`6_000_000_000_000` stroops) — the intended
  Tranche 1 deploy value (see "Deploy-time arguments" below).
- **XLM asset:** native testnet SAC
  `CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC`
- **Admin / oracle / co-signer:** fresh testnet identities generated for
  this deployment — public addresses only, no private keys are shared
  here or anywhere in this repository.

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
cargo test --package protection-pool           # 173 unit tests
cargo build --package protection-pool --release --target wasm32v1-none
stellar contract build --optimize              # or: stellar contract optimize --wasm <path>
```

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
`soroban-sdk`'s built-in budget tracker (`env.cost_estimate().budget()`)
— local only, no deploy. Results as of 2026-07-14 (native-Rust test-host
execution; real WASM costs run somewhat higher — see the caveat in
`src/test/profiling_tests.rs`):

| Entrypoint | CPU instructions | Memory (bytes) |
|---|---|---|
| `stake` | 465,824 | 160,601 |
| `withdraw` | 494,378 | 163,572 |
| `submit_claim` (immediate activation) | 476,538 | 151,656 |
| `approve_override` (execution branch) | 396,624 | 130,903 |
| `claim_stream` (partial vesting, hits the daily-outflow-cap path) | 589,472 | 216,761 |

All comfortably under 1% of Soroban's typical ~100M-instruction
per-transaction CPU budget — no efficiency concerns found at this
profiling depth. `claim_stream` is the most expensive entrypoint, which
tracks with it doing the most computation (vesting math + daily-outflow-
cap math + a token transfer) in one call.

### Fuzzing

```bash
cd contracts/protection-pool
cargo +nightly fuzz run fuzz_solvency -- -max_total_time=60
cargo +nightly fuzz run fuzz_override -- -max_total_time=60
```

Two targets — `fuzz_solvency` (the core invariant) and `fuzz_override`
(the 2-of-2 admin+coSigner escape hatch). Requires nightly Rust
(`rustup toolchain install nightly`) and `cargo-fuzz`
(`cargo install cargo-fuzz`). **On macOS (Apple Silicon), this currently
crashes at libFuzzer's own startup** (`flockfile`/`vfprintf`, before any
fuzz iteration runs) — a host ASan/libc incompatibility confirmed
unrelated to this contract, reproduced independently on 2026-07-14 and
again on 2026-07-22. Two working options: `Dockerfile.fuzz`
(`docker build -f Dockerfile.fuzz -t safu-fuzz . && docker run --rm
safu-fuzz`), or run natively on any Linux host (no Docker needed there —
the incompatibility is macOS-specific; this is how the 2026-07-22 re-run
against the updated mechanism was done, via the team's Linux test VPS).
Combined results across all environments and both targets: **133,672
runs, zero crashes, zero solvency-invariant violations.** Full
breakdown: `TESTING.md` §4.

### Deploy-time arguments

`initialize(admin, oracle, co_signer, xlm_token, pool_cap)` — `pool_cap`
is a plain argument, never hardcoded into contract logic, and stays
admin-adjustable afterward via `set_pool_cap` (mirrors V8's mutable
`maxPoolSize`). The intended Tranche 1 deploy value: **600,000 XLM**
(approximating V8's 60 ETH cap) = `6_000_000_000_000` stroops
(1 XLM = 10,000,000 stroops).

## Contract layout

| File | Role |
|---|---|
| `src/lib.rs` | Public entrypoints (`#[contractimpl]`) — thin wrappers over the modules below, plus read-only view functions |
| `src/types.rs` | Data model (`StakeRecord`, `Claim`, `OverrideRequest`, `ClaimStatus`) and all tuning constants |
| `src/storage.rs` | Storage key layout + TTL-bump helpers (see below) |
| `src/admin.rs` | Init, oracle/coSigner/admin rotation, pause, suspend |
| `src/stake.rs` | Stake, withdraw, `setBeneficiary`, `emergencyExit`, points computation |
| `src/claim.rs` | Full claim lifecycle (submit → activate → stream → complete/cancel) + the 2-of-2 admin+coSigner override escape hatch |
| `src/test/` | Unit tests, split by mechanic (`admin_tests`, `stake_tests`, `claim_tests`, `override_tests`, `solvency_tests`) |
| `fuzz/` | `cargo-fuzz` targets for the solvency invariant and claim state machine (Soroban has no Halmos-equivalent symbolic verifier — this is the compensating control) |

## Storage model

Soroban gives three storage tiers with different TTL/cost semantics. This
contract's placement, and why:

| Tier | Used for | Rationale |
|---|---|---|
| `instance()` | Pool-wide globals: `admin`, `oracle`, `co_signer`, `xlm_token`, `pool_cap`, `total_staked`, `total_allocated`, `total_stakers`, `paused`, the daily outflow-cap counters (`daily_outflow`/`last_outflow_day`), the daily admission-cap counters (`daily_entitlement_total`/`last_entitlement_day`/`daily_claim_count`) | Every call already loads all of `instance()` — never put per-user or unbounded data here. All of these are fixed-size scalars. |
| `persistent()` | Per-staker (`Stake(Address)`), per-claim (`ClaimRec(BytesN<32>)`), per-override-request (`Override(BytesN<32>)`), banked points (`PointsBalance(Address)`) | Distributed across separate keys, not grown as one struct — bounded per-entity storage, archived+restorable via TTL bumps. |
| `temporary()` | Not currently used | Reserved for anything that's naturally allowed to expire and isn't load-bearing for solvency (e.g. a future price cache). |

**On struct field sizing:** reviewed 2026-07-14 — Solidity-style slot
packing (reordering fields to share a 32-byte EVM word) has no real
equivalent here. Soroban's storage cost is driven by read/write *count*,
TTL-extension frequency, and total serialized entry size, not sub-word
bit-packing, and every field in `StakeRecord`/`Claim`/`OverrideRequest`
is already at its minimum meaningful width: `i128` for token amounts
(stroops can exceed `u64` for a large pool), `u32` for ledger sequences
and `u64` for Unix timestamps (Soroban's own native return types —
narrowing either would just add conversion code for no storage saving).
`tier: u32` could theoretically be a byte, but Soroban has no native
`U8` `Val` type to switch to without wrapping overhead that would cost
more than the few bytes it'd save. Nothing changed here as a result —
documented so this doesn't get re-asked next audit pass.

**TTL is never a security mechanism.** The 90-day claim-eligibility gate, the
7-day cooldown, the 45-day vesting window, the 30-day claim-submission
window, and the 365-day false-positive penalty lock are all enforced by
comparing an explicit stored ledger-sequence (or Unix-timestamp) deadline in
contract logic — never by relying on a storage entry's TTL/archival state.
Anyone can extend anyone's TTL via `ExtendFootprintTTLOp` with no contract
auth required, so TTL expiry answers "is this data still cheaply readable,"
never "is this deadline still active."

Full `DataKey` enum (`src/storage.rs`):

```rust
pub enum DataKey {
    // instance() — pool-wide globals
    Admin, Oracle, CoSigner, XlmToken, PoolCap,
    TotalStaked, TotalAllocated, TotalStakers, Paused,
    DailyOutflow, LastOutflowDay,
    DailyEntitlementTotal, LastEntitlementDay, DailyClaimCount,
    // persistent() — per-entity
    Stake(Address), ClaimRec(BytesN<32>), Override(BytesN<32>),
    PointsBalance(Address),
}
```

## Mechanics — what's ported from V8, deliberately

- **Tier assessed off-chain by the oracle at claim time** (not stake-amount
  banded) — coverage cap = `stake × tier_ratio × TIER_COVERAGE_BPS / 10_000`,
  ratio and coverage-percentage kept as two independently-adjustable knobs.
- **90-day time gate**, points banked in full and never burned once earned.
- **Dynamic outflow cap** on payout streaming (5%/3%/1% by pool
  utilization), first-come-first-served per calendar day with automatic
  carry-forward — not a queue.
- **Separate admission-side stress cap** (25%/10%/3% by utilization) plus an
  oracle-only per-day claim-count rate limit.
- **On-chain solvency invariant**, checked at every `submit_claim` and every
  override execution: `total_allocated + entitlement ≤ total_staked`.
- **Forfeiture is immediate and permanent** on claim activation — the
  365-day re-stake lock is a narrower, separate penalty that only applies
  when `cancel_claim` reverses a false positive, never to a genuine paid
  claim.
- **`StakeRecord.amount` is never zeroed by claim-triggered forfeiture** —
  only `withdrawn = true` is set. Only the *voluntary* `withdraw()` path
  zeroes `amount`. (Verified against the live V8 source directly — an
  earlier draft of this port got this backwards, which cascaded into two
  further bugs before being caught and fixed; see git history.)

## Deliberate deviations from V8 (architecture, not gaps)

- **Auth:** Soroban's native `require_auth()` on the exact function+argument
  tuple, instead of V8's manual ECDSA-signature-verification-in-contract
  code. The oracle's authorization *is* the call itself — there's no
  separate signed byte-blob to design or verify.
- **Beneficiary hash:** `sha256`, not `keccak256` — Soroban-native, no
  cross-chain hash-matching requirement exists for this field.
- **Stake bounds:** dynamic basis-points of the (admin-adjustable) pool cap,
  not V8's fixed ETH amounts — reproduces V8's real 1.25%-of-pool ratio
  exactly, portable across future chain deploys with different pool sizes.
- **No yield layer, no failed-payout-rescue bucket, no `revokedApprovals`
  list** — Tranche 1 scope excludes yield entirely (native XLM has no
  Lido-wstETH-style wrap/unwrap and no trustline-failure mode the way
  arbitrary EVM sends do; Soroban's native auth already handles
  replay/nonces without a custom revocation list).

## Known open items before mainnet

- **Outflow cap deviates from the SCF #44 submitted grant text**, which
  commits Tranche 1 to a flat "2%/day" payout cap. What's built here is
  V8's real dynamic 5%/3%/1% cap — a deliberate choice ("copy V8 wholesale"
  read as meaning the actual mechanic, not the grant text's simplified
  description). Not yet raised with the SCF team.
- **No Halmos-equivalent exists for Soroban.** The solvency invariant and
  claim state machine rest on test coverage (178 unit tests, 97.47% line
  coverage, 100% of catchable mutants killed — confirmed 2026-07-22
  against the current code) and 133,672 fuzz runs
  across two targets and multiple environments, rather than symbolic
  proof — see `TESTING.md` for the full methodology. Certora Sunbeam (the
  real ecosystem tool) is deliberately deferred to Tranche 3's SCF-funded
  audit.
- **`cancel_pending_override`'s mechanics were re-verified against V8
  directly** (was previously an unconfirmed guess) — see `src/claim.rs`
  module doc comment for the full account of what was wrong and what was
  fixed.

## Blend/YieldBlox illustrative scenario (SCF #44 reviewer response)

Addresses the SCF #44 reviewer comment asking for "a Blend exploit
analysis with simulations showing how the protocol would have helped in
a real incident."

**Disclosure — read before anything else:**
- This is a simulation against a real historical incident, **not** an
  existing or pending relationship with Blend/YieldBlox. SAFU has no live
  protocol-level pool product, and Blend/YieldBlox is not a SAFU
  depositor or partner.
- No scanner detection logic is used, implied, or reproduced anywhere
  here. Whether a transaction is "drain-shaped" is asserted as a labeled
  fixture input in the tests below — never a computed scanner verdict.
  SAFU's actual scanner (what decides whether a real transaction fires)
  is proprietary and lives entirely off-chain, in a separate private repo.
- Only the public entitlement formula (`entitlement = min(stake ×
  tier_ratio, loss)` — 15x/10x/5x by tier, already SAFU's own published
  protocol mechanic) is exercised, run through the real, audited on-chain
  `submit_claim` → `approve_claim` → `claim_stream` entrypoints — not a
  re-derivation or approximation.
- This section demonstrates the payout mechanism only. It is not this
  repo's answer to whether SAFU is fundamentally a staking product or a
  Stellar integration (a separate reviewer comment); that question is
  addressed in the section above.

**Incident:** Blend/YieldBlox, Stellar, oracle manipulation, real tx
`3e81a3f7b6e17cc22d0a1f33e9dcf90e5664b125b9e61f108b8d2f082f2d4657`
(independently verified against Horizon 2026-07-22) — ~$10.8M loss,
publicly reported, not proprietary.

**What the tests demonstrate** (`src/test/blend_scenario_tests.rs`,
run with `cargo test --package protection-pool blend_scenario --
--nocapture`): a depositor at the contract's own real `MAX_STAKE` bound
(1.25% of pool cap — "$1M" in this scenario's illustrative $-mapping,
pool cap = "$100M") submits a claim carrying Blend/YieldBlox's real tx
hash. Tier only changes the entitlement passed in — the real on-chain
`tier_cap` check is what actually accepts or would reject it, not a mock:

| Tier | Ratio | Coverage cap (deposit × ratio) | Entitlement (capped at $10.8M loss) | % of real loss covered |
|------|-------|------------------------------|--------------------------------------|------------------------|
| A | 15x | $15.0M | $10.8M | 100% |
| B | 10x | $10.0M | $10.0M | 92.6% |
| C | 5x | $5.0M | $5.0M | 46.3% |

A fourth test, `ordinary_transaction_never_becomes_a_claim`, is the
negative control: an ordinary transaction on the same kind of fixture
pool/participant never becomes a claim at all — no `submit_claim` call,
the deposit stays fully claim-eligible, nothing is ever earmarked for
payout. The point is discernment, not "everything pays out."

## Full technical reference

The complete V8→Soroban mechanics map, SDK reference, vulnerability
checklist, and audit history for this contract live in the SAFU team's
internal ops repo: `context/knowledge/smartcontract-soroban.md`. This
README is the standalone summary for anyone reading this repo on its own.
