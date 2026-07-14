# SAFU ProtectionPool — Soroban

Soroban (Stellar smart contracts, Rust → WASM) port of `SAFUPoolV8.sol`, the
live Ethereum mainnet contract at
[`0xa170f0937DEc353C1806eaC0c3d559524d458641`](https://etherscan.io/address/0xa170f0937DEc353C1806eaC0c3d559524d458641).
Built for SCF #44 Tranche 1 — the core pool/points/tier/claim/payout-stream
mechanics, ported wholesale from V8. Yield deployment (Lido wstETH on V8,
DeFindex/Blend planned for Tranche 2) is deliberately excluded from this
scope.

**Status:** core mechanics complete and tested (121 unit tests passing),
compiles to WASM (`cargo build --release --target wasm32v1-none`),
**not yet deployed anywhere** — deployment is gated on a final audit
sign-off and resolving one open item with the SCF team (see below).

## Building

```bash
cargo check --package protection-pool          # fast type/logic check
cargo test --package protection-pool           # 121 unit tests
cargo build --package protection-pool --release --target wasm32v1-none
stellar contract build --optimize              # or: stellar contract optimize --wasm <path>
```

Requires: `rustc`/`cargo`, the `wasm32v1-none` target
(`rustup target add wasm32v1-none`), and `stellar-cli`
(`brew install stellar-cli`, or see
[developers.stellar.org](https://developers.stellar.org)).

### Fuzzing

```bash
cd contracts/protection-pool
cargo +nightly fuzz run fuzz_solvency -- -max_total_time=60
```

Requires nightly Rust (`rustup toolchain install nightly`) and
`cargo-fuzz` (`cargo install cargo-fuzz`). **On macOS (Apple Silicon),
this currently crashes at libFuzzer's own startup** (`flockfile`/
`vfprintf`, before any fuzz iteration runs) — a host ASan/libc
incompatibility confirmed unrelated to this contract. Use
`Dockerfile.fuzz` instead: `docker build -f Dockerfile.fuzz -t
safu-fuzz . && docker run --rm safu-fuzz`. Verified working in that
container 2026-07-14: 4270 runs in 61s, zero crashes, zero solvency-
invariant violations.

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
  claim state machine rest on test coverage (121 unit tests) and the
  `fuzz/` targets rather than symbolic proof.
- **`cancel_pending_override`'s mechanics were re-verified against V8
  directly** (was previously an unconfirmed guess) — see `src/claim.rs`
  module doc comment for the full account of what was wrong and what was
  fixed.

## Full technical reference

The complete V8→Soroban mechanics map, SDK reference, vulnerability
checklist, and audit history for this contract live in the SAFU team's
internal ops repo: `context/knowledge/smartcontract-soroban.md`. This
README is the standalone summary for anyone reading this repo on its own.
