#!/usr/bin/env bash
# T3 item 3 (2026-08-24): atomic pool+vault deploy.
#
# Sequences what was three separate manual steps in the T2 testnet deploy
# (2026-08-20, memory/jobs/2026-08-20_safu-t2-d4-step13-yield-leg-vault-deploy-and-mainnet-findings.md)
# into one operator-triggered script: create the DeFindex vault, wire it to
# the pool, set a deploy_bps ceiling, and run a bounded first deposit so the
# pool is fully yield-ready in one pass.
#
# Deliberately NOT a contract function (locked 2026-08-13,
# memory/projects/safu/t2-build-prep.md:648) — baking a cross-contract call
# to DeFindex's factory into the pool's own constructor would couple the
# pool's own deployability to DeFindex's factory being live and correctly
# configured at that exact moment, and DeFindex is Stellar-specific, which
# wouldn't generalize to a future chain deployment.
#
# STOP before running: this script's DeFindex factory call args are NOT
# independently verified in this session — the exact `create_defindex_vault`
# interface was pulled live via `stellar contract info interface` in the
# 2026-08-20 session, not recorded anywhere in this workspace. Re-verify it
# live the same way before running against real funds — do not trust the
# args below from memory. Everything from Step 2 onward (this contract's
# own set_vault/set_deploy_bps/deploy_to_vault) is exact, pulled directly
# from contracts/protection-pool/src/vault.rs and lib.rs.

set -euo pipefail

: "${NETWORK:?Set NETWORK (e.g. testnet)}"
: "${SOURCE_ACCOUNT:?Set SOURCE_ACCOUNT (admin identity, already configured in stellar-cli)}"
: "${POOL_CONTRACT_ID:?Set POOL_CONTRACT_ID (this pool's deployed contract ID)}"
: "${DEFINDEX_FACTORY_ID:?Set DEFINDEX_FACTORY_ID}"
: "${VAULT_MANAGER_MULTISIG:?Set VAULT_MANAGER_MULTISIG (Manager role address)}"
: "${VAULT_REBALANCE_ADDR:?Set VAULT_REBALANCE_ADDR (RebalanceManager role address)}"
: "${VAULT_EMERGENCY_ADDR:?Set VAULT_EMERGENCY_ADDR (EmergencyManager role address)}"
: "${VAULT_FEE_RECEIVER:?Set VAULT_FEE_RECEIVER (usually the pool's own treasury address)}"
: "${DEPLOY_BPS:?Set DEPLOY_BPS (e.g. 500 for 5%; must be <= MAX_DEPLOY_BPS = 8000)}"
: "${FIRST_DEPOSIT_AMOUNT:?Set FIRST_DEPOSIT_AMOUNT (bounded test deposit, in stroops)}"

# ---------------------------------------------------------------------------
# DeFindex build pinning (added 2026-09-01 after reviewing OtterSec's DeFindex
# audit + Certora/Code4rena's Blend audits).
#
# These are DeFindex's own PUBLISHED mainnet values (docs.defindex.io,
# "Contract Deployments"). Pinning them turns "verify the vault is a current
# post-audit build" from an operator memory item into a checked gate.
#
# ONLY ENFORCED ON MAINNET. DeFindex redeploys testnet frequently and publishes
# testnet addresses only in a repo JSON, so there is no stable testnet hash to
# pin -- asserting one would break every testnet run of this script.
#
# Override via env when DeFindex ships a new build; do not silently edit these.
: "${EXPECTED_FACTORY_ID:=CDKFHFJIET3A73A2YN4KV7NSV32S6YGQMUFH3DNJXLBWL4SKEGVRNFKI}"
: "${EXPECTED_VAULT_WASM:=ae3409a4090bc087b86b4e9b444d2b8017ccd97b90b069d44d005ab9f8e1468b}"

# `stellar contract info hash` prints log lines alongside its output, and the
# unquote() helper below is for JSON-wrapped values, not this. A stray newline
# would produce a false mismatch -- and after Step 1 a false mismatch means a
# spuriously orphaned vault. Trim explicitly.
wasm_hash_of() {
  stellar contract info hash --id "$1" --network "$NETWORK" 2>/dev/null \
    | tr -d '"' | tr -d '[:space:]'
}

is_mainnet() { [ "$NETWORK" = "mainnet" ] || [ "$NETWORK" = "pubnet" ] || [ "$NETWORK" = "public" ]; }

# `stellar contract invoke` emits JSON, so an address comes back wrapped in
# literal double quotes ("CCSS44...") and a None reads as `null`. Every value
# captured from the CLI goes through this — passing a quote-wrapped string
# straight back into a later `--arg` silently sends the wrong value.
unquote() { tr -d '"' <<<"$1" | tr -d '[:space:]'; }

echo "== Step 0: pre-flight — confirm the pool has no vault wired yet =="
EXISTING_VAULT=$(unquote "$(stellar contract invoke \
  --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" \
  -- get_vault)")
