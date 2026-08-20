#!/usr/bin/env python3
"""
SAFU Stellar Tranche #2 — reviewer self-test kit.

Lets an SCF reviewer independently verify and then TRIGGER a real payout
from SAFU's deployed Soroban ProtectionPool, on their own schedule, without
depending on a pre-recorded video capture.

Two commands:

  check   Read-only. Reads the live claim record straight off the deployed
          contract and shows: claim status, where the 7-day cooldown stands,
          how much has vested so far under the 45-day linear schedule, how
          much is claimable right now, and how much has already been paid.
          Touches no keys and submits nothing.

  stream  Calls `claim_stream(claim_id, beneficiary)` on-chain, signed by the
          staker's own key, and reports the resulting transaction hash plus a
          Stellar Expert link. This is the real payout — pull-based, exactly
          the same entrypoint any staker would use.

Why a script rather than a captured payout: the contract enforces a real
7-day cooldown (`CooldownNotPassed`) that is not compressible and was
deliberately not shortened for the demo. Rather than shorten it or ask a
reviewer to take a recording on faith, the reviewer gets the keys and the
caller and can produce the payout themselves whenever they choose.

TESTNET ONLY. Every key in the accompanying config is a synthetic testnet
keypair generated for this demonstration and holds nothing of value.

Usage:
  python3 safu-stellar-reviewer-kit.py check
  python3 safu-stellar-reviewer-kit.py check --claim low
  python3 safu-stellar-reviewer-kit.py stream --claim low
  python3 safu-stellar-reviewer-kit.py stream --claim low --dry-run

Requires: pip install stellar-sdk
"""

import argparse
import json
import os
import re
import sys
import time
from datetime import UTC, datetime, timedelta

from stellar_sdk import Keypair, Network, SorobanServer, TransactionBuilder, scval
from stellar_sdk.soroban_rpc import GetTransactionStatus, SendTransactionStatus

# Contract constants — mirrored from contracts/protection-pool/src/types.rs.
# Used only to EXPLAIN what the contract is doing; the contract itself is the
# sole enforcement. A stale value here can mislead a printout, never mis-pay.
LEDGERS_PER_DAY = 17_280
VESTING_LEDGERS = 45 * LEDGERS_PER_DAY
COOLDOWN_LEDGERS = 7 * LEDGERS_PER_DAY
# Rule B sweep: after this long with no claim_stream call, ANYONE may call
# expire_stale_claim and the uncollected remainder returns to the pool.
# Each collection resets the clock (contract sets last_collected_ledger = now).
COLLECTION_INACTIVITY_LEDGERS = 100 * LEDGERS_PER_DAY
BPS_DENOMINATOR = 10_000
STROOPS_PER_XLM = 10_000_000
SECONDS_PER_LEDGER = 5  # nominal Stellar close time, for date estimates only

POLL_MAX_ATTEMPTS = 30
POLL_INTERVAL_SECONDS = 2
AUTH_VALID_LEDGERS_AHEAD = 100

CLAIM_STATUS = {
    0: "Unused",
    1: "Active",
    2: "Completed",
    3: "Cancelled",
    4: "Reserved",
    5: "PendingTime",
    6: "AwaitingApproval",
    7: "Expired",
}

TIER_NAMES = {1: "A", 2: "B", 3: "C"}

