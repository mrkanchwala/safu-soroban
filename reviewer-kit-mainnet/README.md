# SAFU — Stellar Tranche #3 reviewer self-test kit (MAINNET)

Verify SAFU's payout mechanism yourself, on the live **Stellar mainnet** contract, on your own
schedule. You do not need to take the demo video's word for anything here.

**Contract:** `CB3LZVWKGGWSYHHIE7ILK5CJH2MLUB6SWAU7UK6PMQEP3AESD3DAUBRC`
[View on Stellar Expert](https://stellar.expert/explorer/public/contract/CB3LZVWKGGWSYHHIE7ILK5CJH2MLUB6SWAU7UK6PMQEP3AESD3DAUBRC)

**Network:** Stellar **mainnet** — real capital, real transactions, nothing simulated.

**Audit status:** this contract is pre-audit. SAFU applied to the SCF Audit Bank on 2026-09-02,
immediately after Tranche 2 was approved, and is awaiting a matched auditor. Pre-audit hardening
already done: an 805-mutant mutation campaign (782 caught, 1 documented survivor, 22 unviable),
Komet symbolic verification (3/3 properties), zero clippy warnings, zero known CVEs across 194
dependencies, and 278 passing tests at 98.25% coverage. See this tranche's submission notes for
the current audit-matching status.

---

## Why this kit exists

The demo video walks through deposit → drain → claim → payout on the live mainnet pool. Every step
in it is a real on-chain transaction you can open on Stellar Expert. The last step — the payout —
has one property the others don't: the contract enforces a **7-day cooldown** between a claim
becoming `Active` and the first XLM being payable.

That cooldown is real and was deliberately **not** shortened for the demo, on mainnet any more than
it was on testnet at Tranche 2. Shortening protocol constants to make a demo look faster would mean
showing you something other than the code we actually ship.

So instead of asking you to trust a recording, this kit hands you the staker's key and the caller.
When the cooldown elapses, you trigger the payout yourself and watch it settle — for real, on
mainnet.

---

## Setup

```bash
pip install stellar-sdk
```

That's the only dependency. Both commands run against public Stellar mainnet infrastructure — no
API key, no account, nothing to sign up for.

---

## 1. Check the claim (read-only)

```bash
python3 safu-stellar-reviewer-kit-mainnet.py check
```

Reads the claim record straight off the deployed mainnet contract and shows you:

- claim status and assessed tier
- the staked principal that was **forfeited** when the claim was paid (this is how SAFU funds
  payouts — there is no premium)
- total entitlement, how much has already been paid, and how much is claimable right now
- exactly where the 7-day cooldown stands, in ledgers and as an estimated date
- once cooldown passes, the position on the 45-day linear vesting schedule

This command touches no private keys and submits no transaction.

To look at one claim only: `--claim claim1` or `--claim claim2`.

---

## 2. Trigger a real payout

```bash
python3 safu-stellar-reviewer-kit-mainnet.py stream --claim claim1
```

Calls `claim_stream(claim_id, beneficiary)` on-chain, signed by the staker's own key, for **real
mainnet XLM**. On success it prints the amount paid, the destination, the transaction hash, and a
Stellar Expert link.

To see what would happen without submitting anything:

```bash
python3 safu-stellar-reviewer-kit-mainnet.py stream --claim claim1 --dry-run
```

**Before the cooldown elapses this will refuse, by design**, reporting
`CooldownNotPassed (contract error #62)`. That refusal is itself worth seeing — it is the contract
enforcing its own rule against a caller holding a valid key. Nothing is spent and nothing is
submitted; the rejection happens at simulation.

---

## What's in the box

| Claim | Stake (forfeited) | Entitlement | Cooldown opens |
|---|---|---|---|
| `claim1` | 10 XLM | 50 XLM | ledger 64,497,372 (~2026-09-18) |
| `claim2` | 10 XLM | 50 XLM | ledger 64,533,634 (~2026-09-20) |

Both are real Tier C claims (5x coverage) produced by the actual production pipeline: a genuine
drain transaction on **mainnet**, scored by SAFU's scanner, with the verdict signed by the same
AWS KMS-held oracle key used at Tranche 2, and verified on-chain by the contract's own Ed25519
check. `claim2` additionally exercised the pool's queued-claim path — it was deferred by the daily
stress cap on submission and released the next day through the contract's own permissionless
release call, the first real exercise of that mechanism against a deployed contract anywhere.

---

## How the payout actually works

`claim_stream` is **pull-based**, not a scheduled push:

- Each call computes how much has vested since the cooldown ended (linear over 45 days), subtracts
  what has already been streamed, and pays the difference.
- **Collect at least once every 100 days.** The next call always pays the whole accumulated amount,
  so collecting weekly and collecting monthly reach the same total. But if a claim goes 100 days with
  no collection at all, anyone may call `expire_stale_claim` and the uncollected remainder returns to
  the pool. Each collection resets that window. Vesting finishes at day 45, so a single collection any
  time before the deadline gets you the full amount. `check` prints the exact deadline.
- **Every call is its own transaction** with its own hash, independently verifiable.
- A pool-wide **dynamic daily outflow cap** (5% / 3% / 1% of the pool by utilization) can cap what a
  single day's call pays out. It never reduces the total owed — it spreads it. `check` shows you the
  current ceiling.

One detail that often surprises people: the **staker** authorizes each call
(`claim.wallet.require_auth()`), while the **beneficiary** is a separate address, verified against a
hash committed at stake time, that receives the funds. SAFU supports staking from one wallet to
protect another — for example staking from a hot wallet with a cold wallet as beneficiary. This kit
therefore contains the *staker* keys only.

---

## Honest caveats

- **Mainnet, real capital, permanent record.** Unlike Tranche 2's testnet evidence, these
  transactions are not reset by any network reset — they are the permanent record.
- **This is SAFU's own capped test pool, not an open production pool.** `pool_cap = 40,000 XLM`,
  max single stake 500 XLM at that cap — small by design, bounding what the exposed test admin/
  co-signer keys could ever control, per SAFU's own standing security posture.
- **The claims reached `Active` via a 2-of-2 admin/co-signer override**, exactly the mechanism SCF
  reviewed and approved at Tranche 2. The contract normally requires a 90-day staking history
  before a claim activates, which a freshly staked demo wallet cannot have. The override
  accelerated **only the claim's timing**. The oracle's signature, the assessed tier, and the
  entitlement were all produced by the real production scoring pipeline and were not overridden.
  The 7-day cooldown was **not** bypassed — which is exactly why this kit exists.
- **These are real Stellar accounts, not throwaway test keys.** The staker keys are published
  deliberately (a staker key can trigger a payout but can never redirect it — the beneficiary is
  fixed by an on-chain hash committed at stake time). The matching beneficiary secret keys are
  **not** included in this kit and stay private, per SAFU's own design. Do not fund these accounts
  beyond what SAFU already has staked in them.
- **This contract is pre-audit.** See the Audit status note above.

---

## If something goes wrong

The script maps every contract error it can hit to a plain-language explanation. The ones you are
most likely to see:

| Error | Meaning |
|---|---|
| `CooldownNotPassed` (#62) | The 7-day cooldown is still running. Expected before the dates above. |
| `NothingVested` (#63) | Cooldown passed, but nothing new has vested since your last call. Wait, then retry. |
| `DailyOutflowCapReached` (#64) | The pool's daily payout ceiling is used up for today. Retry tomorrow. |
| `ClaimFullyStreamed` (#60) | The entire entitlement has been paid out. |
| `ClaimNotActive` (#59) | The claim is not Active. If `check` shows `Expired`, it went 100 days with no collection and was swept. |

Any of these is the protocol behaving correctly, not a broken script.
