//! Wires discovery + market-data + the three `dex-*` decoders together into
//! one running process.
//!
//! **Filter history (see IMPLEMENTATION_PLAN.md Phase 2/4 for the full
//! story)**: verified-token + base-token filtering alone left 33k pools /
//! 85k accounts — the WS connection got reset trying to subscribe to that
//! many. Jupiter's `organicScoreLabel` (high/medium/low, free, same API
//! call) cuts the token set from 4,318 verified to ~349 actually-liquid
//! tokens. This is now the default filter: both tokens verified+liquid, and
//! at least one a configured base token.
//!
//! What gets subscribed, and why, differs per DEX:
//! - Raydium: only the two vault accounts. The config account (`AmmInfo`)
//!   has nothing in it that changes pricing after pool creation.
//! - Orca: only the Whirlpool account itself — it IS the live state
//!   (sqrt_price/liquidity/tick), pricing doesn't need its vaults.
//! - Meteora: the LbPair account (active_id/bin_step change every swap)
//!   AND its two vaults.

use arb_core::events::Event;
use arb_core::{CurveState, PoolMetadata, spl};
use market_data::{PoolCache, apply_update, recorder, subscriptions};
use scanner::{PoolGraph, enumerate_cycles, find_opportunities};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::File;
use std::io::BufWriter;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};

/// Canonical pair key — sorted so `(A, B)` and `(B, A)` collide.
fn pair_key(a: Pubkey, b: Pubkey) -> (Pubkey, Pubkey) {
    if a < b { (a, b) } else { (b, a) }
}

/// Collapses multiple pools of the same pair (Meteora especially creates one
/// per bin-step/fee-tier variant) down to the single deepest one, ranked by
/// real vault balances fetched in one batched `getMultipleAccounts` call —
/// not just the first pool `getProgramAccounts` happened to return.
async fn keep_deepest_by_vaults(
    rpc_url: &str,
    candidates: Vec<PoolMetadata>,
) -> anyhow::Result<Vec<PoolMetadata>> {
    let all_vaults: Vec<Pubkey> = candidates
        .iter()
        .flat_map(|m| [m.vault_a, m.vault_b])
        .collect();
    let balances = arb_core::rpc::get_multiple_accounts_data(rpc_url, &all_vaults).await?;

    let mut best: HashMap<(Pubkey, Pubkey), (PoolMetadata, u64)> = HashMap::new();
    for meta in candidates {
        let a = balances
            .get(&meta.vault_a)
            .and_then(|d| spl::token_account_amount(d))
            .unwrap_or(0);
        let b = balances
            .get(&meta.vault_b)
            .and_then(|d| spl::token_account_amount(d))
            .unwrap_or(0);
        let score = a.saturating_add(b);
        let key = pair_key(meta.token_a, meta.token_b);
        best.entry(key)
            .and_modify(|(cur_meta, cur_score)| {
                if score > *cur_score {
                    *cur_meta = meta.clone();
                    *cur_score = score;
                }
            })
            .or_insert((meta, score));
    }
    Ok(best.into_values().map(|(m, _)| m).collect())
}