# PoolError codes reachable from claim_stream — contracts/protection-pool/src/error.rs
POOL_ERRORS = {
    1: ("NoStake", "No stake record found for this claim's wallet."),
    31: (
        "WrongBeneficiary",
        "The beneficiary passed does not match the hash committed at stake time. "
        "Use the beneficiary address from this kit's config.",
    ),
    43: ("StakeSuspended", "The stake is suspended; payouts are blocked."),
    53: ("NoSuchClaim", "No claim exists with this id on this contract."),
    59: (
        "ClaimNotActive",
        "The claim is not in Active status, so it cannot stream. If it shows Expired, "
        "it went 100 days with no collection and was swept -- the uncollected remainder "
        "returned to the pool.",
    ),
    60: (
        "ClaimFullyStreamed",
        "The full entitlement has already been paid out. Nothing left to stream.",
    ),
    62: (
        "CooldownNotPassed",
        "The 7-day cooldown has not elapsed yet. This is the contract working as "
        "designed, not a failure -- run `check` to see the exact ledger and "
        "estimated date when streaming opens.",
    ),
    63: (
        "NothingVested",
        "Cooldown has passed but no NEW amount has vested since the last call. "
        "Vesting is linear over 45 days, so wait a while and call again.",
    ),
    64: (
        "DailyOutflowCapReached",
        "The pool's dynamic daily outflow cap for today is already used up. "
        "This is a pool-wide anti-drain limit. Try again tomorrow.",
    ),
    # The two below are NOT raised by claim_stream's own logic -- they come from
    # guards it calls into, so they are easy to miss when mapping the function
    # line by line. Both are reachable: `pause` is an admin action, and the
    # liquidity check consults the yield vault, which holds real XLM.
    80: (
        "InsufficientLiquidity",
        "The pool does not have enough liquid XLM on hand right now, because "
        "some is deployed in the yield vault. The amount you are owed is "
        "unchanged. Retry shortly; if it persists the operators need to return "
        "funds from the vault.",
    ),
    93: (
        "Paused",
        "The pool is paused, which stops all payouts while it is in effect. "
        "Your claim and everything vested are unaffected. Retry once it resumes.",
    ),
}


def log(msg):
    print(msg, flush=True)


def xlm(stroops):
    return f"{stroops / STROOPS_PER_XLM:,.7f} XLM"


def load_config(path):
    if not os.path.exists(path):
        sys.exit(
            f"Config not found: {path}\n"
            "Expected `reviewer-wallets.json` next to this script, or pass --config."
        )
    with open(path) as fh:
        return json.load(fh)


def pick_claims(cfg, label):
    claims = cfg["claims"]
    if label:
        match = [c for c in claims if c["label"] == label]
        if not match:
            avail = ", ".join(c["label"] for c in claims)
            sys.exit(f"No claim labelled '{label}'. Available: {avail}")
        return match
    return claims


def parse_contract_error(err):
    """Pull a PoolError code out of an RPC error blob, if there is one."""
    text = str(err)
    m = re.search(r"Error\(Contract, #(\d+)\)", text)
    if not m:
        m = re.search(r"#(\d+)", text)
    if not m:
        return None, text
    code = int(m.group(1))
    return code, text


def explain_error(err):
    code, text = parse_contract_error(err)
    if code is not None and code in POOL_ERRORS:
        name, hint = POOL_ERRORS[code]
        return f"{name} (contract error #{code})\n  -> {hint}"
    if code is not None:
        return f"contract error #{code} (not a claim_stream error code)\n  raw: {text}"
    return f"RPC/simulation error\n  raw: {text}"


def read_contract(server, contract_id, source_pub, fn, params):
    """Simulate a call purely to read its return value. Nothing is signed or sent."""
    src = server.load_account(source_pub)
    tx = (
        TransactionBuilder(src, Network.TESTNET_NETWORK_PASSPHRASE, base_fee=1000)
        .append_invoke_contract_function_op(
            contract_id=contract_id, function_name=fn, parameters=params
        )
        .set_timeout(30)
        .build()
    )
    sim = server.simulate_transaction(tx)
    if sim.error:
        raise RuntimeError(f"read `{fn}` failed: {sim.error}")
    return scval.to_native(sim.results[0].xdr)


def ledger_to_estimated_utc(target_ledger, now_ledger):
    delta = (target_ledger - now_ledger) * SECONDS_PER_LEDGER
    return datetime.now(UTC) + timedelta(seconds=delta)


def vesting_snapshot(claim, now_ledger):
    """Reproduce claim.rs:816-822 exactly, so the numbers shown match what the
    contract will actually pay."""
    cooldown_end = claim["cooldown_ends_ledger"]
    vesting_end = claim["vesting_ends_ledger"]
    entitlement = claim["entitlement"]
    streamed = claim["streamed"]

    elapsed_end = min(now_ledger, vesting_end)
    elapsed = max(0, elapsed_end - cooldown_end)
    vested_total = entitlement * elapsed // VESTING_LEDGERS
    claimable = vested_total - streamed
    return {
        "cooldown_passed": now_ledger >= cooldown_end,
        "ledgers_to_cooldown": max(0, cooldown_end - now_ledger),
        "elapsed_vesting_ledgers": elapsed,
        "vested_total": vested_total,
        "claimable": max(0, claimable),
        "fully_vested": now_ledger >= vesting_end,
    }


