//! Single source of truth for pool state. `PoolMetadata` is looked up by the
//! scanner separately (it changes only on discovery, not on every update) —
//! see IMPLEMENTATION_PLAN.md "Data flow".
//!
//! `PoolState` is per-curve (`CurveState`), not a flat struct of `Option<_>`
//! fields shared across every DEX — see `arb_core::CurveState` for why.

use arb_core::{CurveState, PoolMetadata, PoolState, Side};
use dashmap::DashMap;
use dex_meteora::MAX_BIN_PER_ARRAY;
use solana_sdk::pubkey::Pubkey;

#[derive(Default)]
pub struct PoolCache {
    states: DashMap<Pubkey, PoolState>,
    /// Immutable-ish (per-DEX exceptions noted on `PoolMetadata`) — looked up
    /// by the scanner alongside `states`, set once by whoever first decodes
    /// a pool's config account.
    metadata: DashMap<Pubkey, PoolMetadata>,
    /// vault pubkey -> (pool_id, which side that vault is). Shared across
    /// every curve that has vaults (constant-product, DLMM) — Whirlpool
    /// doesn't register here since its pricing needs no vault balances.
    vault_index: DashMap<Pubkey, (Pubkey, Side)>,
}

impl PoolCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records (or refreshes) a pool's metadata — called by the orchestrator
    /// alongside whichever of `register_vaults`/`update_clmm_state`/
    /// `register_dlmm_vaults` applies to the DEX in question.
    pub fn register_metadata(&self, pool_id: Pubkey, meta: PoolMetadata) {
        self.metadata.insert(pool_id, meta);
    }

    pub fn get_metadata(&self, pool_id: &Pubkey) -> Option<PoolMetadata> {
        self.metadata
            .get(pool_id)
            .map(|entry| entry.value().clone())
    }

    /// All known pool ids — what the scanner iterates to build the graph.
    pub fn pool_ids(&self) -> Vec<Pubkey> {
        self.metadata.iter().map(|entry| *entry.key()).collect()
    }

    /// Called once, when a constant-product pool's config account is
    /// decoded (Raydium-style — reserves live in separate vault accounts).
    pub fn register_vaults(&self, pool_id: Pubkey, vault_a: Pubkey, vault_b: Pubkey) {
        self.vault_index.insert(vault_a, (pool_id, Side::A));
        self.vault_index.insert(vault_b, (pool_id, Side::B));
        self.states.entry(pool_id).or_insert_with(|| PoolState {
            curve_state: CurveState::ConstantProduct {
                reserve_a: None,
                reserve_b: None,
            },
            last_slot: 0,
        });
    }

    /// Looks up which pool/side a vault pubkey belongs to (set by
    /// `register_vaults`/`register_dlmm_vaults`) and patches just that side
    /// of the reserves. A no-op if the pool turned out to be Whirlpool (a
    /// caller bug elsewhere, not something to panic over here).
    pub fn update_vault_amount(&self, vault_pubkey: &Pubkey, amount: u64, slot: u64) {
        let Some(entry) = self.vault_index.get(vault_pubkey) else {
            return; // vault update arrived before its pool's config — drop, next update will match
        };
        let (pool_id, side) = *entry;
        let Some(mut state) = self.states.get_mut(&pool_id) else {
            return;
        };
        match &mut state.curve_state {
            CurveState::ConstantProduct {
                reserve_a,
                reserve_b,
            } => {
                match side {
                    Side::A => *reserve_a = Some(amount),
                    Side::B => *reserve_b = Some(amount),
                }
                state.last_slot = slot;
            }
            CurveState::Dlmm {
                reserve_a,
                reserve_b,
                ..
            } => {
                match side {
                    Side::A => *reserve_a = Some(amount),
                    Side::B => *reserve_b = Some(amount),
                }
                state.last_slot = slot;
            }
            CurveState::Whirlpool { .. } => {}
        }
    }

    /// For concentrated-liquidity DEXs (Orca Whirlpool): the pool's own
    /// config account carries live state too, so every re-decode of it
    /// (not just vault updates) overwrites the cached state — Whirlpool
    /// always gives all three fields together, no partial state possible.
    pub fn update_clmm_state(
        &self,
        pool_id: Pubkey,
        sqrt_price: u128,
        liquidity: u128,
        tick: i32,
        slot: u64,
    ) {
        self.states.insert(
            pool_id,
            PoolState {
                curve_state: CurveState::Whirlpool {
                    sqrt_price,
                    liquidity,
                    tick_current_index: tick,
                },
                last_slot: slot,
            },
        );
    }

    /// Called once, when a Meteora DLMM pool's config account is decoded.
    /// Registers the vault index like `register_vaults`, but the initial
    /// state also carries `active_id`/`bin_step` from that same decode
    /// (unlike constant-product, where the config account carries no live
    /// state at all).
    pub fn register_dlmm_vaults(
        &self,
        pool_id: Pubkey,
        vault_a: Pubkey,
        vault_b: Pubkey,
        active_id: i32,
        bin_step: u16,
    ) {
        self.vault_index.insert(vault_a, (pool_id, Side::A));
        self.vault_index.insert(vault_b, (pool_id, Side::B));
        self.update_dlmm_config(pool_id, active_id, bin_step, 0);
    }

    /// Re-decoding a DLMM config account updates `active_id`/`bin_step`
    /// without disturbing whatever reserves *or* `nearby_bins` are already
    /// cached — unlike Whirlpool, this account's re-decode is NOT a full
    /// state replace. `nearby_bins` is keyed by absolute `bin_id`, so it
    /// stays valid data as `active_id` moves within the window that's
    /// already been decoded; only `update_bin_array` prunes it.
    pub fn update_dlmm_config(&self, pool_id: Pubkey, active_id: i32, bin_step: u16, slot: u64) {
        let mut state = self.states.entry(pool_id).or_insert_with(|| PoolState {
            curve_state: CurveState::Dlmm {
                active_id,
                bin_step,
                reserve_a: None,
                reserve_b: None,
                nearby_bins: Vec::new(),
            },
            last_slot: slot,
        });
        if let CurveState::Dlmm {
            active_id: a,
            bin_step: b,
            ..
        } = &mut state.curve_state
        {
            *a = active_id;
            *b = bin_step;
            state.last_slot = slot;
        }
    }

    /// Merges a decoded `BinArray`'s bins into `nearby_bins`, keyed by
    /// absolute `bin_id` (see `dex_meteora` for the byte layout and
    /// bin-array-index math) — replaces existing entries for bins this array
    /// covers, adds new ones, and prunes anything farther than
    /// `BIN_WINDOW_TO_KEEP` from the pool's *current* `active_id` so the
    /// window doesn't grow unbounded as price drifts across a long-running
    /// process.
    pub fn update_bin_array(
        &self,
        pool_id: Pubkey,
        bin_array_index: i64,
        bins: &[(u64, u64)],
        slot: u64,
    ) {
        const BIN_WINDOW_TO_KEEP: i32 = 300;

        let Some(mut state) = self.states.get_mut(&pool_id) else {
            return; // BinArray arrived before the pool's LbPair config — drop, matches update_vault_amount's convention
        };
        if let CurveState::Dlmm {
            active_id,
            nearby_bins,
            ..
        } = &mut state.curve_state
        {
            let base_bin_id = bin_array_index * MAX_BIN_PER_ARRAY;
            for (i, &(amount_x, amount_y)) in bins.iter().enumerate() {
                let bin_id = (base_bin_id + i as i64) as i32;
                match nearby_bins.iter_mut().find(|(id, ..)| *id == bin_id) {
                    Some(entry) => *entry = (bin_id, amount_x, amount_y),
                    None => nearby_bins.push((bin_id, amount_x, amount_y)),
                }
            }
            let active = *active_id;
            nearby_bins.retain(|(id, ..)| (*id - active).abs() <= BIN_WINDOW_TO_KEEP);
            state.last_slot = slot;
        }
    }

    pub fn get(&self, pool_id: &Pubkey) -> Option<PoolState> {
        self.states.get(pool_id).map(|entry| entry.value().clone())
    }

    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_one_side_at_a_time_and_reports_readiness() {
        let cache = PoolCache::new();
        let pool_id = Pubkey::new_unique();
        let vault_a = Pubkey::new_unique();
        let vault_b = Pubkey::new_unique();
        cache.register_vaults(pool_id, vault_a, vault_b);
        assert!(!cache.get(&pool_id).unwrap().is_ready());

        cache.update_vault_amount(&vault_a, 1_000, 10);
        let state = cache.get(&pool_id).unwrap();
        let CurveState::ConstantProduct {
            reserve_a,
            reserve_b,
        } = state.curve_state
        else {
            panic!("expected ConstantProduct");
        };
        assert_eq!(reserve_a, Some(1_000));
        assert_eq!(
            reserve_b, None,
            "side B untouched until its own vault reports in"
        );
        assert!(!state.is_ready());

        cache.update_vault_amount(&vault_b, 2_000, 11);
        let state = cache.get(&pool_id).unwrap();
        let CurveState::ConstantProduct {
            reserve_a,
            reserve_b,
        } = state.curve_state
        else {
            panic!("expected ConstantProduct");
        };
        assert_eq!(reserve_a, Some(1_000));
        assert_eq!(reserve_b, Some(2_000));
        assert_eq!(state.last_slot, 11);
        assert!(state.is_ready());
    }

    #[test]
    fn ignores_vault_update_before_registration() {
        let cache = PoolCache::new();
        let unknown_vault = Pubkey::new_unique();
        cache.update_vault_amount(&unknown_vault, 500, 1);
        assert!(cache.is_empty());
    }

    #[test]
    fn clmm_state_is_always_fully_populated() {
        let cache = PoolCache::new();
        let pool_id = Pubkey::new_unique();
        cache.update_clmm_state(pool_id, 1u128 << 64, 1_000_000, -100, 5);
        let state = cache.get(&pool_id).unwrap();
        assert!(state.is_ready());
        let CurveState::Whirlpool {
            sqrt_price,
            liquidity,
            tick_current_index,
        } = state.curve_state
        else {
            panic!("expected Whirlpool");
        };
        assert_eq!(sqrt_price, 1u128 << 64);
        assert_eq!(liquidity, 1_000_000);
        assert_eq!(tick_current_index, -100);
    }

    #[test]
    fn dlmm_config_update_preserves_already_known_reserves() {
        let cache = PoolCache::new();
        let pool_id = Pubkey::new_unique();
        let vault_a = Pubkey::new_unique();
        let vault_b = Pubkey::new_unique();
        cache.register_dlmm_vaults(pool_id, vault_a, vault_b, 100, 10);
        cache.update_vault_amount(&vault_a, 5_000, 1);

        // Config re-decodes with a new active_id — reserve_a must survive.
        cache.update_dlmm_config(pool_id, 105, 10, 2);
        let state = cache.get(&pool_id).unwrap();
        let CurveState::Dlmm {
            active_id,
            bin_step,
            reserve_a,
            reserve_b,
            ..
        } = state.curve_state
        else {
            panic!("expected Dlmm");
        };
        assert_eq!(active_id, 105);
        assert_eq!(bin_step, 10);
        assert_eq!(reserve_a, Some(5_000));
        assert_eq!(reserve_b, None);
    }

    #[test]
    fn update_bin_array_merges_bins_from_multiple_arrays_keyed_by_bin_id() {
        let cache = PoolCache::new();
        let pool_id = Pubkey::new_unique();
        let vault_a = Pubkey::new_unique();
        let vault_b = Pubkey::new_unique();
        cache.register_dlmm_vaults(pool_id, vault_a, vault_b, 100, 10);

        let active_index = dex_meteora::bin_array_index_for(100);
        let mut active_array_bins = vec![(0u64, 0u64); MAX_BIN_PER_ARRAY as usize];
        let local = 100 - active_index * MAX_BIN_PER_ARRAY;
        active_array_bins[local as usize] = (1_000, 2_000);
        cache.update_bin_array(pool_id, active_index, &active_array_bins, 1);

        // A neighboring array's bins must be added, not overwrite the first.
        let neighbor_index = active_index + 1;
        let mut neighbor_bins = vec![(0u64, 0u64); MAX_BIN_PER_ARRAY as usize];
        neighbor_bins[0] = (3_000, 4_000);
        cache.update_bin_array(pool_id, neighbor_index, &neighbor_bins, 2);

        let state = cache.get(&pool_id).unwrap();
        let CurveState::Dlmm { nearby_bins, .. } = state.curve_state else {
            panic!("expected Dlmm");
        };
        assert!(nearby_bins.contains(&(100, 1_000, 2_000)));
        let neighbor_bin_id = (neighbor_index * MAX_BIN_PER_ARRAY) as i32;
        assert!(nearby_bins.contains(&(neighbor_bin_id, 3_000, 4_000)));
    }

    #[test]
    fn update_bin_array_prunes_bins_far_from_current_active_id() {
        let cache = PoolCache::new();
        let pool_id = Pubkey::new_unique();
        let vault_a = Pubkey::new_unique();
        let vault_b = Pubkey::new_unique();
        cache.register_dlmm_vaults(pool_id, vault_a, vault_b, 100, 10);

        let active_index = dex_meteora::bin_array_index_for(100);
        let mut bins = vec![(0u64, 0u64); MAX_BIN_PER_ARRAY as usize];
        let local = 100 - active_index * MAX_BIN_PER_ARRAY;
        bins[local as usize] = (1_000, 2_000);
        cache.update_bin_array(pool_id, active_index, &bins, 1);

        // Price moves far away (beyond the 300-bin retention window), then
        // any subsequent update_bin_array call prunes the now-stale entry.
        cache.update_dlmm_config(pool_id, 100_000, 10, 2);
        let far_index = dex_meteora::bin_array_index_for(100_000);
        let far_bins = vec![(0u64, 0u64); MAX_BIN_PER_ARRAY as usize];
        cache.update_bin_array(pool_id, far_index, &far_bins, 3);

        let state = cache.get(&pool_id).unwrap();
        let CurveState::Dlmm { nearby_bins, .. } = state.curve_state else {
            panic!("expected Dlmm");
        };
        assert!(
            !nearby_bins.iter().any(|(id, ..)| *id == 100),
            "bin 100 is far outside the window around active_id=100_000 and must be pruned"
        );
    }
}