/// Same as `keep_deepest_by_vaults`, but also carries `(active_id, bin_step)`
/// through the ranking — Meteora needs both alongside the winning
/// `PoolMetadata` so the cache can be seeded with real values instead of a
/// `0, 0` placeholder.
async fn keep_deepest_dlmm_by_vaults(
    rpc_url: &str,
    candidates: Vec<(PoolMetadata, i32, u16)>,
) -> anyhow::Result<Vec<(PoolMetadata, i32, u16)>> {
    let all_vaults: Vec<Pubkey> = candidates
        .iter()
        .flat_map(|(m, ..)| [m.vault_a, m.vault_b])
        .collect();
    let balances = arb_core::rpc::get_multiple_accounts_data(rpc_url, &all_vaults).await?;

    let mut best: HashMap<(Pubkey, Pubkey), (PoolMetadata, i32, u16, u64)> = HashMap::new();
    for (meta, active_id, bin_step) in candidates {
        let a = balances
            .get(&meta.vault_a)
            .and_then(|d| spl::token_account_amount(d))
            .unwrap_or(0);
        let b = balances
            .get(&meta.vault_b)
            .and_then(|d| spl::token_account_amount(d))
            .unwrap_or(0);
        let score = a.saturating_add(b);
        let key = pair_key(meta.token_a, meta.token_b);
        best.entry(key)
            .and_modify(|(cur_meta, cur_active_id, cur_bin_step, cur_score)| {
                if score > *cur_score {
                    *cur_meta = meta.clone();
                    *cur_active_id = active_id;
                    *cur_bin_step = bin_step;
                    *cur_score = score;
                }
            })
            .or_insert((meta, active_id, bin_step, score));
    }
    Ok(best
        .into_values()
        .map(|(m, active_id, bin_step, _)| (m, active_id, bin_step))
        .collect())
}

/// Same idea as `keep_deepest_by_vaults`, but for Orca: `liquidity` is
/// already decoded from the Whirlpool account itself during discovery, so no
/// extra RPC round-trip is needed to rank same-pair duplicates.
fn keep_deepest_by_liquidity(candidates: Vec<(PoolMetadata, u128)>) -> Vec<PoolMetadata> {
    let mut best: HashMap<(Pubkey, Pubkey), (PoolMetadata, u128)> = HashMap::new();
    for (meta, liquidity) in candidates {
        let key = pair_key(meta.token_a, meta.token_b);
        best.entry(key)
            .and_modify(|(cur_meta, cur_liquidity)| {
                if liquidity > *cur_liquidity {
                    *cur_meta = meta.clone();
                    *cur_liquidity = liquidity;
                }
            })
            .or_insert((meta, liquidity));
    }
    best.into_values().map(|(m, _)| m).collect()
}

pub struct RunConfig {
    pub rpc_url: String,
    pub ws_url: String,
    pub base_tokens: Vec<Pubkey>,
    pub accepted_organic_scores: Vec<String>,
    pub max_subscriptions: usize,
    pub record_dir: Option<String>,
    pub api_addr: String,
    pub db_path: String,
    pub max_hops: usize,
    /// `scanner.min_profit`, already converted from a percentage to basis
    /// points (see `main.rs`) — the unit `Opportunity::profit_margin_bps`
    /// (and thus `find_opportunities`'s threshold) uses.
    pub min_profit_bps: i32,
    /// The built dashboard's static files (`apps/dashboard/dist`), served by
    /// the API on the same origin — set in production (see `Dockerfile`),
    /// left `None` in dev where Vite's own dev server serves the dashboard.
    pub static_dir: Option<String>,
}

