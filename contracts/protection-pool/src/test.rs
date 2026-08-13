//! Unit test suite for ProtectionPool (SCF #44 Tranche 1 D2 deliverable:
//! ≥100 tests). Split by mechanic rather than one file — see each
//! submodule for what it covers. `common.rs` holds shared setup, not
//! tests itself.

#![cfg(test)]

mod admin_tests;
mod blend_scenario_tests;
mod claim_tests;
mod common;
/// T2/D1 — on-chain Ed25519 oracle approval verification. Split out rather
/// than folded into `claim_tests` because it tests the signature gate
/// itself, not claim mechanics.
mod d1_signature_tests;
mod mutation_gap_tests;
mod override_tests;
mod pool_demo_tests;
mod profiling_tests;
mod solvency_tests;
mod stake_tests;
