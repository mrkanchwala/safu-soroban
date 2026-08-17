//! D2 (Tranche 2) — DeFindex vault yield deployment.
//!
//! Ports V8's liquid-vs-deployed ACCOUNTING (`SAFUPoolV8.sol:851-921`) and
//! deliberately REJECTS V8's deployment POLICY. See types.rs's D2 block for
//! the full reasoning; the short version is that V8 can deploy 100% inline
//! because it tracks a per-staker wstETH tranche and unwinds it inside
//! `withdraw()`, whereas this contract has one pooled position and two
//! withdrawal paths with no cooldown to react inside — `stake::withdraw`
//! (no time lock) and `stake::emergency_exit` (runs while paused).
//!
//! Design locked by `outputs/2026-08-14_plan-eng-review-safu-t2-d2-yield-integration.md`:
//!
//! 1. Deployment is a separate admin call, never inline in `stake()`.
//! 2. Bounded by `deploy_bps` (hard-capped at `MAX_DEPLOY_BPS`) and floored
//!    so it can never deploy XLM already reserved as `total_allocated`.
//! 3. NEVER auto-unwound from any user-facing path. `stake`, `withdraw`,
//!    `emergency_exit` and `claim_stream` do not touch this module's
//!    vault client at all — they only read `liquid_balance`.
//! 4. `deployed_xlm` is held at original deposit value, never marked to
//!    market, so no externally mutable value feeds a payout decision.
//!
//! **The invariant this module must preserve:**
//!
//! ```text
//! liquid_balance + deployed_xlm  >=  total_staked
//! ```
//!
//! V8's form carries `+ totalFailedPayouts` on the right (`:874`); T1
//! deliberately skipped that machinery (§5b of the Soroban KB), so the
//! Soroban form simplifies as above. Note §5b's ORIGINAL justification —
//! "the contract always holds what it owes, so a native XLM transfer
//! essentially cannot fail" — is void from D2 onward, exactly as its
//! `revokedApprovals` sibling was voided by D1. The decision to skip the
//! rescue bucket still stands, but for a different reason: Soroban has no
//! EVM-style partial-failure mode where a transfer fails while the rest of
//! the transaction succeeds. A short balance is caught by an explicit
//! pre-check returning `InsufficientLiquidity` BEFORE any state is written,
//! and the caller simply retries once admin has rebalanced.
//!
//! **Vault interface** verified 2026-08-14 against the deployed contract's
//! own embedded spec over Soroban RPC (`stellar contract info interface
//! --id CCLV4H7WTLJQ7ATLHBBQV2WW3OINF3FOY5XZ7VPHZO7NH3D2ZS4GFSF6 --network
//! testnet`), not from documentation and not from GitHub.

use soroban_sdk::auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation};
use soroban_sdk::{
    contractclient, contractevent, token::TokenClient, vec, Address, Env, IntoVal, Symbol, Val, Vec,
};

use crate::error::PoolError;
use crate::storage;
use crate::types::{DEPLOY_BPS_DENOMINATOR, MAX_DEPLOY_BPS};

// -----------------------------------------------------------------------
// Vault client — minimal by design.
//
// Only the three functions D2 actually needs are declared. The real vault
// exposes ~40 (full SAC token surface, Soroswap library helpers, role
// management, rebalancing); declaring only what we call keeps our exposure
// to a DeFindex interface change as small as possible.
//
// `deposit`'s real return is
//   Result<(Vec<i128>, i128, Option<Vec<Option<AssetInvestmentAllocation>>>), ContractError>
// whose third element is a DeFindex-internal type. Declared as `Val` and
// ignored: shares gained are measured by balance delta instead, which is
// V8's own pattern (`:296-298`, "delta pattern, F10: concurrent-stake
// safe") and avoids importing a foreign type into our contract purely to
// discard it.
// -----------------------------------------------------------------------

// `#[contractclient]` consumes this trait to GENERATE `VaultClient`, which
// is what the rest of the module calls. The trait itself is never invoked
// directly, so rustc's dead-code pass flags it — expected for this macro,
// and suppressed narrowly here rather than by relaxing lints crate-wide.
// D1 landed at zero warnings; this keeps that.
#[allow(dead_code)]
#[contractclient(name = "VaultClient")]
pub trait VaultInterface {
    fn deposit(
        env: Env,
        amounts_desired: Vec<i128>,
        amounts_min: Vec<i128>,
        from: Address,
        invest: bool,
    ) -> Val;

