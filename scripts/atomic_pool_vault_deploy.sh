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
stellar contract invoke \
  --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" \
  -- set_deploy_bps --bps "$DEPLOY_BPS"

echo "== Step 4: bounded first deposit (deploy_to_vault) =="
# min_shares_out=1: the pool has no prior deposit to reference a rate
# against yet (same bootstrapping constraint auto_deploy_liquidity has) —
# this first deposit is what GIVES it that reference. Bound it small
# (FIRST_DEPOSIT_AMOUNT) precisely because there's no rate-based slippage
# floor possible on the very first call.
stellar contract invoke \
  --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" \
  -- deploy_to_vault --amount "$FIRST_DEPOSIT_AMOUNT" --min_shares_out 1

echo "== Step 5: verify solvency unchanged =="
echo "liquid_balance:      $(stellar contract invoke --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" -- get_liquid_balance)"
echo "total_deployed_xlm:  $(stellar contract invoke --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" -- get_total_deployed_xlm)"
echo "total_staked:        $(stellar contract invoke --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" -- get_total_staked)"
echo "total_allocated:     $(stellar contract invoke --id "$POOL_CONTRACT_ID" --network "$NETWORK" --source "$SOURCE_ACCOUNT" -- get_total_allocated)"
echo "Confirm liquid_balance + total_deployed_xlm == the pre-deploy total_staked figure before trusting this deploy."

echo "== Done. Vault $NEW_VAULT_ID wired, deploy_bps=$DEPLOY_BPS, first deposit $FIRST_DEPOSIT_AMOUNT complete. =="
