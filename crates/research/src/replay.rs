//! Core replay loop: applies a recorded update log to a `PoolCache` and
//! runs the scanner every `scan_every_n` updates — the same logic
//! `apps/cli/src/bin/replay.rs` runs against real recorded mainnet data,
//! extracted here so it's also exercisable by a deterministic test with no
//! live RPC (IMPLEMENTATION_PLAN.md Phase 4.5).

use arb_core::Opportunity;
use market_data::{PoolCache, apply_update, recorder};
use scanner::{RouteCandidate, find_opportunities};
use solana_sdk::pubkey::Pubkey;
use std::io::Read;

/// One scan's worth of results — emitted every `scan_every_n` updates, and
/// once more at the end for whatever didn't land on a boundary.
pub struct ScanEvent {
    pub updates_applied: u64,
    pub opportunities: Vec<Opportunity>,
    pub decode_latency_ms: f64,
    pub cache_latency_ms: f64,
    pub simulation_latency_ms: f64,
}

/// Reads every `RawUpdate` from `reader`, applies it via the same
/// `market_data::apply_update` the live pipeline uses, and calls `on_scan`
/// every `scan_every_n` updates plus once more at the end. Returns the total
/// number of updates applied. Persistence (SQLite) and logging are the
/// caller's concern via `on_scan`, not this function's — keeps this testable
/// with nothing but an in-memory buffer.
pub fn replay(
    cache: &PoolCache,
    routes: &[RouteCandidate],
    reader: &mut impl Read,
    amount_in: u64,
    min_profit_bps: i32,
    scan_every_n: usize,
    mut on_scan: impl FnMut(ScanEvent),
) -> anyhow::Result<u64> {
    let mut applied = 0u64;
    let mut updates_since_scan = 0u64;
    let mut decode_ms_sum = 0.0;
    let mut cache_ms_sum = 0.0;

    let scan = |cache: &PoolCache,
                updates_since_scan: u64,
                decode_ms_sum: f64,
                cache_ms_sum: f64,
                applied: u64,
                on_scan: &mut dyn FnMut(ScanEvent)| {
        let simulation_start = std::time::Instant::now();
        let opportunities = find_opportunities(cache, routes, amount_in, min_profit_bps);
        let simulation_latency_ms = simulation_start.elapsed().as_secs_f64() * 1000.0;
        let denom = updates_since_scan.max(1) as f64;
        on_scan(ScanEvent {
            updates_applied: applied,
            opportunities,
            decode_latency_ms: decode_ms_sum / denom,
            cache_latency_ms: cache_ms_sum / denom,
            simulation_latency_ms,
        });
    };

    while let Some(update) = recorder::read_one(reader)? {
        let pubkey = Pubkey::new_from_array(update.pool_id);
        let timing = apply_update(cache, pubkey, &update.data, update.slot);
        applied += 1;
        updates_since_scan += 1;
        decode_ms_sum += timing.decode_latency_ms;
        cache_ms_sum += timing.cache_latency_ms;

        if updates_since_scan as usize >= scan_every_n {
            scan(
                cache,
                updates_since_scan,
                decode_ms_sum,
                cache_ms_sum,
                applied,
                &mut on_scan,
            );
            updates_since_scan = 0;
            decode_ms_sum = 0.0;
            cache_ms_sum = 0.0;
        }
    }
    // Final scan against whatever didn't land on a scan_every_n boundary —
    // including the all-updates-fit-in-one-window case.
    scan(
        cache,
        updates_since_scan,
        decode_ms_sum,
        cache_ms_sum,
        applied,
        &mut on_scan,
    );

    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arb_core::{Curve, Dex, PoolMetadata};
    use market_data::recorder::{RawUpdate, append};
    use scanner::{PoolGraph, enumerate_cycles};
    use std::io::Cursor;

    fn cp_meta(id: Pubkey, token_a: Pubkey, token_b: Pubkey) -> PoolMetadata {
        PoolMetadata {
            id,
            dex: Dex::Raydium,
            curve: Curve::ConstantProduct,
            token_a,
            token_b,
            vault_a: Pubkey::new_unique(),
            vault_b: Pubkey::new_unique(),
            fee_bps: 1,
        }
    }

    fn vault_update(vault: Pubkey, amount: u64, slot: u64) -> RawUpdate {
        // Mirrors the real SPL Token account layout: mint 0..32, owner
        // 32..64, amount 64..72 LE — see `arb_core::spl`.
        let mut data = vec![0u8; 165];
        data[64..72].copy_from_slice(&amount.to_le_bytes());
        RawUpdate {
            pool_id: vault.to_bytes(),
            slot,
            data,
        }
    }

    /// Same mispriced two-pool setup as `scanner::detector`'s own test, but
    /// driven end-to-end through a recorded log instead of direct cache
    /// calls — this is what "replay through scanner+research" (Phase 4.5)
    /// means: no live RPC, deterministic, asserts the expected opportunity.
    #[test]
    fn replays_recorded_log_into_expected_opportunity() {
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let pool_cheap = Pubkey::new_unique();
        let pool_expensive = Pubkey::new_unique();

        let meta_cheap = cp_meta(pool_cheap, sol, usdc);
        let meta_expensive = cp_meta(pool_expensive, sol, usdc);

        let cache = PoolCache::new();
        cache.register_vaults(pool_cheap, meta_cheap.vault_a, meta_cheap.vault_b);
        cache.register_metadata(pool_cheap, meta_cheap.clone());
        cache.register_vaults(
            pool_expensive,
            meta_expensive.vault_a,
            meta_expensive.vault_b,
        );
        cache.register_metadata(pool_expensive, meta_expensive.clone());

        let pools = vec![meta_cheap.clone(), meta_expensive.clone()];
        let graph = PoolGraph::build(&pools);
        let routes = enumerate_cycles(&graph, &[sol], 2);

        // Record the same reserves as detector.rs's test: pool_cheap is 1:1,
        // pool_expensive prices SOL at 2x USDC.
        let mut log = Vec::new();
        append(&mut log, &vault_update(meta_cheap.vault_a, 1_000_000, 1)).unwrap();
        append(&mut log, &vault_update(meta_cheap.vault_b, 1_000_000, 1)).unwrap();
        append(
            &mut log,
            &vault_update(meta_expensive.vault_a, 1_000_000, 1),
        )
        .unwrap();
        append(
            &mut log,
            &vault_update(meta_expensive.vault_b, 2_000_000, 1),
        )
        .unwrap();

        let mut reader = Cursor::new(log);
        let mut last_opportunities = Vec::new();
        let applied = replay(&cache, &routes, &mut reader, 10_000, 0, 500, |event| {
            last_opportunities = event.opportunities;
        })
        .unwrap();

        assert_eq!(applied, 4);
        assert!(
            last_opportunities.iter().any(|opp| opp.expected_profit > 0),
            "replaying the recorded log should surface the same mispriced-pool \
             opportunity the live scanner would find"
        );
    }

    #[test]
    fn empty_log_applies_nothing_and_finds_no_opportunities() {
        let cache = PoolCache::new();
        let mut reader = Cursor::new(Vec::new());
        let mut scans = 0;
        let applied = replay(&cache, &[], &mut reader, 10_000, 0, 500, |event| {
            scans += 1;
            assert!(event.opportunities.is_empty());
        })
        .unwrap();
        assert_eq!(applied, 0);
        assert_eq!(scans, 1, "the final scan still runs even with zero updates");
    }
}
