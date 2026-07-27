//! Offline replay: reconstructs a `PoolCache` from a recorded pool-metadata
//! snapshot + raw update log (written by `pipeline::run` when `discovery
//! .record_dir` is configured), then runs `research::replay` — the same
//! core loop covered by `research`'s own deterministic tests
//! (IMPLEMENTATION_PLAN.md Phase 4.5) — persisting every opportunity found
//! to SQLite so "does the strategy work" can be answered with
//! `avg(net_profit)` / `count(*) where net_profit > 0`, no dashboard
//! required (IMPLEMENTATION_PLAN.md Phase 4).
//!
//! Usage: `replay <record_dir> <output.sqlite> [scan_every_n_updates]`

use arb_core::PoolMetadata;
use market_data::PoolCache;
use scanner::{PoolGraph, enumerate_cycles};
use solana_sdk::pubkey::Pubkey;
use std::fs::File;
use std::io::BufReader;
use tracing::info;

const AMOUNT_IN: u64 = 1_000_000_000; // 1 SOL, in lamports — same fixed notional as the live pipeline.

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = std::env::args().collect();
    let [_, record_dir, db_path, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: replay <record_dir> <output.sqlite> [scan_every_n_updates]");
    };
    let scan_every_n: usize = rest.first().map(|s| s.parse()).transpose()?.unwrap_or(500);

    let settings = config::Config::builder()
        .add_source(config::File::with_name("config/default"))
        .add_source(config::File::with_name("config/local").required(false))
        .build()?;
    let base_tokens: Vec<Pubkey> = settings
        .get::<Vec<String>>("scanner.base_tokens")?
        .iter()
        .map(|s| s.parse())
        .collect::<Result<_, _>>()?;
    let max_hops: usize = settings.get::<u8>("scanner.max_hops")? as usize;
    // `scanner.min_profit` is a percentage (e.g. 0.15 = 0.15%) — converted to
    // basis points since that's the unit `Opportunity::profit_margin_bps`
    // (and thus `find_opportunities`'s threshold) already uses.
    let min_profit_bps = (settings.get::<f64>("scanner.min_profit")? * 100.0).round() as i32;

    let pools_path = format!("{record_dir}/pools.bin");
    let pools: Vec<PoolMetadata> = bincode::deserialize(&std::fs::read(&pools_path)?)?;
    info!(count = pools.len(), path = %pools_path, "loaded pool metadata snapshot");

    let cache = PoolCache::new();
    for meta in &pools {
        match meta.curve {
            arb_core::Curve::ConstantProduct => {
                cache.register_vaults(meta.id, meta.vault_a, meta.vault_b);
            }
            arb_core::Curve::Whirlpool => {
                // No vault registration needed — the Whirlpool account itself
                // carries full state, applied directly by `apply_update`.
            }
            arb_core::Curve::Dlmm => {
                // active_id/bin_step start at 0 — the log's first notification
                // for this pool (WS's initial-state push) corrects them
                // immediately, same lag the live pipeline tolerated pre-fix.
                cache.register_dlmm_vaults(meta.id, meta.vault_a, meta.vault_b, 0, 0);
            }
        }
        cache.register_metadata(meta.id, meta.clone());
    }

    let graph = PoolGraph::build(&pools);
    let routes = enumerate_cycles(&graph, &base_tokens, max_hops);
    info!(routes = routes.len(), "built static topology from snapshot");

    let db = storage::Db::open(db_path)?;
    let updates_path = format!("{record_dir}/updates.bin");
    let mut reader = BufReader::new(File::open(&updates_path)?);
    let mut opportunities_found = 0usize;

    let applied = research::replay(
        &cache,
        &routes,
        &mut reader,
        AMOUNT_IN,
        min_profit_bps,
        scan_every_n,
        |event| {
            opportunities_found += event.opportunities.len();
            for opp in &event.opportunities {
                if let Err(e) = db.insert_opportunity(opp) {
                    tracing::warn!(error = %e, "failed to persist opportunity");
                }
            }
            let metrics = arb_core::Metrics {
                rpc_latency_ms: 0.0, // offline replay makes no RPC calls
                decode_latency_ms: event.decode_latency_ms,
                cache_latency_ms: event.cache_latency_ms,
                scanner_latency_ms: 0.0, // topology built once above, not rebuilt per scan in replay
                simulation_latency_ms: event.simulation_latency_ms,
                pool_updates_sec: 0.0, // no wall-clock meaning during batch replay
                opportunities_found: event.opportunities.len() as u64,
                simulations_completed: routes.len() as u64,
                false_positive_rate: None,
            };
            info!(
                ?metrics,
                updates_applied = event.updates_applied,
                "replay scan"
            );
        },
    )?;

    info!(
        updates_applied = applied,
        opportunities_found, "replay complete"
    );
    info!(
        total_in_db = db.total_count()?,
        profitable = db.profitable_count()?,
        profitable_and_not_fragile = db.profitable_and_not_fragile_count()?,
        avg_profit_lamports = ?db.avg_profit()?,
        "validation"
    );

    Ok(())
}