    fn withdraw(
        env: Env,
        withdraw_shares: i128,
        min_amounts_out: Vec<i128>,
        from: Address,
    ) -> Val;

    /// The vault is itself a token; dfToken balance IS the share position.
    fn balance(env: Env, id: Address) -> i128;
}

// -----------------------------------------------------------------------
// Events
// -----------------------------------------------------------------------

#[contractevent]
pub struct Deployed {
    #[topic]
    pub vault: Address,
    pub xlm_amount: i128,
    pub shares_gained: i128,
}

#[contractevent]
pub struct LiquidityProvided {
    #[topic]
    pub vault: Address,
    pub shares_redeemed: i128,
    pub xlm_received: i128,
}

#[contractevent]
pub struct YieldExtracted {
    #[topic]
    pub treasury: Address,
    pub shares_redeemed: i128,
    pub xlm_received: i128,
    pub yield_amount: i128,
}

#[contractevent]
pub struct YieldWithdrawn {
    #[topic]
    pub treasury: Address,
    pub amount: i128,
}

/// Emitted when a redemption returns LESS XLM than the proportional
/// principal that was deployed — i.e. the venue lost money (Blend bad debt
/// or an adverse share-price move).
///
/// V8 has no equivalent. Its `extractYield` handles the same case only by
/// declining to extract (`:912`, `receivedEth > ethEquiv ? ... : 0`), so a
/// principal shortfall is silently absorbed and only surfaces much later as
/// an inability to pay a claim. Emitting a distinct event costs nothing and
/// turns a silent loss into something monitorable off-chain. The full
/// remedy is T3 scope; this is the cheap detection half.
#[contractevent]
pub struct DeploymentShortfall {
    #[topic]
    pub vault: Address,
    pub principal_expected: i128,
    pub xlm_received: i128,
}

// -----------------------------------------------------------------------
// Liquidity helpers — used by stake.rs and claim.rs before every outbound
// transfer.
// -----------------------------------------------------------------------

/// The contract's REAL liquid XLM balance, as opposed to `total_staked`
/// (an accounting figure that includes XLM sitting in the vault).
///
/// This is the contract's first-ever balance read; before D2 the two were
/// identical by construction. Note the same caveat V8 documents at `:853`:
/// XLM sent to the contract outside `stake()` inflates this figure and
/// therefore appears as extractable yield. Harmless — it cannot affect
/// `total_staked` and so cannot affect solvency — but worth knowing.
pub fn liquid_balance(env: &Env) -> i128 {
    let token = TokenClient::new(env, &storage::get_xlm_token(env));
    token.balance(&env.current_contract_address())
}

/// Fail with a typed `InsufficientLiquidity` rather than letting the SAC
/// transfer trap opaquely. Called immediately before every outbound
/// transfer in `stake.rs` and `claim.rs`.
pub fn require_liquidity(env: &Env, amount: i128) -> Result<(), PoolError> {
    if liquid_balance(env) < amount {
        return Err(PoolError::InsufficientLiquidity);
    }
    Ok(())
}

// -----------------------------------------------------------------------
// Views
// -----------------------------------------------------------------------

/// V8 `yieldBalance()` (`:854`): everything the pool controls, minus what
/// it owes stakers. Returns 0 while all principal is deployed and no yield
/// has been realised.
pub fn yield_balance(env: &Env) -> i128 {
    let total = liquid_balance(env) + storage::get_total_deployed_xlm(env);
    let reserved = storage::get_total_staked(env);
    if total > reserved {
        total - reserved
    } else {
        0
    }
}

