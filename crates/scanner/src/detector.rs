//! Fee-aware, pre-slippage opportunity candidates: walks each enumerated
//! route hop by hop, quoting with the same `DexAdapter::quote` a future
//! executor would use, and flags routes where `amount_out > amount_in`.
//! "Pre-slippage" because `quote()` itself is already an approximation for
//! Orca/Meteora (see their module docs) — Phase 4's research engine is
//! where a real slippage/success-probability model belongs, not here.

use crate::routes::RouteCandidate;
use arb_core::{Dex, DexAdapter, Opportunity, PoolMetadata, PoolState, Route};
use dex_meteora::MeteoraDlmm;
use dex_orca::OrcaWhirlpool;
use dex_raydium::RaydiumAmmV4;
use market_data::PoolCache;

fn adapter_for(dex: Dex) -> &'static dyn DexAdapter {
    static RAYDIUM: RaydiumAmmV4 = RaydiumAmmV4;
    static ORCA: OrcaWhirlpool = OrcaWhirlpool;
    static METEORA: MeteoraDlmm = MeteoraDlmm;
    match dex {
        Dex::Raydium => &RAYDIUM,
        Dex::Orca => &ORCA,
        Dex::Meteora => &METEORA,
    }
}

/// Quotes a route through the cache's current state, `None` if any hop's
/// pool isn't in the cache yet or isn't `is_ready()` — an unloaded pool
/// can't be quoted, not "quotes as zero".
fn quote_route(
    cache: &PoolCache,
    route: &RouteCandidate,
    amount_in: u64,
) -> Option<(u64, u64, u16)> {
    let mut amount = amount_in;
    let mut last_slot = 0;
    let mut max_price_impact_bps = 0u16;

    for (i, &pool_id) in route.pools.iter().enumerate() {
        let meta: PoolMetadata = cache.get_metadata(&pool_id)?;
        let state: PoolState = cache.get(&pool_id)?;
        if !state.is_ready() {
            return None;
        }

        let from_token = route.tokens[i];
        let a_to_b = from_token == meta.token_a;

        let quote = adapter_for(meta.dex).quote(&meta, &state, amount, a_to_b);
        if quote.amount_out == 0 {
            return None;
        }
        amount = quote.amount_out;
        last_slot = last_slot.max(state.last_slot);
        max_price_impact_bps = max_price_impact_bps.max(quote.price_impact_bps);
    }

    Some((amount, last_slot, max_price_impact_bps))
}

