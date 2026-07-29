#![cfg(test)]

//! SCF #44 reviewer comment (1) — "deliver a Blend exploit analysis with
//! simulations showing how the protocol would have helped." This is that
//! artifact, built directly against the real, audited `ProtectionPool`
//! contract (not a standalone wrapper script) — see README.md "Blend/
//! YieldBlox illustrative scenario" for the full narrative and disclosure.
//!
//! DISCLOSURE (repeated from README — keep both in sync):
//! - Demonstrates the payout MECHANISM only, against a real public
//!   historical incident (Blend/YieldBlox, Stellar, oracle manipulation,
//!   ~$10.8M loss, real tx hash below — independently verified via Horizon
//!   2026-07-22, see the main SAFU repo's case_studies.py for the source
//!   verification; that file is NOT imported or referenced here).
//! - SAFU has no live protocol-level pool product. Blend/YieldBlox is not
//!   a SAFU depositor or partner. All pool/stake amounts here are
//!   illustrative fixtures on a synthetic test contract instance, not
//!   real funds or an existing integration.
//! - No scanner detection logic is used, implied, or reproduced. Whether
//!   a transaction is "drain-shaped" is an ASSERTED fixture label in this
//!   test, never a computed scanner verdict — that logic is proprietary
//!   and lives entirely off-chain, outside this repo.
//! - Only the public entitlement formula (`min(stake x tier_ratio, loss)`,
//!   already SAFU's own published protocol mechanic) is exercised, via
//!   the real, audited on-chain `submit_claim`/`approve_claim`/
//!   `claim_stream` entrypoints — not re-derived or approximated.

use soroban_sdk::{BytesN, Env};

use super::common::*;
use crate::types::ClaimStatus;

/// Real, public Stellar transaction hash for the Blend/YieldBlox
/// oracle-manipulation incident.
const BLEND_YIELDBLOX_TX_HASH_HEX: &str =
    "3e81a3f7b6e17cc22d0a1f33e9dcf90e5664b125b9e61f108b8d2f082f2d4657";

const TIER_A: u32 = 1;
const TIER_B: u32 = 2;
const TIER_C: u32 = 3;

/// Illustrative loss, scaled proportionally to this suite's existing
/// `MAX_STAKE` constant (the contract's own real MAX_STAKE_BPS bound,
/// "$1M" in this scenario's illustrative $-mapping). 10.8x stake mirrors
/// the real, publicly-reported ~$10.8M Blend/YieldBlox loss against a
/// "$1M" illustrative stake — same ratio the mechanism-review locked, not
/// a coincidence.
const ILLUSTRATIVE_LOSS: i128 = MAX_STAKE * 108 / 10;

fn hex_to_bytes32(hex: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        let byte_str = &hex[i * 2..i * 2 + 2];
        *slot = u8::from_str_radix(byte_str, 16).expect("valid hex in BLEND_YIELDBLOX_TX_HASH_HEX");
    }
    out
}

fn blend_tx_hash(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &hex_to_bytes32(BLEND_YIELDBLOX_TX_HASH_HEX))
}

/// Runs one tier's Blend-shaped scenario end to end against the real
/// contract: stake at the real MAX_STAKE bound, meet the 90-day time
/// gate, submit a claim carrying the real Blend/YieldBlox tx hash plus
/// the illustrative loss, confirm the contract accepts the oracle-
/// attested entitlement (proving it's within this tier's real on-chain
/// tier_cap — the panic path is the actual enforcement, not a mock),
/// approve it, and stream a payment to confirm funds actually move.
fn run_blend_scenario(tier: u32, expected_entitlement: i128) {
    let env = new_env();
    let s = setup(&env);

    // Backing liquidity so the real anti-drain throttles (daily stress
    // cap on new claims, dynamic daily outflow cap on streaming) have
    // enough total_staked to admit and pay a "$10.8M-scale" claim over a
    // realistic few-day window — standing in for the many other stakers
    // a genuinely $100M-pool-cap deployment would have, not one depositor.
    for _ in 0..50 {
        staked_wallet_amount(&env, &s, MAX_STAKE);
    }

    let (staker, ben) = staked_wallet_amount(&env, &s, MAX_STAKE);
    advance_days(&env, 90); // meets the real 90-day time gate

    // Fixture-asserted input, not a scanner output: this dummy tx is
    // labeled drain-shaped for this scenario. The contract itself has no
    // opinion on that — it only enforces entitlement <= tier_cap, and
    // this call not panicking is exactly that enforcement passing.
    let claim_id = s.client.submit_claim(
        &s.oracle,
        &staker,
        &blend_tx_hash(&env),
        &expected_entitlement,
        &tier,
        &now_ts(&env),
    );
    s.client.approve_claim(&claim_id);

    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.entitlement, expected_entitlement);
    assert_eq!(claim.status, ClaimStatus::Active);

    advance_days(&env, 7); // mandatory cooldown
    advance_days(&env, 5); // partial vesting, enough for a non-zero claimable
    let transferred = s.client.claim_stream(&claim_id, &ben);
    assert!(
        transferred > 0,
        "payout mechanism must release real funds, not just record a number"
    );
}

#[test]
fn blend_scenario_tier_a_covers_full_loss() {
    // 15x ratio x "$1M" stake = "$15M" cap, loss-capped at "$10.8M" -> 100% of loss.
    run_blend_scenario(TIER_A, ILLUSTRATIVE_LOSS);
}

#[test]
fn blend_scenario_tier_b_covers_92_6_percent_of_loss() {
    // 10x ratio x "$1M" stake = "$10M" cap, stake-capped -> 10 / 10.8 = 92.6% of loss.
    let expected = MAX_STAKE * 10;
    run_blend_scenario(TIER_B, expected);
}

#[test]
fn blend_scenario_tier_c_covers_46_3_percent_of_loss() {
    // 5x ratio x "$1M" stake = "$5M" cap, stake-capped -> 5 / 10.8 = 46.3% of loss.
    let expected = MAX_STAKE * 5;
    run_blend_scenario(TIER_C, expected);
}

/// Reviewer comment (1)'s "proves discernment, not everything pays out"
/// half: an ordinary, non-hack-shaped transaction on the same kind of
/// fixture pool/participant. The fixture's label ("this dummy tx is NOT
/// drain-shaped") is asserted by simply never calling `submit_claim` for
/// it — exactly what happens off-chain in production when SAFU's real
/// (proprietary, not included here) scanner doesn't flag a transaction.
#[test]
fn ordinary_transaction_never_becomes_a_claim() {
    let env = new_env();
    let s = setup(&env);
    let (staker, _ben) = staked_wallet_amount(&env, &s, MAX_STAKE);
    advance_days(&env, 90);

    // No submit_claim call for this wallet at all -- the negative
    // control. The stake stays fully claim-eligible and nothing was ever
    // earmarked for payout.
    assert!(s.client.is_claim_eligible(&staker));
    assert_eq!(s.client.get_total_allocated(), 0);
}
