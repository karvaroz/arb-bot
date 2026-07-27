//! Offline analysis: does `fragile` (Opportunity::is_fragile — quoted price
//! impact already consuming the theoretical edge) cluster on specific DEXs?
//! Cross-references a persisted `storage::Db` against the `PoolMetadata`
//! snapshot from the same run to classify each opportunity's route by which
//! DEX(es) it touches, then reports fragile-rate and average price impact
//! per DEX composition — grounded entirely in real recorded data, no
//! assumptions (IMPLEMENTATION_PLAN.md Phase 4 validation).
//!
//! Usage: `analyze_fragility <pools.bin> <opportunities.sqlite>`

use arb_core::{Dex, PoolMetadata};
use solana_sdk::pubkey::Pubkey;
use std::collections::{BTreeSet, HashMap};

fn dex_name(dex: Dex) -> &'static str {
    match dex {
        Dex::Raydium => "Raydium",
        Dex::Orca => "Orca",
        Dex::Meteora => "Meteora",
    }
}

#[derive(Default)]
struct Stats {
    count: u64,
    fragile_count: u64,
    sum_max_price_impact_bps: u64,
    sum_profit_margin_bps: i64,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, pools_path, db_path] = args.as_slice() else {
        anyhow::bail!("usage: analyze_fragility <pools.bin> <opportunities.sqlite>");
    };

    let pools: Vec<PoolMetadata> = bincode::deserialize(&std::fs::read(pools_path)?)?;
    let dex_by_pool: HashMap<Pubkey, Dex> = pools.iter().map(|m| (m.id, m.dex)).collect();
    println!("loaded {} pools from {}", pools.len(), pools_path);

    let db = storage::Db::open(db_path)?;
    let rows = db.all_rows()?;
    println!("loaded {} opportunity rows from {}\n", rows.len(), db_path);

    let mut unresolved_pools = 0u64;
    let mut by_composition: HashMap<String, Stats> = HashMap::new();
    let mut by_single_dex_presence: HashMap<&'static str, Stats> = HashMap::new();

    for row in &rows {
        let mut dexes: BTreeSet<&'static str> = BTreeSet::new();
        let mut resolved = true;
        for pool_id in &row.route_pools {
            match dex_by_pool.get(pool_id) {
                Some(dex) => {
                    dexes.insert(dex_name(*dex));
                }
                None => {
                    resolved = false;
                    unresolved_pools += 1;
                }
            }
        }
        if !resolved {
            continue; // pool not in this snapshot — shouldn't happen, skip rather than misclassify
        }

        let composition = dexes.iter().copied().collect::<Vec<_>>().join("+");
        let entry = by_composition.entry(composition).or_default();
        entry.count += 1;
        entry.fragile_count += row.fragile as u64;
        entry.sum_max_price_impact_bps += row.max_price_impact_bps as u64;
        entry.sum_profit_margin_bps += row.profit_margin_bps as i64;

        for dex_str in &dexes {
            let entry = by_single_dex_presence.entry(dex_str).or_default();
            entry.count += 1;
            entry.fragile_count += row.fragile as u64;
            entry.sum_max_price_impact_bps += row.max_price_impact_bps as u64;
            entry.sum_profit_margin_bps += row.profit_margin_bps as i64;
        }
    }

    if unresolved_pools > 0 {
        println!(
            "WARNING: {unresolved_pools} pool references not found in the snapshot — those rows were skipped, not misclassified\n"
        );
    }

    println!("=== By exact DEX composition of the route ===");
    println!(
        "{:<20} {:>8} {:>10} {:>12} {:>14}",
        "composition", "count", "fragile%", "avg_impact_bps", "avg_margin_bps"
    );
    let mut compositions: Vec<_> = by_composition.into_iter().collect();
    compositions.sort_by_key(|c| std::cmp::Reverse(c.1.count));
    for (composition, s) in &compositions {
        let fragile_pct = 100.0 * s.fragile_count as f64 / s.count as f64;
        let avg_impact = s.sum_max_price_impact_bps as f64 / s.count as f64;
        let avg_margin = s.sum_profit_margin_bps as f64 / s.count as f64;
        println!(
            "{:<20} {:>8} {:>9.1}% {:>12.0} {:>14.1}",
            composition, s.count, fragile_pct, avg_impact, avg_margin
        );
    }

    println!(
        "\n=== By DEX presence anywhere in the route (routes can touch >1 DEX, so this double-counts) ==="
    );
    println!(
        "{:<20} {:>8} {:>10} {:>12} {:>14}",
        "dex", "count", "fragile%", "avg_impact_bps", "avg_margin_bps"
    );
    let mut dexes: Vec<_> = by_single_dex_presence.into_iter().collect();
    dexes.sort_by_key(|d| std::cmp::Reverse(d.1.count));
    for (dex, s) in &dexes {
        let fragile_pct = 100.0 * s.fragile_count as f64 / s.count as f64;
        let avg_impact = s.sum_max_price_impact_bps as f64 / s.count as f64;
        let avg_margin = s.sum_profit_margin_bps as f64 / s.count as f64;
        println!(
            "{:<20} {:>8} {:>9.1}% {:>12.0} {:>14.1}",
            dex, s.count, fragile_pct, avg_impact, avg_margin
        );
    }

    Ok(())
}
