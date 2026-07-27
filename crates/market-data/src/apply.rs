//! Decode-and-cache-update dispatch, by account byte length — the same
//! logic the live pipeline needs for its WS notification loop and the
//! offline replay engine needs for recorded updates. Living in one place
//! means both stay in sync automatically instead of two hand-maintained
//! copies drifting apart.

use arb_core::{CurveState, DecodedAccount, DexAdapter, spl};
use dex_meteora::MeteoraDlmm;
use dex_orca::OrcaWhirlpool;
use solana_sdk::pubkey::Pubkey;
use std::time::Instant;
use tracing::warn;

use crate::PoolCache;

/// Split decode-vs-cache timing for one `apply_update` call — decode (byte
/// parsing) and cache update (a `DashMap` write) are different costs, and
/// IMPLEMENTATION_PLAN.md Phase 4 names them as separate metrics
/// (`decode_latency_ms`, `cache_latency_ms`), not one combined number.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApplyTiming {
    pub decode_latency_ms: f64,
    pub cache_latency_ms: f64,
}

/// Dispatches one raw account update to the cache, by data length:
/// - 165 bytes: SPL Token vault balance (Raydium or Meteora vault).
/// - 653 bytes: Orca Whirlpool (config + live state together).
/// - 904 bytes: Meteora LbPair (config + live state together).
/// - `dex_meteora::BIN_ARRAY_LEN` (10,136) bytes: Meteora `BinArray` — real
///   per-bin liquidity for whichever pool the account's embedded `lb_pair`
///   field names (see `dex_meteora` module docs).
/// - 752 bytes: Raydium AmmInfo — not subscribed to on purpose (see
///   `pipeline` module docs); if one arrives anyway there's nothing to
///   update.
/// - anything else: unrecognized, logged and ignored (zero timing — nothing
///   was decoded or cached).
pub fn apply_update(cache: &PoolCache, pubkey: Pubkey, data: &[u8], slot: u64) -> ApplyTiming {
    match data.len() {
        spl::TOKEN_ACCOUNT_LEN => {
            let decode_start = Instant::now();
            let amount = spl::token_account_amount(data);
            let decode_latency_ms = decode_start.elapsed().as_secs_f64() * 1000.0;

            let cache_start = Instant::now();
            if let Some(amount) = amount {
                cache.update_vault_amount(&pubkey, amount, slot);
            }
            let cache_latency_ms = cache_start.elapsed().as_secs_f64() * 1000.0;
            ApplyTiming {
                decode_latency_ms,
                cache_latency_ms,
            }
        }
        653 => {
            let decode_start = Instant::now();
            let decoded = OrcaWhirlpool.decode(data);
            let decode_latency_ms = decode_start.elapsed().as_secs_f64() * 1000.0;

            let cache_start = Instant::now();
            if let Some(DecodedAccount::ConfigWithLiveState(_meta, curve_state)) = decoded
                && let CurveState::Whirlpool {
                    sqrt_price,
                    liquidity,
                    tick_current_index,
                } = curve_state
            {
                cache.update_clmm_state(pubkey, sqrt_price, liquidity, tick_current_index, slot);
            }
            let cache_latency_ms = cache_start.elapsed().as_secs_f64() * 1000.0;
            ApplyTiming {
                decode_latency_ms,
                cache_latency_ms,
            }
        }
        904 => {
            let decode_start = Instant::now();
            let decoded = MeteoraDlmm.decode(data);
            let decode_latency_ms = decode_start.elapsed().as_secs_f64() * 1000.0;

            let cache_start = Instant::now();
            if let Some(DecodedAccount::ConfigWithLiveState(_meta, curve_state)) = decoded
                && let CurveState::Dlmm {
                    active_id,
                    bin_step,
                    ..
                } = curve_state
            {
                cache.update_dlmm_config(pubkey, active_id, bin_step, slot);
            }
            let cache_latency_ms = cache_start.elapsed().as_secs_f64() * 1000.0;
            ApplyTiming {
                decode_latency_ms,
                cache_latency_ms,
            }
        }
        dex_meteora::BIN_ARRAY_LEN => {
            let decode_start = Instant::now();
            let decoded = MeteoraDlmm.decode(data);
            let decode_latency_ms = decode_start.elapsed().as_secs_f64() * 1000.0;

            let cache_start = Instant::now();
            if let Some(DecodedAccount::MeteoraBinArray {
                lb_pair,
                bin_array_index,
                bins,
            }) = decoded
            {
                cache.update_bin_array(lb_pair, bin_array_index, &bins, slot);
            }
            let cache_latency_ms = cache_start.elapsed().as_secs_f64() * 1000.0;
            ApplyTiming {
                decode_latency_ms,
                cache_latency_ms,
            }
        }
        752 => {
            warn!(%pubkey, "unexpected AmmInfo notification, ignoring");
            ApplyTiming::default()
        }
        other => {
            warn!(%pubkey, len = other, "unrecognized account shape");
            ApplyTiming::default()
        }
    }
}