/// How much of `total_staked` is currently in the vault, in bps.
///
/// Can read ABOVE `deploy_bps` without anything being wrong: the ceiling is
/// checked prospectively at `deploy_to_vault`, so a large withdrawal
/// afterwards lowers `total_staked` while `deployed_xlm` is unchanged. That
/// is drift, not a breach — the solvency invariant still holds — and is
/// resolved by an admin `provide_liquidity`. Exposed so it is observable
/// rather than auto-rebalanced, which would put the vault back into a
/// user-triggered path.
pub fn deployment_ratio_bps(env: &Env) -> i128 {
    let total_staked = storage::get_total_staked(env);
    if total_staked <= 0 {
        return 0;
    }
    storage::get_total_deployed_xlm(env) * DEPLOY_BPS_DENOMINATOR / total_staked
}

// -----------------------------------------------------------------------
// Admin configuration
// -----------------------------------------------------------------------

/// Refuses while shares are still held. Repointing `Vault` mid-position
/// would leave `deployed_shares`/`deployed_xlm` asserting a position at an
/// address the contract no longer knows, stranding it outside contract
/// logic entirely — same class of guard as `set_pool_cap`'s refusal to
/// shrink below `total_staked` (admin.rs).
pub fn set_vault(env: &Env, vault: &Address) -> Result<(), PoolError> {
    let admin = storage::get_admin(env);
    admin.require_auth();

    if storage::get_total_deployed_shares(env) > 0 {
        return Err(PoolError::VaultChangeWhileDeployed);
    }
    storage::set_vault(env, vault);
    storage::bump_instance_ttl(env);
    Ok(())
}

pub fn set_treasury(env: &Env, treasury: &Address) -> Result<(), PoolError> {
    let admin = storage::get_admin(env);
    admin.require_auth();

    storage::set_treasury(env, treasury);
    storage::bump_instance_ttl(env);
    Ok(())
}

/// Hard-capped at `MAX_DEPLOY_BPS` so an admin cannot configure this pool
/// into V8's 100%-deployed posture even deliberately. Lowering it does not
/// force an unwind — it only constrains future `deploy_to_vault` calls.
pub fn set_deploy_bps(env: &Env, bps: i128) -> Result<(), PoolError> {
    let admin = storage::get_admin(env);
    admin.require_auth();

    if bps < 0 {
        return Err(PoolError::AmountNotPositive);
    }
    if bps > MAX_DEPLOY_BPS {
        return Err(PoolError::DeployBpsTooHigh);
    }
    storage::set_deploy_bps(env, bps);
    storage::bump_instance_ttl(env);
    Ok(())
}

// -----------------------------------------------------------------------
// Cross-contract authorization
//
// When this contract calls `vault.deposit(from = self, ..)`, the vault
// re-enters the token to pull the XLM. Those `require_auth` calls resolve to
// THIS contract, and Soroban does not propagate a contract's own authority
// into sub-invocations automatically — it has to be declared up front via
// `authorize_as_current_contract` (Soroban vuln checklist #2).
//
// The tree below is VERIFIED, not assumed. On 2026-08-14 a `deposit` was
// simulated against the LIVE testnet vault over Soroban RPC
// (`simulateTransaction`) and the auth entries it returned were decoded:
//
//     deposit(vault, [amounts_desired, amounts_min, from, invest])
//       └── transfer(token, [from, vault, amount])
//
// That settles both questions D2 had recorded as open:
//
//   1. DeFindex's `deposit` calls `from.require_auth()` ITSELF, so the tree
//      is genuinely two levels deep. A single top-level `transfer` entry
//      does NOT satisfy it — the transfer entry must hang beneath the
//      `deposit` context or it is never reached.
//   2. The pull is `transfer(from, vault, amount)`, NOT `transfer_from`.
//
// `withdraw` was verified the same way, and needed a real position to do it.
// The vault reverts at its own share-balance check before reaching any
// sub-invocation, so a simulating account holding zero shares learns
// nothing; a 1 XLM deposit was submitted from the `admin` testnet identity
// (tx `1d0232a7e50f2a8f0e2a1448da64ede3b5332b814676cc03e7d2dd418483a058`)
// to create one. The tree it then returned is ONE level:
//
//     withdraw(vault, [withdraw_shares, min_amounts_out, from])
//
// with NO sub-invocations. That confirms the mechanism-based reading: the
// XLM leg of a redemption moves the VAULT's own funds to us, and a contract
// self-authorizes movement of its own balance, so no nested entry of ours
// applies. The testnet position was left in place deliberately — it is what
// makes this simulation repeatable at D4.
//
// Note the real vault does NOT mint shares 1:1 (that deposit returned 5,996
// dfTokens for 10,000,000 stroops). Irrelevant to correctness here because
// `deploy_to_vault` measures shares by balance delta rather than assuming a
// rate, but worth knowing before reading the mock's 1:1 rate as realistic.
//
// Both helpers must be called IMMEDIATELY before their vault call:
// `authorize_as_current_contract` applies to the next contract invocation
// only, so an intervening call (e.g. the `balance` read) would consume it.
// -----------------------------------------------------------------------

