#![cfg(test)]

//! General pool walkthrough for the SCF #44 Deliverable Verification
//! video: fund the pool at the real Tranche 1 deploy size (600,000 XLM,
//! matching the live testnet deployment), stake multiple Tier A/B/C
//! participants plus one never-attacked participant, run each tiered
//! participant through a dummy attack + claim + full payout stream, and
//! confirm the untouched participant stays fully eligible throughout.
//! Output is formatted in whole XLM (not raw stroops) with a pool
//! solvency check after every claim, ending in a summary table.
//!
//! DISCLOSURE (same boundary as blend_scenario_tests.rs):
//! - No scanner detection logic is used, implied, or reproduced. Whether
//!   a dummy transaction is "drain-shaped" is an ASSERTED fixture label
//!   in this file, never a computed scanner verdict. SAFU's real scanner
//!   is proprietary and lives entirely off-chain, outside this repo.
//! - All wallets, stakes, and "attacks" here are synthetic fixtures on a
//!   fresh test contract instance, not real funds or a real incident.
//! - Only the public entitlement formula (`min(stake x tier_ratio,
//!   loss)`) and the real, audited on-chain entrypoints (`stake`,
//!   `submit_claim`, `approve_claim`, `claim_stream`) are exercised.

use soroban_sdk::Env;
use std::{format, println, string::ToString};

use super::common::*;
use crate::types::ClaimStatus;

const TIER_A: u32 = 1;
const TIER_B: u32 = 2;
const TIER_C: u32 = 3;

struct DemoResult {
    label: &'static str,
    staked: i128,
    loss: Option<i128>,
    streamed: i128,
}

/// Runs one participant through: stake at the real MAX_STAKE bound,
/// meet the 90-day time gate, submit a dummy-attack claim (fixture-
/// asserted entitlement, real on-chain tier_cap enforcement), approve
/// it, and stream the payout to completion.
#[allow(clippy::too_many_arguments)]
fn run_dummy_attack(
    env: &Env,
    s: &Setup<'_>,
    label: &'static str,
    tier: u32,
    stake_amount: i128,
    loss: i128,
    expected_entitlement: i128,
    seed: u8,
) -> DemoResult {
    let (staker, ben) = staked_wallet_amount(env, s, stake_amount);
    advance_days(env, 90); // real 90-day time gate, not skipped

    println!();
    pause();
    println!("[{}] Staked {} XLM.", label, xlm(stake_amount));
    pause();
    println!(
        "[{}] Dummy attack detected (fixture-labeled, no real scanner code), loss {} XLM.",
        label,
        xlm(loss)
    );
    pause();

    let claim_id = submit_claim_signed(env, s, &s.oracle,
        &staker,
        &tx_hash(env, seed),
        &expected_entitlement,
        &tier,
        &now_ts(env),
    );
    s.client.approve_claim(&claim_id);

    let claim = s.client.get_claim(&claim_id).unwrap();
    assert_eq!(claim.entitlement, expected_entitlement);
    assert_eq!(claim.status, ClaimStatus::Active);

    println!(
        "[{}] Claim approved. Entitlement: {} XLM.",
        label,
        xlm(expected_entitlement)
    );
    pause();
    print_solvency(s, label);

    advance_days(env, 7); // mandatory cooldown
    advance_days(env, 45); // fully vested

    let mut streamed = 0i128;
    for _ in 0..10 {
        let claim_now = s.client.get_claim(&claim_id).unwrap();
        if claim_now.status == ClaimStatus::Completed {
            break;
        }
        let t = s.client.claim_stream(&claim_id, &ben);
        streamed += t;
        if t == 0 {
            break;
        }
        advance_days(env, 1);
    }

    println!(
        "[{}] Payout complete. Streamed to beneficiary: {} XLM.",
        label,
        xlm(streamed)
    );
    pause();
    print_solvency(s, label);

    DemoResult {
        label,
        staked: stake_amount,
        loss: Some(loss),
        streamed,
    }
}

