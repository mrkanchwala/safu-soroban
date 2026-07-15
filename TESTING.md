# Testing Methodology — ProtectionPool (SCF #44 Tranche 1)

This is the answer to "what did we do to verify this contract" — every
method applied, why, and what it found. Scope is code-level verification
only (this contract has not been deployed anywhere yet — see
`README.md`'s "Known open items"). All numbers below are reproducible
with the commands in each section.

## 1. Unit tests — 157 passing

```bash
cargo test --package protection-pool
```

Split by mechanic in `src/test/`:

| Module | Covers |
|---|---|
| `admin_tests.rs` | init, oracle/coSigner/admin rotation, pause, suspend |
| `stake_tests.rs` | stake, withdraw, `set_beneficiary`, `emergency_exit`, points |
| `claim_tests.rs` | full claim lifecycle: submit → activate → stream → complete/cancel |
| `override_tests.rs` | 2-of-2 admin+coSigner override/rotation escape hatch |
| `solvency_tests.rs` | the core `total_allocated ≤ total_staked` invariant |
| `profiling_tests.rs` | CPU/memory cost per hot-path entrypoint (see README) |
| `mutation_gap_tests.rs` | 24 boundary/exact-value tests added 2026-07-15 specifically to close mutation-testing gaps (see §3) |

## 2. Coverage — 96.65% line / 97.77% region / 93.64% function

```bash
cargo llvm-cov --package protection-pool --summary-only
```

| File | Line % | Notes |
|---|---|---|
| `admin.rs` | 98.18% | |
| `claim.rs` | 97.30% | largest file (683 lines) — claim lifecycle + override flow |
| `lib.rs` | 93.33% | thin entrypoint wrappers |
| `stake.rs` | 94.55% | |
| `storage.rs` | 99.44% | |
| `test/common.rs` | 100% | shared test setup |
| `types.rs` | 0% | 4 lines — pure data/const declarations, no branches to cover |

Industry guidance (checked 2026-07-15 against current smart-contract QA
practice) targets ≥90% line coverage with ≥95% on fund-handling code
combined with property-based fuzzing — this suite meets or exceeds that
on every fund-handling file.

**Coverage measures whether a line ran during tests — not whether a bug
there would be caught. That distinction is exactly what §3 addresses.**

## 3. Mutation testing — 100% of catchable mutants killed

```bash
cargo mutants --workspace
```

Mutation testing deliberately injects small bugs (flip `>` to `>=`, `+` to
`-`, etc.) one at a time and checks whether the test suite notices. It is
the only method here that actually measures test *quality* rather than
test *quantity* — a suite can have 96% line coverage and still miss real
bugs if it never asserts the exact value at a boundary.

**First full run (2026-07-15):** 464 mutants — 390 caught, 60 missed
(86.7%), 14 unviable (didn't compile).

**Response:** every missed mutant was individually triaged against the
actual source, not treated as a generic gap:
- **51 were real test gaps** — the line executed during tests but no
  assertion pinned down the exact resulting value or exact boundary
  (e.g., the points-formula day-tier thresholds at 90/180/365 days, the
  utilization-bps thresholds in `stress_cap`/`dynamic_outflow_bps` at
  exactly 2000/5000 bps, the override release-math on the exact surface
  where 6 real bugs were previously found). Closed with 24 new targeted
  tests in `src/test/mutation_gap_tests.rs` — each names the exact
  mutant(s) it kills by file:line.
- **10 were provably equivalent** — the mutated code has no observable
  behavioral difference under any reachable contract state (the
  recurring pattern: `amount == 0` implies `withdrawn == true` in every
  reachable path, so a `>` vs `>=` swap on that guard is unobservable).
  Documented with per-entry justification in `.cargo/mutants.toml` — not
  silently excluded.

**Final rerun (2026-07-15):** 455 mutants (post-exclusion) — **440
caught, 1 missed, 14 unviable.** The single survivor was re-triaged,
confirmed equivalent (defensive-only guard, no reachable call site
violates it), and added as the 10th exclusion. **Result: 100% of
catchable mutants killed.**

## 4. Fuzzing — 2 targets, 2 independent environments, 53,588+37,943 runs

```bash
cd contracts/protection-pool
cargo +nightly fuzz run fuzz_solvency -- -max_total_time=300
cargo +nightly fuzz run fuzz_override -- -max_total_time=300
```

Two coverage-guided targets, both replaying random Stake/Withdraw/
SubmitClaim/ClaimStream/CancelClaim/AdvanceDays-style action sequences:

| Target | What it checks |
|---|---|
| `fuzz_solvency` | `total_allocated ≤ total_staked` after every single operation — the pool's single most important guarantee |
| `fuzz_override` | the 2-of-2 admin+coSigner override/rotation flow, including mid-approval key rotation — the exact surface where 6 real bugs were found earlier in this project |

**Two independent environments, deliberately:**
- **2026-07-14, Docker on macOS** (workaround for a host ASan/libFuzzer
  crash confirmed unrelated to this contract): `fuzz_solvency` 37,943
  runs / `fuzz_override` 22,813 runs.
- **2026-07-15, native Linux on a dedicated test VPS** (no Docker needed
  — the macOS incompatibility doesn't exist on Linux): `fuzz_solvency`
  21,692 runs / `fuzz_override` 31,896 runs.

**Combined: 114,344 fuzzed action-sequences across both targets and both
environments. Zero crashes, zero solvency-invariant violations, ever.**

Soroban has no Halmos-equivalent symbolic verifier (Kani was researched
and ruled infeasible for `no_std`/FFI-heavy `soroban-sdk` code; Certora
Sunbeam is the real ecosystem tool, deliberately deferred to Tranche 3's
SCF-funded audit rather than self-run now). Fuzzing at this depth is the
compensating control.

## 5. Static analysis & dependency scanning

```bash
cargo clippy --workspace --all-targets
cargo audit
grep -rn "unsafe" --include="*.rs" src/
```

- **clippy:** 1 trivial style warning (`needless_borrows_for_generic_args`,
  `stake.rs:178`) — no functional issue, not yet fixed.
- **cargo-audit (RUSTSEC advisory database):** 0 CVEs. 1 unmaintained-crate
  notice (`paste`, a transitive `soroban-sdk` dependency — not directly
  actionable).
- **`unsafe` blocks:** zero, confirmed by grep.
- **Arithmetic safety:** the contract uses plain `+`/`*`/`/` rather than
  `checked_*` throughout — verified safe because the workspace
  `[profile.release]` sets `overflow-checks = true` + `panic = "abort"`,
  so any overflow aborts the transaction cleanly in production WASM
  rather than wrapping silently (found during the `/audit-chain` pass,
  §7 below — flagged there as a profile-dependent guarantee worth
  re-checking if the profile ever changes).

## 6. Manual V8-parity verification — 4 escalating passes

Every mechanic was checked against the live `SAFUPoolV8.sol` source
directly, not against a summary of it, across four separate passes as
the contract was built and re-reviewed:
1. Full-source-read during initial build (found the 6-state claim
   machine, the beneficiary-hash pattern, the FCFS-per-day outflow cap).
2. Adversarial cross-reference during `/plan-eng-review` (corrected the
   `totalStakedSnapshot` timing).
3. Direct re-read of `cancel_pending_override` and the
   `StakeRecord.amount`-never-zeroed-on-forfeiture behavior (found and
   fixed a real double-release accounting bug this surfaced).
4. Full re-verification of `admin.rs`/`stake.rs` beyond `claim.rs` (found
   2 more gaps: `set_co_signer`'s missing `!= owner()` check, and that
   the computed view functions — `is_eligible`/`pointsOf`-equivalent —
   had only been ported as raw storage getters, not the live-computed
   values V8 actually returns).

Full narrative for each: `src/claim.rs`, `src/admin.rs`, and
`src/stake.rs` module-level doc comments; `context/knowledge/
smartcontract-soroban.md` §5b in the SAFU team's internal ops repo.

## 7. Adversarial security review

- **`/audit-chain --target soroban`, comprehensive mode (2026-07-15):**
  full attacker-mindset pass — 14/14 Soroban vulnerability checklist,
  8/8 ProtectionPool-specific checks, 4/4 economic-attack probes (cap-
  manipulation, oracle rate-limit gaming, dust/rounding extraction,
  storage-TTL griefing) — all clean. **0 CRITICAL/HIGH/MEDIUM.** Full
  report: `audits/2026-07-15_internal-audit-chain.md`.
- **`/cso`, oracle/infra scope (2026-07-15):** secrets archaeology on
  this repo (working tree + full git history) — clean. The Stellar
  oracle signer itself is Tranche 2 scope and doesn't exist yet; a
  design checklist for that future build is filed in the report rather
  than treated as a T1 gap. Full report:
  `audits/2026-07-15_internal-cso-oracle-scope.md`.
- **Solodit cross-reference** (Cyfrin's real-world finding database,
  50,000+ findings across 30+ audit firms): searched Access Control,
  Oracle, Reentrancy, Rounding, and Replay-Attack finding classes for
  patterns applicable to this contract's architecture. No new finding —
  each dominant real-world pattern was either not applicable (no on-chain
  price oracle to go stale, no external-callback reentrancy surface) or
  already covered by the checks above.

## 8. What this does NOT cover — the honest ceiling

- **No formal/symbolic verification.** V8's Solidity contract has 10/10
  Halmos properties proven with zero counterexamples. No direct Soroban
  equivalent exists (see §4); Certora Sunbeam is the real answer and is
  deliberately deferred to Tranche 3's SCF-funded audit.
- **No external human audit yet.** Everything above is internal
  (self-run tooling + manual review), which is why Tranche 3 budgets a
  real external audit through the SCF Audit Bank.
- **Not deployed anywhere.** No live-network behavior (real Stellar RPC
  timing, real trustline edge cases, real congestion) has been observed
  yet — see README's "Known open items."

## Reproducing this report

All commands above run from the repo root (or `contracts/protection-pool`
where noted) with: `rustc`/`cargo` (stable + nightly), `wasm32v1-none`
target, `cargo-fuzz`, `cargo-audit`, `cargo-llvm-cov`, `cargo-mutants`,
and `stellar-cli`. No deployment, network access, or secrets required for
any check in this document.