fn authorize_deposit(
    env: &Env,
    vault_addr: &Address,
    desired: &Vec<i128>,
    mins: &Vec<i128>,
    amount: i128,
) {
    let pool = env.current_contract_address();

    let mut transfer_args: Vec<Val> = Vec::new(env);
    transfer_args.push_back(pool.clone().into_val(env));
    transfer_args.push_back(vault_addr.clone().into_val(env));
    transfer_args.push_back(amount.into_val(env));

    let mut deposit_args: Vec<Val> = Vec::new(env);
    deposit_args.push_back(desired.clone().into_val(env));
    deposit_args.push_back(mins.clone().into_val(env));
    deposit_args.push_back(pool.into_val(env));
    deposit_args.push_back(true.into_val(env));

    let token = storage::get_xlm_token(env);

    // The nested form is what the LIVE vault requires (see the block comment
    // above). The flat form is what the test harness requires, because
    // `mock_all_auths()` satisfies the vault's own `deposit` require_auth
    // without consuming the entry above it — which leaves anything nested
    // beneath that entry unreachable. Declaring both makes this correct in
    // both environments; the one that does not apply is simply never
    // matched, which the host tolerates. Verified empirically in each
    // direction rather than assumed: the flat entry alone fails on the
    // simulated testnet tree, and the nested entry alone fails under
    // `mock_all_auths`.
    env.authorize_as_current_contract(vec![
        env,
        InvokerContractAuthEntry::Contract(SubContractInvocation {
            context: ContractContext {
                contract: vault_addr.clone(),
                fn_name: Symbol::new(env, "deposit"),
                args: deposit_args,
            },
            sub_invocations: vec![
                env,
                InvokerContractAuthEntry::Contract(SubContractInvocation {
                    context: ContractContext {
                        contract: token.clone(),
                        fn_name: Symbol::new(env, "transfer"),
                        args: transfer_args.clone(),
                    },
                    sub_invocations: vec![env],
                }),
            ],
        }),
        InvokerContractAuthEntry::Contract(SubContractInvocation {
            context: ContractContext {
                contract: token,
                fn_name: Symbol::new(env, "transfer"),
                args: transfer_args,
            },
            sub_invocations: vec![env],
        }),
    ]);
}

fn authorize_withdraw(env: &Env, vault_addr: &Address, shares: i128, mins: &Vec<i128>) {
    let mut withdraw_args: Vec<Val> = Vec::new(env);
    withdraw_args.push_back(shares.into_val(env));
    withdraw_args.push_back(mins.clone().into_val(env));
    withdraw_args.push_back(env.current_contract_address().into_val(env));

    env.authorize_as_current_contract(vec![
        env,
        InvokerContractAuthEntry::Contract(SubContractInvocation {
            context: ContractContext {
                contract: vault_addr.clone(),
                fn_name: Symbol::new(env, "withdraw"),
                args: withdraw_args,
            },
            sub_invocations: vec![env],
        }),
    ]);
}

// -----------------------------------------------------------------------
// deploy_to_vault — admin-triggered, bounded, the ONLY inbound path to the
// vault. Deliberately absent from stake().
// -----------------------------------------------------------------------

