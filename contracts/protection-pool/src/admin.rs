//! Admin: one-time init + oracle/coSigner rotation. Ported from V8's
//! constructor + `setOracle`/`setCoSigner`. The 2-of-2 override
//! request/approval flow lives in claim.rs instead (Task 3) — it operates
//! on Claim/OverrideRequest records and is conceptually part of the
//! claims subsystem, not admin config.

use soroban_sdk::{Address, Env};

use crate::storage;

/// Reinitialization guard (vuln checklist V6) — `has()` check before any
/// state is written, not just before returning early.
/// Soroban Tranche 1 deploy default (user-confirmed 2026-07-14): 600,000
/// XLM, approximating V8's 60 ETH pool cap. 1 XLM = 10_000_000 stroops,
/// so 600,000 XLM = 6_000_000_000_000 stroops. This is NOT hardcoded into
/// contract logic — `pool_cap` is an `initialize` argument and stays
/// admin-adjustable afterward (mirrors V8's mutable `maxPoolSize`, set via
/// a future `set_pool_cap`). Documented here as the intended deploy-time
/// value, not enforced here.
pub const TRANCHE1_DEPLOY_POOL_CAP_STROOPS: i128 = 6_000_000_000_000;

pub fn initialize(
    env: &Env,
    admin: &Address,
    oracle: &Address,
    co_signer: &Address,
    xlm_token: &Address,
    pool_cap: i128,
) {
    if env.storage().instance().has(&crate::storage::DataKey::Admin) {
        panic!("SAFU: already initialized");
    }
    // V8 (S4 audit checklist item): oracle != coSigner enforced at
    // construction and at every setter.
    if oracle == co_signer {
        panic!("SAFU: oracle cannot equal coSigner");
    }
    if pool_cap <= 0 {
        panic!("SAFU: pool cap must be positive");
    }

    admin.require_auth();

    storage::set_admin(env, admin);
    storage::set_oracle(env, oracle);
    storage::set_co_signer(env, co_signer);
    storage::set_xlm_token(env, xlm_token);
    storage::set_pool_cap(env, pool_cap);
    storage::set_total_staked(env, 0);
    storage::set_total_allocated(env, 0);
    storage::bump_instance_ttl(env);
}

/// Mirrors V8's `setPoolSize` — admin-adjustable operational cap.
pub fn set_pool_cap(env: &Env, new_cap: i128) {
    let admin = storage::get_admin(env);
    admin.require_auth();

    if new_cap <= 0 {
        panic!("SAFU: pool cap must be positive");
    }
    // Don't allow shrinking below what's already staked — would make the
    // pool immediately "full" or leave min/max stake bounds inconsistent
    // with live state.
    if new_cap < storage::get_total_staked(env) {
        panic!("SAFU: pool cap below current total staked");
    }
    storage::set_pool_cap(env, new_cap);
    storage::bump_instance_ttl(env);
}

pub fn set_oracle(env: &Env, new_oracle: &Address) {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let co_signer = storage::get_co_signer(env);
    if new_oracle == &co_signer {
        panic!("SAFU: oracle cannot equal coSigner");
    }
    storage::set_oracle(env, new_oracle);
    storage::bump_instance_ttl(env);
}

pub fn set_co_signer(env: &Env, new_co_signer: &Address) {
    let admin = storage::get_admin(env);
    admin.require_auth();

    let oracle = storage::get_oracle(env);
    if new_co_signer == &oracle {
        panic!("SAFU: coSigner cannot equal oracle");
    }
    storage::set_co_signer(env, new_co_signer);
    storage::bump_instance_ttl(env);
}
