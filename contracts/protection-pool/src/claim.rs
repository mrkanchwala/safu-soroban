//! Claims: submit → activate (7d cooldown) → claim_stream (45d vesting,
//! FCFS-per-day daily outflow cap) → completed, or cancel (365-day
//! penalty lock only on false-positive reversal). Also owns the 2-of-2
//! (oracle + coSigner) override request/approval flow.
//!
//! NOT YET IMPLEMENTED — Task 3. This is the highest-stakes module in the
//! contract (solvency invariant, oracle signature verification, the
//! FCFS-per-day outflow-cap streaming logic). Stubbed deliberately rather
//! than rushed: see context/knowledge/smartcontract-soroban.md §1 for the
//! full locked mechanics map this needs to implement, and the eng review
//! (outputs/2026-07-14_plan-eng-review-safu-soroban-protectionpool.md)
//! for the exact V8 algorithm this must port (verified against source,
//! not assumed).
//!
//! Known open question before writing submit_claim: exact byte layout of
//! the oracle's signed verdict payload (which fields, what order, Ed25519
//! over what exactly) isn't pinned down anywhere in this repo yet — ask
//! before implementing signature verification, don't invent a format.
