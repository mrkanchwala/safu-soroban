//! Fuzz target for the solvency invariant + claim state machine — the
//! compensating control for "no Halmos-equivalent exists for Soroban"
//! (context/knowledge/smartcontract-soroban.md §6, research-ops repo).
//!
//! Drives a random sequence of stake/withdraw/submit_claim/claim_stream/
//! cancel_claim/time-advance operations against a small fixed pool of
//! wallets, using `try_*` client methods throughout so ordinary business-
//! logic reverts (out-of-range stake, wrong beneficiary, etc.) are just
//! ignored — only a genuine host-level crash or a failed invariant
//! assertion counts as a finding. The invariant
//! (`total_allocated <= total_staked`, both non-negative) is checked
//! after every single operation, not just at the end, so a violation is
//! caught at the exact operation that caused it.
//!
//! Run: `cargo +nightly fuzz run fuzz_solvency -- -max_total_time=120`

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{Address, BytesN, Env};

use protection_pool::{ProtectionPool, ProtectionPoolClient};

const POOL_CAP: i128 = 100_000_000_000;
const NUM_WALLETS: usize = 4;

#[derive(Arbitrary, Debug)]
enum Op {
    Stake { wallet: u8, amount: i128 },
    Withdraw { wallet: u8 },
    SubmitClaim { wallet: u8, entitlement: i128, tier: u8 },
    ClaimStream { claim_idx: u8 },
    CancelClaim { claim_idx: u8 },
    AdvanceDays { days: u8 },
}

fuzz_target!(|ops: Vec<Op>| {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|li| {
        li.sequence_number = 1_000_000;
        li.timestamp = 1_700_000_000;
    });

    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let co_signer = Address::generate(&env);

    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let token_id = sac.address();
    let token_admin = StellarAssetClient::new(&env, &token_id);

    let contract_id = env.register(ProtectionPool, ());
    let client = ProtectionPoolClient::new(&env, &contract_id);
    client.initialize(&admin, &oracle, &co_signer, &token_id, &POOL_CAP);

    let wallets: Vec<Address> = (0..NUM_WALLETS)
        .map(|_| {
            let a = Address::generate(&env);
            token_admin.mint(&a, &POOL_CAP); // over-fund; contract's own bounds still apply
            a
        })
        .collect();
    let beneficiaries: Vec<Address> = (0..NUM_WALLETS).map(|_| Address::generate(&env)).collect();

    // (claim_id, beneficiary) — paired at submission so ClaimStream calls
    // the beneficiary that actually matches, not a random guess.
    let mut claims: Vec<(BytesN<32>, Address)> = Vec::new();

    for op in ops {
        match op {
            Op::Stake { wallet, amount } => {
                let i = (wallet as usize) % NUM_WALLETS;
                let _ = client.try_stake(&wallets[i], &amount, &beneficiaries[i]);
            }
            Op::Withdraw { wallet } => {
                let i = (wallet as usize) % NUM_WALLETS;
                let _ = client.try_withdraw(&wallets[i], &beneficiaries[i]);
            }
            Op::SubmitClaim {
                wallet,
                entitlement,
                tier,
            } => {
                let i = (wallet as usize) % NUM_WALLETS;
                let seed = claims.len() as u8;
                let hash = BytesN::from_array(&env, &[seed; 32]);
                let now = env.ledger().timestamp();
                let tier_val = 1 + (tier % 3) as u32;
                if let Ok(Ok(id)) = client.try_submit_claim(
                    &oracle,
                    &wallets[i],
                    &hash,
                    &entitlement,
                    &tier_val,
                    &now,
                ) {
                    claims.push((id, beneficiaries[i].clone()));
                }
            }
            Op::ClaimStream { claim_idx } => {
                if !claims.is_empty() {
                    let (id, ben) = &claims[(claim_idx as usize) % claims.len()];
                    let _ = client.try_claim_stream(id, ben);
                }
            }
            Op::CancelClaim { claim_idx } => {
                if !claims.is_empty() {
                    let (id, _) = &claims[(claim_idx as usize) % claims.len()];
                    let _ = client.try_cancel_claim(id);
                }
            }
            Op::AdvanceDays { days } => {
                let d = 1u32 + (days as u32 % 100);
                env.ledger().with_mut(|li| {
                    li.sequence_number += d * 17_280;
                    li.timestamp += (d as u64) * 86_400;
                });
            }
        }

        // The invariant — checked after EVERY op, not just at the end.
        let total_allocated = client.get_total_allocated();
        let total_staked = client.get_total_staked();
        assert!(
            total_allocated <= total_staked,
            "SOLVENCY INVARIANT VIOLATED: total_allocated={} > total_staked={}",
            total_allocated,
            total_staked
        );
        assert!(total_allocated >= 0, "total_allocated went negative: {}", total_allocated);
        assert!(total_staked >= 0, "total_staked went negative: {}", total_staked);
    }
});
