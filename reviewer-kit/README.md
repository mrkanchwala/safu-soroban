# SAFU — Stellar Tranche #2 reviewer self-test kit

Verify SAFU's payout mechanism yourself, on the live testnet contract, on your own schedule.
You do not need to take the demo video's word for anything here.

**Contract:** `CDTXVIA4TSQ6PY76VFD4BBW4R4UMGSE5HTBNAMASAPRYRNV37DBDJJBB`
[View on Stellar Expert](https://stellar.expert/explorer/testnet/contract/CDTXVIA4TSQ6PY76VFD4BBW4R4UMGSE5HTBNAMASAPRYRNV37DBDJJBB)

**Network:** Stellar testnet

---

## Why this kit exists

The demo video walks through deposit → points → claim → payout. Every step in it is a real
on-chain transaction you can open on Stellar Expert. The last step — the payout — has one
property the others don't: the contract enforces a **7-day cooldown** between a claim becoming
`Active` and the first XLM being payable.

That cooldown is real and was deliberately **not** shortened for the demo. Shortening protocol
constants to make a demo look faster would mean showing you something other than the code we
actually intend to ship.

So instead of asking you to trust a recording, this kit hands you the staker's key and the caller.
When the cooldown elapses, you trigger the payout yourself and watch it settle.

---

## Setup

```bash
pip install stellar-sdk
```

That's the only dependency. Both commands run against public Stellar testnet infrastructure —
no API key, no account, nothing to sign up for.

---

## 1. Check the claim (read-only)

```bash
python3 safu-stellar-reviewer-kit.py check
```

Reads the claim record straight off the deployed contract and shows you:

- claim status and assessed tier
- the staked principal that was **forfeited** when the claim was paid (this is how SAFU funds
  payouts — there is no premium)
- total entitlement, how much has already been paid, and how much is claimable right now
- exactly where the 7-day cooldown stands, in ledgers and as an estimated date
- once cooldown passes, the position on the 45-day linear vesting schedule

This command touches no private keys and submits no transaction.

To look at one claim only: `--claim low` or `--claim mid`.

---

## 2. Trigger a real payout

```bash
python3 safu-stellar-reviewer-kit.py stream --claim low
```

Calls `claim_stream(claim_id, beneficiary)` on-chain, signed by the staker's own key. On success it
prints the amount paid, the destination, the transaction hash, and a Stellar Expert link.

To see what would happen without submitting anything:

```bash
python3 safu-stellar-reviewer-kit.py stream --claim low --dry-run
```

**Before the cooldown elapses this will refuse, by design**, reporting
`CooldownNotPassed (contract error #62)`. That refusal is itself worth seeing — it is the contract
enforcing its own rule against a caller holding a valid key. Nothing is spent and nothing is
submitted; the rejection happens at simulation.

---

## What's in the box

| Claim | Stake (forfeited) | Entitlement | Cooldown opens |
|---|---|---|---|
| `low` | 300 XLM | 1,500 XLM | ledger 4,362,066 (~2026-08-27) |
| `mid` | 3,800 XLM | 19,000 XLM | ledger 4,362,068 (~2026-08-27) |

`low` is the simplest walkthrough and the one the demo video follows. `mid` is included so you can
see the same mechanism at a different tier ratio and confirm the numbers are not special-cased.

Both claims were produced by the real pipeline: a genuine drain transaction on testnet, scored by
SAFU's scanner, with the verdict signed by the production KMS-held oracle key and verified on-chain
by the contract's own Ed25519 check.

---

## How the payout actually works

`claim_stream` is **pull-based**, not a scheduled push:

- Each call computes how much has vested since the cooldown ended — linear over 45 days — subtracts
  what has already been streamed, and pays the difference.
- **Skipping days costs nothing.** Call it once a week or once a month; the next call pays the whole
  accumulated amount. There is no forfeiture for not collecting.
- **Every call is its own transaction** with its own hash, independently verifiable.
- A pool-wide **dynamic daily outflow cap** (5% / 3% / 1% of the pool by utilization) can cap what a
  single day's call pays out. It never reduces the total owed — it spreads it. `check` shows you the
  current ceiling.

One detail that often surprises people: the **staker** authorizes each call
(`claim.wallet.require_auth()`), while the **beneficiary** is a separate address, verified against a
hash committed at stake time, that receives the funds. SAFU supports staking from one wallet to
protect another — for example staking from a hot wallet with a cold wallet as beneficiary. This kit
therefore contains the *staker* keys.

---

## Honest caveats

- **Testnet, not mainnet.** These transactions are verifiable **until the next Stellar testnet
  reset**, not permanently. Mainnet deployment is Tranche #3.
- **The claims reached `Active` via a 2-of-2 admin/co-signer override.** The contract normally
  requires a 90-day staking history before a claim activates, which a freshly staked demo wallet
  cannot have. The override accelerated **only the claim's timing**. The oracle's signature, the
  assessed tier, and the entitlement were all produced by the real scoring pipeline and were not
  overridden. The 7-day cooldown was **not** bypassed — which is exactly why this kit exists.
- **These keys are synthetic testnet keypairs** generated for this demonstration. They hold nothing
  of value anywhere. Do not reuse them for anything.

---

## If something goes wrong

The script maps every contract error it can hit to a plain-language explanation. The ones you are
most likely to see:

| Error | Meaning |
|---|---|
| `CooldownNotPassed` (#62) | The 7-day cooldown is still running. Expected before ~2026-08-27. |
| `NothingVested` (#63) | Cooldown passed, but nothing new has vested since your last call. Wait, then retry. |
| `DailyOutflowCapReached` (#64) | The pool's daily payout ceiling is used up for today. Retry tomorrow. |
| `ClaimFullyStreamed` (#60) | The entire entitlement has been paid out. |

Any of these is the protocol behaving correctly, not a broken script.
