# Testing Methodology — ProtectionPool (SCF #44 Tranche 1)

This is the answer to "what did we do to verify this contract" — every
method applied, why, and what it found. Scope is code-level verification
only (this contract has not been deployed anywhere yet — see
`README.md`'s "Known open items"). All numbers below are reproducible
with the commands in each section.

**Updated 2026-08-17 (Tranche 2 + 7a audit)** — this file previously described
Tranche 1 only. T2 added D1 (on-chain Ed25519 oracle approval verification),
D2 (DeFindex vault yield deployment, `src/vault.rs`), atomic oracle rotation,
and the 7a audit fixes. Concretely, since the numbers below were last measured:
- **Unit tests: 250 passing** (was 184 at T1, 238 after the T2 merge) — §1's count is updated below
- **Error variants: 73** (was 72) — D1 appended 73-76, D2 appended 80-92, and
  the 7a audit appended `Paused = 93` when `require_not_paused` was converted
  from the last surviving bare `panic!`. Codes are public ABI and are never
  renumbered
- **`initialize` no longer exists** — configuration moved to `__constructor`
  (7a audit) to close the deploy→init front-running window. See README's
  "Deploy-time arguments"
- **Fuzzing (§4) HAS been re-run against T2 code — 2026-08-17.** Both targets,
  in Docker on the team's Linux host, against the exact commit deployed to
  testnet: `fuzz_solvency` 71,034 runs / `fuzz_override` 80,364 runs =
  **151,398 runs, zero crashes, zero artifacts.** Re-verified 2026-08-20
  against the then-current `main`: identical commit, and no changes to
  `contracts/protection-pool/src/` or `fuzz/` since, so the result still holds
  for the deployed code.
  *(This bullet previously asserted the opposite and cited "2026-07-14: 4,270
  runs". That figure is the container smoke test in `Dockerfile.fuzz`'s own
  header, not a campaign, and the section reference was wrong too — fuzzing is
  §4, not §3. Corrected 2026-08-21.)*

**Updated 2026-07-31** — converted the contract from `panic!("SAFU: ...")`
string-based error handling to a typed `#[contracterror] PoolError` enum
(`src/error.rs`, 72 variants at that time) + `Result<T, PoolError>` on every fallible
public/internal function. Pure error-handling-mechanism refactor: same
validation conditions, same order, same business logic, zero behavior
change beyond the failure signal — verified by a full line-by-line diff
review (zero logic drift found) plus a fresh mutation-testing re-run
(§3). Triggered by an SCF #44 reviewer's "not much experience with
Soroban" comment; this was the concrete gap found and closed. This
update re-measured §1 and §3 against the new code.

**Updated 2026-07-22** — added a points burn-on-claim mechanism (staker-
gated approval step + two 100-day expiry rules, full detail in `claim.rs`
module doc comments) and fixed 5 pre-existing accounting bugs found during
a full-contract review, independent of the new mechanism (see §7 for
what they were). This update re-measured every section below against the
new code rather than leaving the 2026-07-15 numbers in place unverified.

## 1. Unit tests — 250 passing (up from 238; 184 at Tranche 1)

```bash
cargo test --package protection-pool
```

Split by mechanic in `src/test/`:

| Module | Covers |
|---|---|
| `admin_tests.rs` | init, oracle/coSigner/admin rotation, pause, suspend (now including suspend/unsuspend on an already-approved claim) |
| `stake_tests.rs` | stake, withdraw, `set_beneficiary`, `emergency_exit`, points |
| `claim_tests.rs` | full claim lifecycle: submit → **AwaitingApproval → approve_claim (burn) →** stream → complete/cancel, plus the new `expire_pending_approval`/`expire_stale_claim` sweeps and their suspend-interaction edge cases |
| `override_tests.rs` | 2-of-2 admin+coSigner override/rotation escape hatch |
| `solvency_tests.rs` | the core `total_allocated ≤ total_staked` invariant |
| `profiling_tests.rs` | CPU/memory cost per hot-path entrypoint (see README) |
| `mutation_gap_tests.rs` | boundary/exact-value tests, including 2 new regression tests added 2026-07-22 proving the override-conflict guard and the daily-cap saturating-subtraction fix specifically (see §3, §7) |

16 pre-existing tests were updated for the new approval-gated flow (the gate no longer auto-activates a claim); none were weakened — 2 of the 16 had been silently pinning bugs as "expected" values, corrected with the bug fix rather than left inconsistent.

## 2. Coverage — 98.40% line / 98.11% region / 97.28% function (re-measured 2026-08-25, after the Tranche 2 mutation-gap tests)

```bash
cargo llvm-cov --package protection-pool --summary-only
```

| File | Line % | Notes |
|---|---|---|
| `admin.rs` | 96.95% | |
| `claim.rs` | 98.76% | largest file — claim lifecycle + override flow + approval/expiry + D1 oracle verification |
| `lib.rs` | 99.21% | thin entrypoint wrappers |
| `stake.rs` | 95.00% | |
| `storage.rs` | 99.60% | |
| `test/common.rs` | 100% | shared test setup |
| `types.rs` | 0% | pure data/const declarations, no branches to cover |
| `vault.rs` | 98.93% | D2 yield module, added in Tranche 2 |

Overall line coverage improved on the prior 97.47% figure and function coverage on the prior 95.69% — the 12 tests added to close the Tranche 2 mutation gaps (§3) landed real assertions on previously-thin branches, not just incidental coverage. `vault.rs`, where 19 of the 22 surviving mutants sat, is the second-best-covered non-trivial file in the crate.

Industry guidance (checked 2026-07-15 against current smart-contract QA
practice) targets ≥90% line coverage with ≥95% on fund-handling code
combined with property-based fuzzing — this suite meets or exceeds that
on every fund-handling file.

**Coverage measures whether a line ran during tests — not whether a bug
there would be caught. That distinction is exactly what §3 addresses.**

## 3. Mutation testing — five campaigns; 100% of catchable mutants killed on Tranche 1, 389/390 on the Tranche 2 diff

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

**Re-run against the 2026-07-22 changes, same day (second data point):**
485 mutants — 461 caught, 10 missed, 14 unviable (97.9%). All 10 misses
were in the new `approve_claim`/`expire_pending_approval`/
`expire_stale_claim` functions and the suspend/unsuspend fairness fixes —
nothing pre-existing regressed. Triaged individually, same method as the
2026-07-15 baseline:
- **9 were real test gaps** — closed with 5 new targeted tests across
  `admin_tests.rs`, `claim_tests.rs`, and `override_tests.rs`: a wallet-
  ownership-check inversion, both deleted Rule A/Rule B clock-reset match
  arms, the deadline-arithmetic and burn-sum-arithmetic mutants (the
  latter required reading the `ClaimApproved` event's `points_burned`
  field directly via XDR decode, since storage is unconditionally zeroed
  regardless of the sum's correctness — no test can observe it any other
  way), the approval-window exact-boundary case, and the override-release
  skip for `AwaitingApproval`/`PendingTime` claims.
- **1 was provably equivalent** (`get_points_balance`'s `&&`→`||`) — full
  reachable-state truth-table walk: the only state where the two
  conditions diverge (`amount>0`, `withdrawn=true`, reachable since this
  session's claim-forfeiture path) still forces the identical return
  value (0) via `compute_points_for_record`'s own independent `withdrawn`
  guard plus the burn's unconditional zero-out. Added as the 11th
  `.cargo/mutants.toml` exclusion.

**Final rerun (2026-07-22):** 484 mutants (post-exclusion) — **470
caught, 0 missed, 14 unviable. Result: 100% of catchable mutants killed,
confirmed against the current code, not carried forward stale.**

**Re-run against the 2026-07-31 typed-errors-conversion changes:** 485
mutants (`cargo mutants --no-shuffle --jobs 4`, 89 minutes wall-clock) —
**470 caught, 14 unviable, 1 missed.** The single survivor
(`stake.rs:137:28: replace > with >= in stake`, the re-stake guard
`existing.amount > 0 && !existing.withdrawn`) was not a new gap — it is
the exact same equivalent mutant excluded above (10th exclusion), which
had been anchored to line 126 pre-conversion; the refactor's new imports/
doc comments/signature changes shifted the line to 137, so the old
line-specific regex stopped matching it. Re-triaged from scratch (not
assumed equivalent just because it matched a known pattern): confirmed
by the identical reasoning as the original exclusion — `amount == 0`
implies `withdrawn == true` in every reachable state, so the `!withdrawn`
conjunct already excludes the boundary the `>=` mutant would newly admit.
`.cargo/mutants.toml`'s line anchor updated to 137 (commit `562ddc7`) —
not re-run after the fix, since the fix is a config correction to an
already-proven-equivalent mutant, not a code change requiring
re-verification. **Result: every one of the 485 mutants that could
possibly be caught, was — no regression from the error-handling
refactor.**

**2026-08-25, fifth campaign — the Tranche 2 diff, and the first mutation
run scoped to it.** The four campaigns above were all measured against
Tranche 1 code. This one runs `cargo mutants --in-diff` over the contract
diff between `pre-typed-errors-2026-07-31` and the Tranche 2 submission
tag, so it tests exactly what Tranche 2 added or changed: **400 mutants —
389 caught, 10 unviable, 1 survivor.**

The initial pass returned 22 survivors, **19 of them in `src/vault.rs`**,
the DeFindex yield module Tranche 2 introduced, concentrated on the
capital-movement path (`redeem`, `withdraw_yield`, `extract_yield`,
`deploy_to_vault`, `set_deploy_bps`, `yield_balance`) plus both approval-
window boundaries in `revoke_approval`. Triaged individually, same method
as the 2026-07-15 baseline:

- **17 were real test gaps** — the line executed under test but no
  assertion pinned the exact value or boundary. Closed with **12 targeted
  tests** in `src/test/t2_mutation_gap_tests.rs`; each names the mutant(s)
  it kills by `file:line:col`, and each was verified by hand-injecting the
  mutation into the real source and confirming the test fails, then
  restoring and checksumming. Suite 238 -> 250.
- **3 were provably equivalent** — `yield_balance`'s and `extract_yield`'s
  zero-width `>`/`>=` boundaries, where both arms compute 0, and
  `extract_yield`'s `current + 0` write, unobservable because
  `get_total_extracted_yield` ends in `.unwrap_or(0)`. Documented with
  per-entry justification in `.cargo/mutants.toml`, not silently excluded.
  Two are line-anchored deliberately: `extract_yield` has three `>`/`>=`
  sites producing identical mutant descriptions and the third is a REAL
  gap, so a function-scoped pattern would have swallowed it.
- **1 was a pre-existing documented equivalent** (`stake.rs:137`), already
  excluded and surfaced only because this pass ran with exclusions off.

**The scope is 400 rather than 404** because the shipped
`.cargo/mutants.toml` excludes those four; 400 is what `cargo mutants`
reports from this repository as-is.

**The one survivor: `vault.rs:375`, replacing the body of
`authorize_withdraw` with `()`.** That function's job is to request
authorization from the vault before withdrawing. The test environment
approves all authorization automatically, so it cannot observe whether the
request was made — deleting the function leaves all 250 tests passing. It
is **not** an equivalent mutant. Four approaches were tried to close it,
including scoped `mock_auths`, asserting on `env.auths()`, and hand-built
`SorobanAuthorizationEntry` values under both credential types; all fail
for the same structural reason, since disabling the mocking also disables
the admin's own authorization and the test then never reaches the vault
call. The function's behaviour is verified against the deployed DeFindex
vault directly — its authorization shape was captured from a live testnet
`withdraw` on 2026-08-14 (see §6).

**No contract source file was modified by any of this.** The Tranche 2
WASM rebuilt from this tree hashes to
`62ca8a24acf4fdb262ae479587924fb36bf5604421b895a0b8b7accfb5eaed3a` —
byte-identical to the deployed contract.

## 4. Fuzzing — 2 targets, 4 campaigns, 285,070 runs, zero crashes ever

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

**Subtotal for the two July campaigns: 114,344 fuzzed action-sequences
across both targets and both environments. Zero crashes, zero
solvency-invariant violations.**

**2026-07-22, third data point — same macOS ASan/libFuzzer host
incompatibility recurred** (confirmed independently again: both targets
crashed identically inside libFuzzer's own internal print routine before
exercising any contract code, not a target-code issue). Worked around by
re-running natively on the Linux VPS test rig against the updated
(mechanism + bug-fix) code: `fuzz_solvency` 8,237 runs / `fuzz_override`
11,091 runs, 90s each. **19,328 additional runs, zero crashes,
zero solvency-invariant violations** — the new approval/expiry logic and
all 5 bug fixes held up clean under fuzzing, not just the unit-test suite.

**2026-08-17, fourth campaign — the current Tranche 2 result, and the one
the Tranche 2 submission quotes.** Run in Docker on the team's Linux host
against the exact commit deployed to testnet (`16b4d118`), both targets at
601s each: `fuzz_solvency` **71,034 runs** / `fuzz_override` **80,364
runs** = **151,398 fuzzed action-sequences, zero crashes, zero artifacts,
zero solvency-invariant violations.** Re-verified 2026-08-20 against the
then-current `main`: identical commit, with no changes to
`contracts/protection-pool/src/` or `fuzz/` since, so this result stands
for the deployed contract rather than for a superseded build.

**All four campaigns combined: 285,070 fuzzed action-sequences. Zero
crashes, zero solvency-invariant violations, ever.** When a single figure
is quoted for the current code, it is the 151,398 above, because the three
July campaigns predate the D1/D2/7a changes and the `__constructor`
migration.

Soroban has no Halmos-equivalent symbolic verifier (Kani was researched
and ruled infeasible for `no_std`/FFI-heavy `soroban-sdk` code; Certora
Sunbeam is the real Soroban tool, deliberately deferred to Tranche 3's
SCF-funded audit rather than self-run now). Fuzzing at this depth is the
compensating control.

## 5. Static analysis & dependency scanning

```bash
cargo clippy --workspace --all-targets
cargo audit
grep -rn "unsafe" --include="*.rs" src/
```

- **clippy:** 0 warnings (re-checked 2026-07-31 against the typed-errors-
  conversion code — unchanged from 2026-07-22; 3 unrelated pre-existing
  doc-comment style warnings in a test file, not from this change).
- **cargo-audit (RUSTSEC advisory database):** 0 CVEs (re-checked
  2026-07-31 on the VPS, unchanged). 1 unmaintained-crate notice
  (`paste`, a transitive `soroban-sdk` dependency — not directly
  actionable).
- **`unsafe` blocks:** zero, confirmed by grep.
- **Arithmetic safety:** the contract uses plain `+`/`*`/`/` rather than
  `checked_*` throughout — verified safe because the workspace
  `[profile.release]` sets `overflow-checks = true` + `panic = "abort"`,
  so any overflow aborts the transaction cleanly in production WASM
  rather than wrapping silently (found during the `/audit-chain` pass,
  §7 below — flagged there as a profile-dependent guarantee worth
  re-checking if the profile ever changes).

## 5b. Symbolic verification — Komet (K framework)

Komet (runtimeverification/komet) is the Soroban counterpart to Halmos, and
appears on the SCF Audit Bank intake form's accepted-tooling list. It has two
modes: `komet test` fuzzes properties over many random inputs, and
`komet prove run` symbolically executes them, establishing a property for ALL
inputs rather than sampled ones.

**What it verifies.** Properties are written as a *second contract* that calls
the one under test — `contracts/test-protection-pool`, with `kasmer.json`
naming the contracts to compile and deploy. The property that matters is
`test_solvency_invariant` (`total_allocated <= total_staked`). The staker is
funded with 100,000 XLM-equivalent so a property can never pass merely because
every transfer reverted.

**Why the harness is a separate crate.** Making the pool directly
Komet-deployable by gating `__constructor` behind a cargo feature was built,
measured, and reverted: the optimised Wasm hash moved
(`2cec7e74…` -> `024b4078…`) **with the feature disabled**, because
`#[contractimpl]` emits a different contract spec. The harness therefore lives
in its own crate that `#[path]`-includes the real modules, and all three
harness crates are excluded from the workspace so they cannot reach the
production build.

### Running it

Komet is **not self-contained**: `komet/kasmer.py:156` shells out to
`stellar contract build`, so the environment needs `stellar` on PATH plus
`cargo` and the `wasm32v1-none` target (that command compiles Rust).

Two supported paths:

```
# 1. GitHub Actions — manual dispatch, produces the citable public artifact
gh workflow run komet.yml --ref main

# 2. Container — x86_64 only (the K toolchain ships x86_64)
docker build -f Dockerfile.komet -t safu-komet .
docker run --rm safu-komet bash -lc \
  '. ~/.nix-profile/etc/profile.d/nix.sh; komet test -C contracts/test-protection-pool'
docker run --rm safu-komet bash -lc \
  '. ~/.nix-profile/etc/profile.d/nix.sh; komet prove run -C contracts/test-protection-pool'
```

The bare forms (`komet test`, `komet prove`) are not valid — `-C <dir>` is
required, and `prove` takes the `run` subcommand. `komet` has no `--version`;
it exits 2 without a subcommand, so use `--help`.

The container image verifies its own toolchain at build time and prints
`TOOLCHAIN COMPLETE`, so a broken image cannot silently reach the test step.

### Reading the result — a green tick is not a result

**Judge Komet by its exit code, never by grepping its log.** Both
`komet test` and `komet prove run` must exit **0 with non-empty output**.

This is not theoretical. CI run 33481585123 reported *"Verdict: properties
checked — results above are real"* while both invocations had crashed in under
a second with `RuntimeError: Couldn't find 'stellar' executable` — the log grep
looked for `^error`, which `RuntimeError:` does not match. An absent finding is
not a clean finding and must never be cited as one.

### Status

**No Komet result is claimed in this document yet.** The environment is proven
(toolchain verified, both harness contracts build) but the property tests had
not produced a completed result at the time of writing. This section will carry
the numbers once they exist; until then Komet is listed as tooling in use, not
as evidence.

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

**The 2026-07-22 points burn-on-claim mechanism has no V8 equivalent to
verify against — it's net-new Soroban logic, not a port.** V8's points
were passive (accrued, never consumed); this build adds a staker-gated
approval step that burns the wallet's full points balance, plus two
100-day expiry rules with no analogue anywhere in the reference contract.
Verification for this piece rests on §1 (unit tests), §4 (fuzzing), and
§7 (adversarial review) instead of source-parity checking.

## 7. Adversarial security review

- **`/audit-chain --target soroban`, comprehensive mode (2026-07-15):**
  full attacker-mindset pass — 14/14 Soroban vulnerability checklist,
  8/8 ProtectionPool-specific checks, 4/4 economic-attack probes (cap-
  manipulation, oracle rate-limit gaming, dust/rounding extraction,
  storage-TTL griefing) — all clean. **0 CRITICAL/HIGH/MEDIUM.**
- **`/audit-chain --target soroban`, comprehensive mode, re-run 2026-07-22
  on the burn-mechanism + bug-fix changes** — explicitly run in
  bug-bounty style (actively trying to break the new logic, not just
  confirming the checklist). Found and fixed **3 real issues within the
  same pass**, all in the suspend/unsuspend fairness logic added this
  session (the core burn/expiry mechanism and the 5 bug fixes held up
  clean under this adversarial re-examination):
  1. The two new expiry sweeps (`expire_pending_approval`/
     `expire_stale_claim`) could still fire while a staker was actively
     suspended — the original fix only reset the deadline on unsuspend,
     never blocked the sweep from running during an ongoing suspension.
  2. The suspend-freezes-an-active-claim upgrade was unreachable in
     practice — a separate, pre-existing guard in `suspend_stake` blocked
     admin from ever suspending an already-forfeited stake, exactly the
     case the upgrade needed to reach. Found by tracing a real test
     failure back to its actual cause, not by inspection alone.
  3. `unsuspend_stake`'s deadline-reset didn't verify the passed claim
     belonged to the wallet being unsuspended (admin-only surface, low
     severity, fixed anyway).

  All 3 fixed, 3 new regression tests added (one proves the suspend fix's
  positive path genuinely works, not just that the bug panics correctly).
  **14/14 + 8/8 checklists PASS, 0 CRITICAL/HIGH.** Full report:
  `outputs/2026-07-22_audit-chain-soroban-protection-pool.md` in the SAFU
  team's internal ops repo.
- **`/cso`, oracle/infra scope (2026-07-15):** secrets archaeology on
  this repo (working tree + full git history) — clean. The Stellar
  oracle signer itself is Tranche 2 scope and doesn't exist yet; a
  design checklist for that future build was produced rather than
  treated as a T1 gap.
- **Solodit cross-reference** (Cyfrin's real-world finding database,
  50,000+ findings across 30+ audit firms): searched Access Control,
  Oracle, Reentrancy, Rounding, and Replay-Attack finding classes for
  patterns applicable to this contract's architecture. No new finding —
  each dominant real-world pattern was either not applicable (no on-chain
  price oracle to go stale, no external-callback reentrancy surface) or
  already covered by the checks above.

## 8. What this does NOT cover — the honest ceiling

- **No *completed* formal/symbolic verification result yet.** V8's Solidity
  contract has 10/10 Halmos properties proven with zero counterexamples. The
  Soroban equivalent is **Komet**, and it is now wired up (§5b) rather than
  absent — but it has not yet produced a citable result, so nothing here rests
  on it. Certora Sunbeam remains the heavier answer, deliberately deferred to
  Tranche 3's SCF-funded audit.
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
and `stellar-cli`. Komet (§5b) is the one exception: it is installed via
Nix/kup and is run either in CI or in the `Dockerfile.komet` container,
never on a workstation. No deployment, network access, or secrets required for
any check in this document.