def outflow_cap(server, contract_id, source_pub, claim):
    """Pool-wide daily payout ceiling — claim.rs:824-838.

    `daily_outflow_so_far` has no public getter, so this reports the ceiling
    itself, not the remaining headroom. Any streaming already done today by
    any claim eats into it. Shown as context; the simulate step is what
    reveals the true payable amount.
    """
    total_staked = read_contract(server, contract_id, source_pub, "get_total_staked", [])
    total_allocated = read_contract(server, contract_id, source_pub, "get_total_allocated", [])
    cap_base = max(total_staked, claim["total_staked_snapshot"])
    if cap_base == 0:
        bps = 100
    else:
        utilization_bps = total_allocated * BPS_DENOMINATOR // cap_base
        bps = 500 if utilization_bps < 2_000 else (300 if utilization_bps < 5_000 else 100)
    return {
        "total_staked": total_staked,
        "total_allocated": total_allocated,
        "cap_base": cap_base,
        "bps": bps,
        "cap_today": cap_base * bps // BPS_DENOMINATOR,
    }


def expert_tx(txid):
    return f"https://stellar.expert/explorer/testnet/tx/{txid}"


def expert_contract(cid):
    return f"https://stellar.expert/explorer/testnet/contract/{cid}"


def cmd_check(cfg, args):
    server = SorobanServer(cfg["soroban_rpc_url"])
    contract_id = cfg["contract_id"]
    now_ledger = server.get_latest_ledger().sequence
    selected = pick_claims(cfg, args.claim)
    any_pub = selected[0]["staker_public"]
    pool_staked = read_contract(server, contract_id, any_pub, "get_total_staked", [])
    pool_allocated = read_contract(server, contract_id, any_pub, "get_total_allocated", [])

    log("=" * 72)
    log("SAFU Stellar ProtectionPool - claim status (read-only)")
    log("=" * 72)
    log(f"contract      : {contract_id}")
    log(f"               {expert_contract(contract_id)}")
    log(f"network       : {cfg['network']}")
    log(f"current ledger: {now_ledger:,}")
    log("")

    for c in selected:
        source_pub = c["staker_public"]
        claim = read_contract(
            server,
            contract_id,
            source_pub,
            "get_claim",
            [scval.to_bytes(bytes.fromhex(c["claim_id"]))],
        )
        if claim is None:
            log(f"[{c['label']}] claim {c['claim_id'][:16]}... NOT FOUND on this contract")
            log("")
            continue

        v = vesting_snapshot(claim, now_ledger)
        cap = outflow_cap(server, contract_id, source_pub, claim)
        status = CLAIM_STATUS.get(claim["status"], f"unknown({claim['status']})")

        log(f"--- claim [{c['label']}] ---------------------------------------------")
        log(f"claim id     : {c['claim_id']}")
        log(f"staker       : {source_pub}")
        log(f"beneficiary  : {c['beneficiary_public']}   (receives the payout)")
        log(f"status       : {status}")
        log(f"tier         : {TIER_NAMES.get(claim['tier'], claim['tier'])}")
        log(f"stake forfeit: {xlm(claim['stake'])}   (principal, forfeited on a paid claim)")
        log(f"entitlement  : {xlm(claim['entitlement'])}")
        log(f"already paid : {xlm(claim['streamed'])}")
        log("")

        if not v["cooldown_passed"]:
            eta = ledger_to_estimated_utc(claim["cooldown_ends_ledger"], now_ledger)
            log("COOLDOWN: still running (contract-enforced 7 days, not compressible)")
            log(f"  opens at ledger : {claim['cooldown_ends_ledger']:,}")
            log(f"  ledgers to go   : {v['ledgers_to_cooldown']:,}")
            log(f"  estimated (UTC) : ~{eta:%Y-%m-%d %H:%M}  (at ~5s/ledger)")
            log("  `stream` will return CooldownNotPassed until then. That is correct behaviour.")
        else:
            day = v["elapsed_vesting_ledgers"] / LEDGERS_PER_DAY
            log(f"VESTING: open - linear over 45 days, day {day:,.2f} of 45")
            log(f"  vested to date  : {xlm(v['vested_total'])}")
            log(f"  already paid    : {xlm(claim['streamed'])}")
            log(f"  CLAIMABLE NOW   : {xlm(v['claimable'])}")
            if v["fully_vested"]:
                log("  fully vested - the whole entitlement is available.")
            log("")
            log(f"  pool daily outflow ceiling: {xlm(cap['cap_today'])} "
                f"({cap['bps'] / 100:.0f}% of {xlm(cap['cap_base'])})")
            log("  (pool-wide, shared across all claims; reduces a single call, never the total owed)")

        # Rule B sweep -- shown whether or not cooldown has passed, because the
        # clock starts at the cooldown end, not at the first collection.
        deadline = claim["last_collected_ledger"] + COLLECTION_INACTIVITY_LEDGERS
        left = deadline - now_ledger
        log("")
        log("COLLECTION DEADLINE (do not ignore)")
        log(f"  collect before ledger : {deadline:,}  (~{ledger_to_estimated_utc(deadline, now_ledger):%Y-%m-%d})")
        if left > 0:
            log(f"  time left             : {left:,} ledgers (~{left / LEDGERS_PER_DAY:.1f} days)")
            log("  If a claim goes 100 days with no collection, ANYONE may close it and the")
            log("  uncollected remainder returns to the pool. Each collection resets this window.")
            log("  Vesting finishes at day 45, so one collection before the deadline gets the full amount.")
        else:
            log("  PASSED - this claim can now be swept by anyone; the remainder may already be gone.")
            if v["claimable"] > 0:
                log("")
                log(f"  -> ready: python3 {os.path.basename(__file__)} stream --claim {c['label']}")
        log("")

    log("Pool state: "
        f"total staked {xlm(pool_staked)} | allocated to claims {xlm(pool_allocated)}")
    log("")
    log("NOTE: testnet data is verifiable until the next Stellar testnet reset.")