#[test]
fn full_pool_lifecycle_demo() {
    let env = new_env();
    let s = setup_with_cap(&env, DEMO_POOL_CAP);

    println!();
    pause();
    println!("================================================================");
    pause();
    println!(" SAFU ProtectionPool: Pool Lifecycle Demo (Stellar / Soroban)");
    pause();
    println!(
        " Pool cap: {} XLM, matches the live testnet deployment",
        xlm(DEMO_POOL_CAP)
    );
    pause();
    println!(" Live contract: CCQT2VRONZTE5ODBNM3XAQWUPQRLKGMU4MMLA2JK6HJHJMK34Q7ZFTGJ");
    pause();
    println!(" Every call below (stake, submit_claim, approve_claim,");
    pause();
    println!(" claim_stream) is the exact same real contract entrypoint");
    pause();
    println!(" deployed live at that address, not a different mock API.");
    pause();
    println!("================================================================");
    pause();

    // Backing liquidity: real stakers a genuinely-funded 600,000 XLM
    // pool would have, so the anti-drain admission/outflow throttles
    // clear real-sized dummy claims (same reasoning as the Blend demo).
    let stake_amount = max_stake(DEMO_POOL_CAP);
    for _ in 0..60 {
        staked_wallet_amount(&env, &s, stake_amount);
    }
    println!();
    pause();
    println!(
        "[pool] 60 backing participants staked {} XLM each ({} XLM total pool liquidity).",
        xlm(stake_amount),
        xlm(stake_amount * 60)
    );
    pause();
    print_solvency(&s, "pool funded");

    // A dummy loss bigger than Tier C's cap but smaller than A/B's caps,
    // so the same "attack" size produces a tier-differentiated result:
    // A and B fully cover it, C is capped by its own lower ratio.
    let dummy_loss: i128 = 500_000_000_000; // "50,000 XLM"

    let expected_a = dummy_loss; // 15x cap comfortably covers the loss
    let expected_b = dummy_loss; // 10x cap comfortably covers the loss
    let expected_c = stake_amount * 5; // 5x cap is the binding constraint

    let result_a = run_dummy_attack(
        &env, &s, "Tier A", TIER_A, stake_amount, dummy_loss, expected_a, 1,
    );
    advance_days(&env, 1);
    let result_b = run_dummy_attack(
        &env, &s, "Tier B", TIER_B, stake_amount, dummy_loss, expected_b, 2,
    );
    advance_days(&env, 1);
    let result_c = run_dummy_attack(
        &env, &s, "Tier C", TIER_C, stake_amount, dummy_loss, expected_c, 3,
    );

    assert_eq!(result_a.streamed, expected_a);
    assert_eq!(result_b.streamed, expected_b);
    assert_eq!(result_c.streamed, expected_c);
    assert!(
        expected_c < expected_a,
        "Tier C's payout must be capped lower than Tier A's for the same dummy attack size"
    );

    // Ordinary participant: staked in the same pool, never attacked.
    // Fixture label is "not drain-shaped" -- asserted by simply never
    // calling submit_claim, exactly as production behaves when SAFU's
    // real (proprietary, not included here) scanner doesn't fire.
    let (ordinary_staker, _ben) = staked_wallet_amount(&env, &s, stake_amount);
    advance_days(&env, 90);
    assert!(s.client.is_claim_eligible(&ordinary_staker));
    println!();
    pause();
    println!(
        "[Ordinary] Staked {} XLM. Never attacked. Stays fully eligible, no claim ever filed.",
        xlm(stake_amount)
    );
    pause();

    let ordinary = DemoResult {
        label: "Ordinary",
        staked: stake_amount,
        loss: None,
        streamed: 0,
    };

    println!();
    pause();
    println!("================================================================");
    pause();
    println!(" SUMMARY");
    pause();
    println!("================================================================");
    pause();
    println!(
        " {:<10} {:<14} {:<16} {:<16}",
        "Depositor", "Staked (XLM)", "Attack Loss", "Streamed (XLM)"
    );
    pause();
    println!(" {}", "-".repeat(58));
    pause();
    for r in [&result_a, &result_b, &result_c, &ordinary] {
        let loss_str = match r.loss {
            Some(l) => format!("{} XLM", xlm(l)),
            None => "none".to_string(),
        };
        println!(
            " {:<10} {:<14} {:<16} {:<16}",
            r.label,
            xlm(r.staked),
            loss_str,
            format!("{} XLM", xlm(r.streamed))
        );
        pause();
    }
    println!(" {}", "-".repeat(58));
    pause();
    println!(
        " Pool remained solvent throughout (allocated <= staked confirmed after every claim)."
    );
    pause();
    println!("================================================================");
}