if [ "$EXISTING_VAULT" != "null" ] && [ -n "$EXISTING_VAULT" ]; then
  echo "ABORT: pool already has a vault wired ($EXISTING_VAULT). This script is for first-time setup only." >&2
  exit 1
fi

# Deliberately placed BEFORE Step 1: no vault exists yet, so a hard abort here
# strands nothing. Every check AFTER Step 1 must be a confirm, not a hard abort.
if is_mainnet; then
  echo "== Pre-flight: DeFindex factory pin (mainnet) =="
  if [ "$DEFINDEX_FACTORY_ID" != "$EXPECTED_FACTORY_ID" ]; then
    echo "ABORT: DEFINDEX_FACTORY_ID does not match the pinned mainnet factory." >&2
    echo "  expected: $EXPECTED_FACTORY_ID" >&2
    echo "  got:      $DEFINDEX_FACTORY_ID" >&2
    echo "  Nothing has been created. If DeFindex has published a new factory," >&2
    echo "  re-verify it against docs.defindex.io and set EXPECTED_FACTORY_ID." >&2
    exit 1
  fi
  echo "Factory matches pinned mainnet value."
else
  echo "== Pre-flight: factory pin SKIPPED (network=$NETWORK, not mainnet) =="
fi

echo "== STOP: re-verify the DeFindex factory interface live before continuing =="
echo "Run: stellar contract info interface --id $DEFINDEX_FACTORY_ID --network $NETWORK"
echo "Confirm create_defindex_vault's exact parameter names/order match what this script sends below."
read -r -p "Verified live and matches? Type 'yes' to continue: " CONFIRM
if [ "$CONFIRM" != "yes" ]; then
  echo "Aborted — re-verify before running." >&2
  exit 1
fi

echo "== Step 1: create_defindex_vault (VaultFee=0, upgradable=false) =="
# Role-ID enum, verified 2026-08-20: EmergencyManager=0, VaultFeeReceiver=1,
# Manager=2, RebalanceManager=3. Manager should be a 2-of-3 multisig, not a
# single key (matches the 2026-08-20 precedent: vault_mgr1/2/3, thresholds
# {2,2,2}) — set that up as its own multisig account BEFORE running this.
NEW_VAULT_ID=$(unquote "$(stellar contract invoke \
  --id "$DEFINDEX_FACTORY_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" \
  -- create_defindex_vault \
  --vault_fee 0 \
  --upgradable false \
  --emergency_manager "$VAULT_EMERGENCY_ADDR" \
  --vault_fee_receiver "$VAULT_FEE_RECEIVER" \
  --manager "$VAULT_MANAGER_MULTISIG" \
  --rebalance_manager "$VAULT_REBALANCE_ADDR")")
echo "New vault: $NEW_VAULT_ID"

# From here on a real vault exists on-chain. Every abort path below must say
# so explicitly — otherwise an operator re-runs the script, Step 0 passes
# (the pool still has no vault wired), and a SECOND vault gets created while
# the first is left stranded and unreferenced.
orphan_warn() {
  echo "" >&2
  echo "!! A DeFindex vault WAS created before this failure: $NEW_VAULT_ID" >&2
  echo "!! It is NOT wired to the pool. Do not re-run this script blindly —" >&2
  echo "!! either wire this vault by hand via set_vault, or record it as" >&2
  echo "!! abandoned before creating another." >&2
}

# A vault now exists. This check therefore CONFIRMS rather than aborting: a
# stale pin must cost one deliberate operator "yes", not a stranded vault.
if is_mainnet; then
  echo "== Step 1b: verify the created vault's WASM build =="
  ACTUAL_VAULT_WASM=$(wasm_hash_of "$NEW_VAULT_ID")
  if [ "$ACTUAL_VAULT_WASM" != "$EXPECTED_VAULT_WASM" ]; then
    echo "" >&2
    echo "!! VAULT BUILD MISMATCH." >&2
    echo "!!   expected: $EXPECTED_VAULT_WASM" >&2
    echo "!!   actual:   $ACTUAL_VAULT_WASM" >&2
    echo "!! The factory produced a vault this script does not recognise." >&2
    echo "!! Either DeFindex shipped a new build (re-verify against their docs" >&2
    echo "!! and update EXPECTED_VAULT_WASM), or the factory is not what it" >&2
    echo "!! claims to be. The OtterSec audit findings this pin exists for" >&2
    echo "!! (share inflation, bRate manipulation) were fixed upstream; an" >&2
    echo "!! unrecognised build has NOT been checked for them." >&2
    orphan_warn
    read -r -p "Continue anyway with this unrecognised vault build? Type 'yes': " VCONFIRM
    if [ "$VCONFIRM" != "yes" ]; then
      echo "Aborted at operator request." >&2
      exit 1
    fi
  else
    echo "Vault build matches the pinned post-audit DeFindex WASM."
  fi