def cmd_stream(cfg, args):
    server = SorobanServer(cfg["soroban_rpc_url"])
    contract_id = cfg["contract_id"]
    claims = pick_claims(cfg, args.claim)
    if len(claims) > 1:
        sys.exit("`stream` needs exactly one claim. Pass --claim <label>.")
    c = claims[0]

    if not c.get("staker_secret"):
        sys.exit(f"No staker_secret in config for claim '{c['label']}'.")

    staker_kp = Keypair.from_secret(c["staker_secret"])
    if staker_kp.public_key != c["staker_public"]:
        sys.exit(
            "Config mismatch: staker_secret does not correspond to staker_public. "
            "Refusing to build a transaction."
        )

    now_ledger = server.get_latest_ledger().sequence
    claim = read_contract(
        server,
        contract_id,
        c["staker_public"],
        "get_claim",
        [scval.to_bytes(bytes.fromhex(c["claim_id"]))],
    )
    if claim is None:
        sys.exit(f"Claim {c['claim_id']} not found on contract {contract_id}.")

    v = vesting_snapshot(claim, now_ledger)
    log(f"claim [{c['label']}] {c['claim_id'][:16]}...")
    log(f"  status      : {CLAIM_STATUS.get(claim['status'], claim['status'])}")
    log(f"  entitlement : {xlm(claim['entitlement'])}")
    log(f"  already paid: {xlm(claim['streamed'])}")
    log(f"  claimable   : {xlm(v['claimable'])}")
    if not v["cooldown_passed"]:
        eta = ledger_to_estimated_utc(claim["cooldown_ends_ledger"], now_ledger)
        log("")
        log(f"  Cooldown has {v['ledgers_to_cooldown']:,} ledgers left "
            f"(~{eta:%Y-%m-%d %H:%M} UTC). The call below will revert with "
            "CooldownNotPassed - that is the contract enforcing its own rule.")
    log("")

    # `claim.wallet.require_auth()` (claim.rs:793) -- the STAKER authorizes,
    # not the beneficiary. The beneficiary is checked separately against the
    # hash committed at stake time (claim.rs:811-814).
    source = server.load_account(staker_kp.public_key)
    tx = (
        TransactionBuilder(source, Network.TESTNET_NETWORK_PASSPHRASE, base_fee=1000)
        .append_invoke_contract_function_op(
            contract_id=contract_id,
            function_name="claim_stream",
            parameters=[
                scval.to_bytes(bytes.fromhex(c["claim_id"])),
                scval.to_address(c["beneficiary_public"]),
            ],
        )
        .set_timeout(60)
        .build()
    )

    log("Simulating (no cost, nothing submitted)...")
    sim = server.simulate_transaction(tx)
    if sim.error:
        log("")
        log("SIMULATION REJECTED - nothing was submitted, nothing was spent.")
        log(f"  {explain_error(sim.error)}")
        sys.exit(1)

    payout = scval.to_native(sim.results[0].xdr)
    log(f"Simulation OK. This call would pay out: {xlm(payout)}")

    if args.dry_run:
        log("")
        log("--dry-run set - stopping here. Re-run without it to submit for real.")
        return

    from stellar_sdk.auth import authorize_entry

    latest = server.get_latest_ledger()
    valid_until = latest.sequence + AUTH_VALID_LEDGERS_AHEAD
    op_auth = sim.results[0].auth or []
    if op_auth:
        tx.transaction.operations[0].auth = [
            authorize_entry(e, staker_kp, valid_until, Network.TESTNET_NETWORK_PASSPHRASE)
            for e in op_auth
        ]

    prepared = server.prepare_transaction(tx, sim)
    prepared.sign(staker_kp)

    log("Submitting...")
    resp = server.send_transaction(prepared)
    if resp.status == SendTransactionStatus.ERROR:
        log("")
        log("SUBMIT REJECTED")
        log(f"  {explain_error(getattr(resp, 'error_result_xdr', resp))}")
        sys.exit(1)

    txid = resp.hash
    for _ in range(POLL_MAX_ATTEMPTS):
        result = server.get_transaction(txid)
        if result.status in (GetTransactionStatus.SUCCESS, GetTransactionStatus.FAILED):
            break
        time.sleep(POLL_INTERVAL_SECONDS)
    else:
        log(f"Transaction {txid} did not resolve in "
            f"{POLL_MAX_ATTEMPTS * POLL_INTERVAL_SECONDS}s. Check it directly:")
        log(f"  {expert_tx(txid)}")
        sys.exit(1)

    if result.status != GetTransactionStatus.SUCCESS:
        log("")
        log(f"TRANSACTION FAILED: {result.status}")
        log(f"  {expert_tx(txid)}")
        sys.exit(1)

    log("")
    log("=" * 72)
    log("PAYOUT SENT")
    log("=" * 72)
    log(f"  amount      : {xlm(payout)}")
    log(f"  to          : {c['beneficiary_public']}")
    log(f"  transaction : {txid}")
    log(f"  verify      : {expert_tx(txid)}")
    log("")
    log("Re-run `check` to see `already paid` increase and `claimable` reset.")
    log("Vesting is linear over 45 days, so more becomes claimable as time passes;")
    log("call this as often or as rarely as you like - skipping days loses nothing,")
    log("the next call pays the whole accumulated amount.")


def main():
    default_cfg = os.path.join(os.path.dirname(os.path.abspath(__file__)), "reviewer-wallets.json")

    p = argparse.ArgumentParser(
        description="SAFU Stellar T2 reviewer self-test kit (testnet).",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument("--config", default=default_cfg, help="path to reviewer-wallets.json")
    sub = p.add_subparsers(dest="cmd", required=True)

    pc = sub.add_parser("check", help="read-only claim + vesting status")
    pc.add_argument("--claim", help="claim label (default: all)")

    ps = sub.add_parser("stream", help="trigger a real payout")
    ps.add_argument("--claim", required=True, help="claim label")
    ps.add_argument("--dry-run", action="store_true", help="simulate only, do not submit")

    args = p.parse_args()
    cfg = load_config(args.config)

    if args.cmd == "check":
        cmd_check(cfg, args)
    else:
        cmd_stream(cfg, args)


if __name__ == "__main__":
    main()