pub async fn run(config: RunConfig) -> anyhow::Result<()> {
    let RunConfig {
        rpc_url,
        ws_url,
        base_tokens,
        accepted_organic_scores,
        max_subscriptions,
        record_dir,
        api_addr,
        db_path,
        max_hops,
        min_profit_bps,
        static_dir,
    } = config;
    let http = reqwest::Client::new();
    let base_token_set: HashSet<Pubkey> = base_tokens.iter().copied().collect();

    info!(scores = ?accepted_organic_scores, "fetching Jupiter liquid token set");
    let liquid =
        discovery::jupiter::fetch_liquid_token_mints(&http, &accepted_organic_scores).await?;
    info!(count = liquid.len(), "liquid tokens loaded");

    let keep = |a: &Pubkey, b: &Pubkey| {
        liquid.contains(a)
            && liquid.contains(b)
            && (base_token_set.contains(a) || base_token_set.contains(b))
    };

    let cache = Arc::new(PoolCache::new());
    let mut initial_subscriptions = Vec::new();
    let discovery_start = std::time::Instant::now();

    info!("discovering Raydium pools");
    let raydium_candidates: Vec<PoolMetadata> = dex_raydium::discover_pools(&rpc_url)
        .await?
        .into_iter()
        .filter(|meta| keep(&meta.token_a, &meta.token_b))
        .collect();
    let raydium_pools = keep_deepest_by_vaults(&rpc_url, raydium_candidates).await?;
    for meta in raydium_pools {
        cache.register_vaults(meta.id, meta.vault_a, meta.vault_b);
        initial_subscriptions.push(meta.vault_a);
        initial_subscriptions.push(meta.vault_b);
        cache.register_metadata(meta.id, meta);
    }
    info!(
        kept = cache.pool_ids().len(),
        "Raydium pools after liquidity+deepest-per-pair filter"
    );

    info!("discovering Orca pools");
    let orca_before = cache.pool_ids().len();
    let orca_candidates: Vec<(PoolMetadata, u128)> = dex_orca::discover_pools(&rpc_url)
        .await?
        .into_iter()
        .filter(|(meta, _)| keep(&meta.token_a, &meta.token_b))
        .collect();
    for meta in keep_deepest_by_liquidity(orca_candidates) {
        initial_subscriptions.push(meta.id); // the Whirlpool account itself carries live state
        cache.register_metadata(meta.id, meta);
    }
    info!(
        kept = cache.pool_ids().len() - orca_before,
        "Orca pools after liquidity+deepest-per-pair filter"
    );

    info!("discovering Meteora pools");
    let meteora_before = cache.pool_ids().len();
    let meteora_candidates: Vec<(PoolMetadata, i32, u16)> = dex_meteora::discover_pools(&rpc_url)
        .await?
        .into_iter()
        .filter(|(meta, _)| keep(&meta.token_a, &meta.token_b))
        .map(|(meta, curve_state)| {
            let CurveState::Dlmm {
                active_id,
                bin_step,
                ..
            } = curve_state
            else {
                unreachable!("dex_meteora::discover_pools only yields Dlmm curve state")
            };
            (meta, active_id, bin_step)
        })
        .collect();
    let meteora_pools = keep_deepest_dlmm_by_vaults(&rpc_url, meteora_candidates).await?;
    // Tracks which BinArray indices are already subscribed per pool, so the
    // live loop below can add coverage dynamically as active_id drifts
    // instead of staying pinned to what discovery saw at startup.
    let mut meteora_subscribed_arrays: HashMap<Pubkey, HashSet<i64>> = HashMap::new();
    for (meta, active_id, bin_step) in meteora_pools {
        // Seeded with the real active_id/bin_step read at discovery — quotes
        // are correct from the first scan tick, not just after the first WS
        // notification arrives.
        cache.register_dlmm_vaults(meta.id, meta.vault_a, meta.vault_b, active_id, bin_step);
        initial_subscriptions.push(meta.id);
        initial_subscriptions.push(meta.vault_a);
        initial_subscriptions.push(meta.vault_b);
        // The array covering active_id *as observed right now*, plus one
        // neighbor each side — derived locally, no extra RPC call. Gives the
        // cross-bin walk (dex_meteora::walk_bins) room to cross a depleted
        // active bin in either direction from the first scan tick. Coverage
        // then follows active_id dynamically, see the update loop below.
        let active_index = dex_meteora::bin_array_index_for(active_id);
        let subscribed = meteora_subscribed_arrays.entry(meta.id).or_default();
        for array_index in [active_index - 1, active_index, active_index + 1] {
            subscribed.insert(array_index);
            initial_subscriptions.push(dex_meteora::derive_bin_array_pda(&meta.id, array_index));
        }
        cache.register_metadata(meta.id, meta);
    }
    info!(
        kept = cache.pool_ids().len() - meteora_before,
        "Meteora pools after liquidity+deepest-per-pair filter"
    );

    let rpc_latency_ms = discovery_start.elapsed().as_secs_f64() * 1000.0;
    info!(
        total_pools = cache.pool_ids().len(),
        total_subscriptions = initial_subscriptions.len(),
        rpc_latency_ms,
        "discovery complete"
    );

    if initial_subscriptions.len() > max_subscriptions {
        anyhow::bail!(
            "{} subscriptions exceeds the {} safety ceiling — the liquidity filter isn't tight \
             enough yet for a single WS connection; tighten it before removing this check",
            initial_subscriptions.len(),
            max_subscriptions
        );
    }

    // Replay needs both: the metadata snapshot (id -> vault/side mapping,
    // static for the run) written once here, and the update log (append-only
    // below) — replaying the log alone can't tell which pool/side a vault
    // notification belongs to.
    let mut record_writer: Option<BufWriter<File>> = None;
    if let Some(dir) = &record_dir {
        std::fs::create_dir_all(dir)?;
        let pools: Vec<PoolMetadata> = cache
            .pool_ids()
            .iter()
            .filter_map(|id| cache.get_metadata(id))
            .collect();
        let pools_path = format!("{dir}/pools.bin");
        std::fs::write(&pools_path, bincode::serialize(&pools)?)?;
        info!(path = %pools_path, count = pools.len(), "wrote pool metadata snapshot");
        record_writer = Some(BufWriter::new(File::create(format!("{dir}/updates.bin"))?));
    }

    let (tx, mut rx) = mpsc::channel(1024);
    let (add_tx, add_rx) = mpsc::unbounded_channel();
    tokio::spawn(subscriptions::run(
        ws_url,
        initial_subscriptions,
        tx,
        add_rx,
    ));

    // Phase 5: research/monitoring API, same process as the pipeline (no
    // IPC — see IMPLEMENTATION_PLAN.md). `events_tx` is the one broadcast
    // channel every publish point below sends through; a lagging or absent
    // dashboard client never blocks the pipeline (broadcast::send drops for
    // slow receivers instead of backpressuring the sender).
    let db = Arc::new(Mutex::new(storage::Db::open(&db_path)?));
    let metrics_shared = Arc::new(RwLock::new(arb_core::Metrics::default()));
    let (events_tx, _) = broadcast::channel::<Event>(1024);
    let api_state = api::ApiState {
        pool_cache: Arc::clone(&cache),
        db: Arc::clone(&db),
        metrics: Arc::clone(&metrics_shared),
        events: events_tx.clone(),
        scanner_config: api::ScannerConfig {
            max_hops: max_hops as u8,
            min_profit_bps,
        },
        static_dir: static_dir.map(std::path::PathBuf::from),
    };
    let listener = tokio::net::TcpListener::bind(&api_addr).await?;
    info!(addr = %api_addr, "api server listening");
    let api_router = api::router(api_state);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, api_router).await {
            warn!(error = %e, "api server stopped");
        }
    });

    // Notional amount for candidate quoting. Fixed for now (MVP scope) —
    // Phase 4's research engine is where sizing this properly (per base
    // token, per pool depth) belongs.
    const AMOUNT_IN: u64 = 1_000_000_000; // 1 SOL, in lamports
    const SCAN_INTERVAL_SECS: f64 = 5.0;
    let mut scan_tick = tokio::time::interval(Duration::from_secs_f64(SCAN_INTERVAL_SECS));

    // Accumulated since the last tick — drained into a `Metrics` snapshot
    // each tick, per IMPLEMENTATION_PLAN.md Phase 4 (defined now, even
    // though nothing scrapes them yet; logged via `tracing` in the meantime).
    let mut updates_since_tick: u64 = 0;
    let mut decode_ms_sum = 0.0;
    let mut cache_ms_sum = 0.0;

    loop {
        tokio::select! {
            _ = scan_tick.tick() => {
                let pool_ids = cache.pool_ids();
                let pools: Vec<_> = pool_ids.iter().filter_map(|id| cache.get_metadata(id)).collect();

                let scanner_start = std::time::Instant::now();
                let graph = PoolGraph::build(&pools);
                let routes = enumerate_cycles(&graph, &base_tokens, max_hops);
                let scanner_latency_ms = scanner_start.elapsed().as_secs_f64() * 1000.0;

                let simulation_start = std::time::Instant::now();
                let opportunities = find_opportunities(&cache, &routes, AMOUNT_IN, min_profit_bps);
                let simulation_latency_ms = simulation_start.elapsed().as_secs_f64() * 1000.0;

                let metrics = arb_core::Metrics {
                    rpc_latency_ms: 0.0, // one-time at discovery, logged separately above
                    decode_latency_ms: if updates_since_tick > 0 { decode_ms_sum / updates_since_tick as f64 } else { 0.0 },
                    cache_latency_ms: if updates_since_tick > 0 { cache_ms_sum / updates_since_tick as f64 } else { 0.0 },
                    scanner_latency_ms,
                    simulation_latency_ms,
                    pool_updates_sec: updates_since_tick as f64 / SCAN_INTERVAL_SECS,
                    opportunities_found: opportunities.len() as u64,
                    simulations_completed: routes.len() as u64,
                    false_positive_rate: None,
                };
                info!(?metrics, pools = pools.len(), "scan tick");
                updates_since_tick = 0;
                decode_ms_sum = 0.0;
                cache_ms_sum = 0.0;

                *metrics_shared.write().unwrap() = metrics;
                let _ = events_tx.send(Event::MetricsUpdated(metrics));

                for opp in &opportunities {
                    info!(
                        profit_lamports = opp.expected_profit,
                        profit_margin_bps = opp.profit_margin_bps,
                        max_price_impact_bps = opp.max_price_impact_bps,
                        fragile = opp.is_fragile(),
                        route = ?opp.route.pools,
                        "opportunity found"
                    );
                    if let Err(e) = db.lock().unwrap().insert_opportunity(opp) {
                        warn!(error = %e, "failed to persist opportunity");
                    }
                    let _ = events_tx.send(Event::OpportunityFound(opp.clone()));
                }
            }
            update = rx.recv() => {
                let Some(update) = update else { break };
                if let Some(writer) = record_writer.as_mut() {
                    let raw = recorder::RawUpdate {
                        pool_id: update.account_pubkey.to_bytes(),
                        slot: update.slot,
                        data: update.data.clone(),
                    };
                    if let Err(e) = recorder::append(writer, &raw) {
                        warn!(error = %e, "failed to append to recorder log");
                    }
                }
                let timing = apply_update(&cache, update.account_pubkey, &update.data, update.slot);
                updates_since_tick += 1;
                decode_ms_sum += timing.decode_latency_ms;
                cache_ms_sum += timing.cache_latency_ms;
                let _ = events_tx.send(Event::PoolUpdated {
                    pool_id: update.account_pubkey,
                    slot: update.slot,
                });

                // Dynamic bin-array coverage: an LbPair notification (the
                // account is the pool's own id) means active_id may have
                // moved. If it drifted near or past the edge of what's
                // subscribed, add coverage now instead of leaving the walk
                // stuck with a stale window until the next restart.
                if let Some(meta) = cache.get_metadata(&update.account_pubkey)
                    && meta.dex == arb_core::Dex::Meteora
                    && meta.id == update.account_pubkey
                    && let Some(state) = cache.get(&update.account_pubkey)
                    && let CurveState::Dlmm { active_id, .. } = state.curve_state
                {
                    let needed_index = dex_meteora::bin_array_index_for(active_id);
                    let subscribed = meteora_subscribed_arrays.entry(meta.id).or_default();
                    for array_index in [needed_index - 1, needed_index, needed_index + 1] {
                        if subscribed.insert(array_index) {
                            let pda = dex_meteora::derive_bin_array_pda(&meta.id, array_index);
                            if add_tx.send(pda).is_err() {
                                warn!("failed to send dynamic BinArray subscription request");
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