else
  echo "== Step 1b: vault WASM pin SKIPPED (network=$NETWORK, not mainnet) =="
  echo "   informational — created vault WASM: $(wasm_hash_of "$NEW_VAULT_ID")"
fi

echo "== Step 2: set_vault (pool's own function, exact signature) =="
if ! stellar contract invoke \
  --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" \
  -- set_vault --vault_address "$NEW_VAULT_ID"; then
  echo "ABORT: set_vault failed." >&2
  orphan_warn
  exit 1
fi

VERIFIED_VAULT=$(unquote "$(stellar contract invoke \
  --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" \
  -- get_vault)")
if [ "$VERIFIED_VAULT" != "$NEW_VAULT_ID" ]; then
  echo "ABORT: get_vault() returned $VERIFIED_VAULT, expected $NEW_VAULT_ID" >&2
  orphan_warn
  exit 1
fi

echo "== Step 3: set_deploy_bps =="
# Guarded like Step 2. Without this, `set -e` aborts here with a vault already
# created AND wired, and the operator gets no orphan warning at all.
if ! stellar contract invoke \
  --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" \
  -- set_deploy_bps --bps "$DEPLOY_BPS"; then
  echo "ABORT: set_deploy_bps failed." >&2
  orphan_warn
  exit 1
fi

echo "== Step 3b: confirm the vault is still untouched before depositing =="
# WHY THIS EXISTS (2026-09-01). OtterSec OS-DIX-ADV-03 (HIGH) on DeFindex: an
# attacker deposits into a fresh vault, withdraws all but one share, donates to
# inflate the share price, and the next depositor's shares round down -- victim
# loses ~25%. Our first deposit is precisely that "next depositor", into a vault
# this script created moments earlier whose address is printed above.
#
# `total_supply` detects that condition DIRECTLY rather than inferring it from a
# derived rate, and its signature is infallible (`-> i128`, no Result), so it
# cannot revert on an empty vault the way a share-price quote might.
#
# Current DeFindex builds also mint locked "dead shares" on first deposit, which
# independently blunts this attack -- that is why the WASM pin above matters,
# and why this remains a second layer rather than the only one.
VAULT_SUPPLY=$(unquote "$(stellar contract invoke \
  --id "$NEW_VAULT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" \
  --send=no -- total_supply)")
echo "vault total_supply before our deposit: $VAULT_SUPPLY"
if [ "$VAULT_SUPPLY" != "0" ]; then
  echo "" >&2
  echo "!! The vault we just created ALREADY HAS SHARES OUTSTANDING ($VAULT_SUPPLY)." >&2
  echo "!! Nobody should have been able to deposit between vault creation and" >&2
  echo "!! now. Treat this as a possible share-inflation front-run" >&2
  echo "!! (OtterSec OS-DIX-ADV-03) and do NOT deposit into it." >&2
  orphan_warn
  exit 1
fi

echo "== Step 4: bounded first deposit (deploy_to_vault) =="
# min_shares_out is a real floor, not the placeholder 1 this script shipped with.
# With total_supply proven 0 above, a fresh DeFindex vault mints approximately
# 1:1 (minus dead shares), so anything far below the deposit amount means the
# rate moved between the check and the deposit -- fail closed.
#
# This matters beyond the deposit itself: deploy_to_vault's result sets
# deployed_shares/deployed_xlm, which is the ONLY reference rate
# auto_deploy_liquidity ever has. A bad first deposit poisons that anchor for
# every automatic deposit afterwards.
MIN_SHARES_OUT="${MIN_SHARES_OUT:-$(( FIRST_DEPOSIT_AMOUNT / 2 ))}"
echo "min_shares_out floor: $MIN_SHARES_OUT (deposit $FIRST_DEPOSIT_AMOUNT)"
if ! stellar contract invoke \
  --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" \
  -- deploy_to_vault --amount "$FIRST_DEPOSIT_AMOUNT" --min_shares_out "$MIN_SHARES_OUT"; then
  echo "ABORT: deploy_to_vault failed (MinSharesNotMet means the floor held)." >&2
  orphan_warn
  exit 1
fi

echo "== Step 5: verify solvency unchanged =="
echo "liquid_balance:      $(stellar contract invoke --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" -- get_liquid_balance)"
echo "total_deployed_xlm:  $(stellar contract invoke --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" -- get_total_deployed_xlm)"
echo "total_staked:        $(stellar contract invoke --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" -- get_total_staked)"
echo "total_allocated:     $(stellar contract invoke --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" -- get_total_allocated)"
echo "Confirm liquid_balance + total_deployed_xlm == the pre-deploy total_staked figure before trusting this deploy."

echo "== Done. Vault $NEW_VAULT_ID wired, deploy_bps=$DEPLOY_BPS, first deposit $FIRST_DEPOSIT_AMOUNT complete. =="