pub fn deploy_to_vault(
    env: &Env,
    amount: i128,
    min_shares_out: i128,
) -> Result<i128, PoolError> {
    storage::require_not_paused(env)?;
    let admin = storage::get_admin(env);
    admin.require_auth();

    if amount <= 0 {
        return Err(PoolError::AmountNotPositive);
    }
    let vault_addr = storage::get_vault(env).ok_or(PoolError::VaultNotSet)?;

    let liquid = liquid_balance(env);
    if liquid < amount {
        return Err(PoolError::InsufficientLiquidity);
    }

    // Ceiling: never deploy more than deploy_bps of staker principal.
    let deployed_xlm = storage::get_total_deployed_xlm(env);
    let total_staked = storage::get_total_staked(env);
    let ceiling = total_staked * storage::get_deploy_bps(env) / DEPLOY_BPS_DENOMINATOR;
    if deployed_xlm + amount > ceiling {
        return Err(PoolError::DeployExceedsCeiling);
    }

    // Floor: never deploy XLM already reserved for a live claim. This is
    // what guarantees claim_stream's liquidity pre-check can always be
    // satisfied for entitlements already admitted.
    if liquid - amount < storage::get_total_allocated(env) {
        return Err(PoolError::DeployBreachesAllocation);
    }

    let vault = VaultClient::new(env, &vault_addr);
    let shares_before = vault.balance(&env.current_contract_address());

    // Single-asset vault (XLM only), so both vectors are one element.
    // amounts_min == amounts_desired: this bound is on what leaves US, and
    // a partial deposit would desynchronise the delta accounting below.
    let mut desired = Vec::new(env);
    desired.push_back(amount);
    let mut mins = Vec::new(env);
    mins.push_back(amount);

    authorize_deposit(env, &vault_addr, &desired, &mins, amount);

    vault.deposit(
        &desired,
        &mins,
        &env.current_contract_address(),
        &true, // invest immediately rather than sitting as vault idle_funds
    );

    let shares_gained = vault.balance(&env.current_contract_address()) - shares_before;
    if shares_gained < min_shares_out {
        return Err(PoolError::MinSharesNotMet);
    }

    storage::set_total_deployed_shares(env, storage::get_total_deployed_shares(env) + shares_gained);
    storage::set_total_deployed_xlm(env, deployed_xlm + amount);
    storage::bump_instance_ttl(env);

    Deployed {
        vault: vault_addr,
        xlm_amount: amount,
        shares_gained,
    }
    .publish(env);

    Ok(shares_gained)
}

// -----------------------------------------------------------------------
// Redemption — shared core for provide_liquidity and extract_yield.
// -----------------------------------------------------------------------

/// Redeems `shares`, returns `(xlm_received, principal_equivalent)`.
///
/// `principal_equivalent` is the proportional slice of `deployed_xlm` this
/// tranche represents, at ORIGINAL deposit value — V8's `ethEquiv`
/// (`:881`/`:903`). Multiplication before division throughout (Soroban vuln
/// checklist #3: `(a / b) * c` truncates to zero where `(a * c) / b` does
/// not).
fn redeem(
    env: &Env,
    vault_addr: &Address,
    shares: i128,
    min_xlm_out: i128,
) -> Result<(i128, i128), PoolError> {
    if shares <= 0 {
        return Err(PoolError::AmountNotPositive);
    }
    let deployed_shares = storage::get_total_deployed_shares(env);
    if deployed_shares <= 0 {
        return Err(PoolError::NothingDeployed);
    }
    if shares > deployed_shares {
        return Err(PoolError::RedeemExceedsDeployed);
    }

    let deployed_xlm = storage::get_total_deployed_xlm(env);
    let principal_equiv = deployed_xlm * shares / deployed_shares;

    let vault = VaultClient::new(env, vault_addr);
    let liquid_before = liquid_balance(env);

    let mut mins = Vec::new(env);
    mins.push_back(min_xlm_out);

    authorize_withdraw(env, vault_addr, shares, &mins);

    vault.withdraw(&shares, &mins, &env.current_contract_address());

    let xlm_received = liquid_balance(env) - liquid_before;
    if xlm_received < min_xlm_out {
        return Err(PoolError::MinAmountNotMet);
    }

    storage::set_total_deployed_shares(env, deployed_shares - shares);
    storage::set_total_deployed_xlm(env, deployed_xlm - principal_equiv);

    if xlm_received < principal_equiv {
        DeploymentShortfall {
            vault: vault_addr.clone(),
            principal_expected: principal_equiv,
            xlm_received,
        }
        .publish(env);
    }

    Ok((xlm_received, principal_equiv))
}