/// Runs every candidate route through `quote_route` and keeps the ones that
/// clear both bars: strictly profitable, and at or above `min_profit_bps`
/// (the configured `scanner.min_profit` threshold, in basis points) — a
/// route that "works" but only by 1 lamport isn't what that config knob is
/// for.
pub fn find_opportunities(
    cache: &PoolCache,
    routes: &[RouteCandidate],
    amount_in: u64,
    min_profit_bps: i32,
) -> Vec<Opportunity> {
    routes
        .iter()
        .filter_map(|route| {
            let (amount_out, slot, max_price_impact_bps) = quote_route(cache, route, amount_in)?;
            let expected_profit = amount_out as i64 - amount_in as i64;
            if expected_profit <= 0 {
                return None;
            }
            let profit_margin_bps = ((expected_profit * 10_000) / amount_in as i64) as i32;
            if profit_margin_bps < min_profit_bps {
                return None;
            }
            Some(Opportunity {
                route: Route {
                    pools: route.pools.clone(),
                    tokens: route.tokens.clone(),
                },
                expected_profit,
                slot,
                max_price_impact_bps,
                profit_margin_bps,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arb_core::Curve;
    use solana_sdk::pubkey::Pubkey;

    fn cp_meta(id: Pubkey, token_a: Pubkey, token_b: Pubkey, fee_bps: u16) -> PoolMetadata {
        PoolMetadata {
            id,
            dex: Dex::Raydium,
            curve: Curve::ConstantProduct,
            token_a,
            token_b,
            vault_a: Pubkey::new_unique(),
            vault_b: Pubkey::new_unique(),
            fee_bps,
        }
    }

    #[test]
    fn finds_profitable_two_hop_when_pools_are_mispriced() {
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let pool_cheap = Pubkey::new_unique();
        let pool_expensive = Pubkey::new_unique();

        let cache = PoolCache::new();

        // Pool A: 1_000_000 SOL <-> 1_000_000 USDC (price 1:1, tiny fee).
        let meta_cheap = cp_meta(pool_cheap, sol, usdc, 1);
        cache.register_vaults(pool_cheap, meta_cheap.vault_a, meta_cheap.vault_b);
        cache.update_vault_amount(&meta_cheap.vault_a, 1_000_000, 1);
        cache.update_vault_amount(&meta_cheap.vault_b, 1_000_000, 1);
        cache.register_metadata(pool_cheap, meta_cheap);

        // Pool B: 1_000_000 SOL <-> 2_000_000 USDC (SOL is worth 2x more here).
        let meta_expensive = cp_meta(pool_expensive, sol, usdc, 1);
        cache.register_vaults(
            pool_expensive,
            meta_expensive.vault_a,
            meta_expensive.vault_b,
        );
        cache.update_vault_amount(&meta_expensive.vault_a, 1_000_000, 1);
        cache.update_vault_amount(&meta_expensive.vault_b, 2_000_000, 1);
        cache.register_metadata(pool_expensive, meta_expensive);

        // Sell SOL where it's expensive (pool_expensive) first, then buy it
        // back where it's cheap (pool_cheap) — not the other order.
        let route = RouteCandidate {
            pools: vec![pool_expensive, pool_cheap],
            tokens: vec![sol, usdc, sol],
        };

        let opportunities = find_opportunities(&cache, &[route], 10_000, 0);
        assert_eq!(
            opportunities.len(),
            1,
            "buying SOL cheap and selling it expensive should be profitable"
        );
        assert!(opportunities[0].expected_profit > 0);
    }

    #[test]
    fn skips_route_with_unloaded_pool() {
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let pool = Pubkey::new_unique();
        let cache = PoolCache::new();
        cache.register_metadata(pool, cp_meta(pool, sol, usdc, 25));
        // No register_vaults / update_vault_amount — state never becomes ready.

        let route = RouteCandidate {
            pools: vec![pool],
            tokens: vec![sol, usdc],
        };
        assert!(find_opportunities(&cache, &[route], 1_000, 0).is_empty());
    }

    #[test]
    fn skips_unprofitable_route() {
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let pool_a = Pubkey::new_unique();
        let pool_b = Pubkey::new_unique();
        let cache = PoolCache::new();

        for pool in [pool_a, pool_b] {
            let meta = cp_meta(pool, sol, usdc, 25);
            cache.register_vaults(pool, meta.vault_a, meta.vault_b);
            cache.update_vault_amount(&meta.vault_a, 1_000_000, 1);
            cache.update_vault_amount(&meta.vault_b, 1_000_000, 1);
            cache.register_metadata(pool, meta);
        }

        let route = RouteCandidate {
            pools: vec![pool_a, pool_b],
            tokens: vec![sol, usdc, sol],
        };
        // Same price both pools + fees on both legs -> guaranteed loss.
        assert!(find_opportunities(&cache, &[route], 10_000, 0).is_empty());
    }

    #[test]
    fn skips_profitable_route_below_min_profit_bps_threshold() {
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let pool_cheap = Pubkey::new_unique();
        let pool_expensive = Pubkey::new_unique();

        let cache = PoolCache::new();

        let meta_cheap = cp_meta(pool_cheap, sol, usdc, 1);
        cache.register_vaults(pool_cheap, meta_cheap.vault_a, meta_cheap.vault_b);
        cache.update_vault_amount(&meta_cheap.vault_a, 1_000_000, 1);
        cache.update_vault_amount(&meta_cheap.vault_b, 1_000_000, 1);
        cache.register_metadata(pool_cheap, meta_cheap);

        let meta_expensive = cp_meta(pool_expensive, sol, usdc, 1);
        cache.register_vaults(
            pool_expensive,
            meta_expensive.vault_a,
            meta_expensive.vault_b,
        );
        cache.update_vault_amount(&meta_expensive.vault_a, 1_000_000, 1);
        cache.update_vault_amount(&meta_expensive.vault_b, 2_000_000, 1);
        cache.register_metadata(pool_expensive, meta_expensive);

        let route = RouteCandidate {
            pools: vec![pool_expensive, pool_cheap],
            tokens: vec![sol, usdc, sol],
        };

        // Same route as `finds_profitable_two_hop_when_pools_are_mispriced`
        // (profitable at threshold 0) — an unreasonably high min_profit_bps
        // must filter it out instead of only checking `expected_profit > 0`.
        assert!(find_opportunities(&cache, &[route], 10_000, 100_000).is_empty());
    }
}