/// V8 `provideClaimLiquidity` (`:876`) — redeem from the vault so the
/// redeemed XLM sits liquid in the contract, ready to fund payouts and
/// withdrawals. Nothing leaves the pool.
///
/// **Deliberate deviation from V8: no `require_not_paused`.** V8 marks its
/// equivalent `whenNotPaused`. Porting that modifier here would be a real
/// defect: `stake::emergency_exit` is specifically the pause-time escape
/// hatch, so if liquid XLM is short during a pause the operator would have
/// no way to fund the very function stakers are relying on. V8's own
/// `extractYield`/`withdrawYield` already omit the modifier for a related
/// reason (`:895`).
pub fn provide_liquidity(
    env: &Env,
    shares: i128,
    min_xlm_out: i128,
) -> Result<i128, PoolError> {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let vault_addr = storage::get_vault(env).ok_or(PoolError::VaultNotSet)?;
    let (xlm_received, _principal) = redeem(env, &vault_addr, shares, min_xlm_out)?;
    storage::bump_instance_ttl(env);

    LiquidityProvided {
        vault: vault_addr,
        shares_redeemed: shares,
        xlm_received,
    }
    .publish(env);

    Ok(xlm_received)
}

/// V8 `extractYield` (`:898`) — redeem a tranche and send ONLY the excess
/// above proportional principal to treasury. Principal stays in the
/// contract.
///
/// Saturating at zero on a loss is V8's behaviour (`:912`) and is load-
/// bearing here, not cosmetic: this workspace builds with
/// `overflow-checks = true`, so an unguarded `xlm_received - principal`
/// would panic rather than yield a negative. On a shortfall the redemption
/// still completes, nothing goes to treasury, and `DeploymentShortfall`
/// fires from `redeem`.
pub fn extract_yield(env: &Env, shares: i128, min_xlm_out: i128) -> Result<i128, PoolError> {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let vault_addr = storage::get_vault(env).ok_or(PoolError::VaultNotSet)?;
    let treasury = storage::get_treasury(env).ok_or(PoolError::TreasuryNotSet)?;

    let (xlm_received, principal_equiv) = redeem(env, &vault_addr, shares, min_xlm_out)?;

    let yield_amount = if xlm_received > principal_equiv {
        xlm_received - principal_equiv
    } else {
        0
    };

    if yield_amount > 0 {
        storage::set_total_extracted_yield(
            env,
            storage::get_total_extracted_yield(env) + yield_amount,
        );
    }
    storage::bump_instance_ttl(env);

    YieldExtracted {
        treasury: treasury.clone(),
        shares_redeemed: shares,
        xlm_received,
        yield_amount,
    }
    .publish(env);

    if yield_amount > 0 {
        let token = TokenClient::new(env, &storage::get_xlm_token(env));
        token.transfer(&env.current_contract_address(), &treasury, &yield_amount);
    }

    Ok(yield_amount)
}

/// V8 `withdrawYield` (`:860`) — send already-liquid excess to treasury.
/// Ports V8's double gate exactly: the amount must be within the yield
/// balance AND within the real liquid balance. The second check is what
/// stops treasury withdrawals from eating staker principal that happens to
/// be sitting liquid.
pub fn withdraw_yield(env: &Env, amount: i128) -> Result<(), PoolError> {
    let admin = storage::get_admin(env);
    admin.require_auth();

    if amount <= 0 {
        return Err(PoolError::AmountNotPositive);
    }
    let treasury = storage::get_treasury(env).ok_or(PoolError::TreasuryNotSet)?;

    if amount > yield_balance(env) {
        return Err(PoolError::ExceedsYieldBalance);
    }
    if amount > liquid_balance(env) {
        return Err(PoolError::InsufficientLiquidity);
    }

    storage::set_total_extracted_yield(env, storage::get_total_extracted_yield(env) + amount);
    storage::bump_instance_ttl(env);

    YieldWithdrawn {
        treasury: treasury.clone(),
        amount,
    }
    .publish(env);

    let token = TokenClient::new(env, &storage::get_xlm_token(env));
    token.transfer(&env.current_contract_address(), &treasury, &amount);
    Ok(())
}
