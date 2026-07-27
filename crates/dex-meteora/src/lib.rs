//! Meteora DLMM (bin-based liquidity) decoder + pricing.
//!
//! Program ID: LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo (verified
//! 2026-07-27).
//!
//! `LbPair` layout verified against the official Anchor IDL
//! (https://github.com/MeteoraAg/dlmm-sdk/blob/main/idls/dlmm.json,
//! `"serialization": "bytemuck", "repr": {"kind": "c"}`) — every alignment
//! gap the compiler would otherwise insert is already an explicit
//! `_padding*` field in the IDL, so offsets are a flat cumulative sum of
//! field sizes in declaration order, same as a packed struct. 8-byte Anchor
//! discriminator prefix, total size 904 bytes.
//!
//! Fee/price formulas verified against the SDK's own source
//! (`commons/src/extensions/lb_pair.rs::get_base_fee`,
//! `commons/src/math/price_math.rs::get_price_from_id`):
//!   base_fee_rate = base_factor * bin_step * 10 * 10^base_fee_power_factor
//!   (relative to FEE_PRECISION = 1_000_000_000)
//!   price = (1 + bin_step / 10_000) ^ active_id   (Q64.64 in the real program)
//!
//! Like Raydium/Orca: reserves live in `reserve_x`/`reserve_y`, plain SPL
//! Token vault accounts — `arb_core::spl` is reused as-is.
//!
//! ## Bin depth (`BinArray`) — fixed 2026-07-27
//!
//! Real DLMM liquidity is bin-based: the exact amount available at
//! `active_id` lives in a separate `BinArray` account, holding 70 `Bin`
//! entries (`MAX_BIN_PER_ARRAY`, verified against
//! `commons/src/constants.rs` in the SDK source — the IDL's own doc comment
//! on `BinArray` claims 600 bins per array, which is stale/wrong; the type
//! definition's `[Bin; 70]` and the SDK's actual constant agree on 70, and
//! the type definition is what's really serialized on-chain).
//!
//! `Bin` layout (also `bytemuck`/`repr(C)`, verified against the IDL's
//! `types` section): `amount_x: u64` at offset 0, `amount_y: u64` at offset
//! 8 — the only two fields this crate reads; the other ~120 bytes per bin
//! (price, fee accounting, limit-order bookkeeping) aren't needed for
//! quoting and aren't decoded. `Bin` is 144 bytes total.
//!
//! `BinArray` layout: 8-byte Anchor discriminator, `index: i64` (8),
//! `version: u8` (1), `_padding_1: [u8; 7]` (7), `lb_pair: pubkey` (32),
//! then `bins: [Bin; 70]` (70 * 144 = 10,080) — total account size 10,136
//! bytes, bins starting at byte offset 56.
//!
//! Which `BinArray` covers a given `active_id`, and the address to
//! subscribe to, are both verified against the SDK source, not guessed:
//! `bin_array_index = active_id.div_floor(70)` (exact port of
//! `commons/src/extensions/bin_array.rs::bin_id_to_bin_array_index`, which
//! truncates-then-adjusts for negative `active_id` rather than using
//! Euclidean division directly — same result, ported literally to avoid any
//! off-by-one risk), and the account address is a PDA derived from
//! `[b"bin_array", lb_pair, bin_array_index.to_le_bytes()]`
//! (`commons/src/pda.rs::derive_bin_array_pda`, seed constant from
//! `commons/src/seeds.rs::BIN_ARRAY`) — meaning it can be computed locally,
//! no extra discovery RPC call needed.
//!
//! ## Cross-bin swap walk — fixed 2026-07-27
//!
//! `quote()` walks bins starting at `active_id`, crossing into adjacent bins
//! once one depletes, same as a real DLMM swap. The crossing *direction* is
//! not guessed: selling more of a token can only push that token's price
//! down — a no-free-lunch invariant every correct AMM already encodes (it's
//! the same reasoning behind Raydium's constant-product curve and Orca's
//! tick-crossing), not an implementation detail specific to this program.
//! Concretely: selling token_a (`a_to_b`) walks toward *decreasing*
//! `bin_id` (price of `a` in terms of `b` only gets worse for the seller);
//! selling token_b walks toward *increasing* `bin_id`. Each bin is priced at
//! its own real rate (`(1 + bin_step/10_000)^bin_id`), not one flat number
//! for the whole trade.
//!
//! This was verified empirically, not just reasoned about: a live
//! cross-check against Jupiter's quote API
//! (`apps/cli/src/bin/verify_pricing.rs`) caught the *previous* single-bin
//! version underquoting a real trade by -99.99% (active bin was thin, real
//! liquidity sat one bin over) — the walk closes that gap. Still bounded by
//! how wide a bin window has actually been decoded (`nearby_bins`, capped at
//! ±300 bins from `active_id` — see `market_data::cache`): running out of
//! known bins stops the walk and reports whatever filled, never guesses
//! liquidity beyond what's been observed.
//!
//! Dynamic re-subscription: the pipeline tracks which bin-array indices are
//! subscribed per pool and adds more via `add_tx` when `active_id` drifts
//! near the edge of known coverage — see `apps/cli/src/pipeline.rs`.

use arb_core::{CurveState, DecodedAccount, DexAdapter, PoolMetadata, PoolState, Quote, spl};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

pub const PROGRAM_ID: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";
const LB_PAIR_LEN: usize = 904;

/// Bins per `BinArray` — verified against `commons/src/constants.rs::MAX_BIN_PER_ARRAY`
/// in the DLMM SDK source (the IDL's own doc comment claiming 600 is stale/wrong).
pub const MAX_BIN_PER_ARRAY: i64 = 70;
/// One `Bin` entry's size in bytes (only `amount_x`/`amount_y` are read; see
/// module docs for the full field list this crate doesn't need).
const BIN_LEN: usize = 144;
/// `BinArray` account size: 8 (Anchor discriminator) + 8 (`index: i64`) + 1
/// (`version: u8`) + 7 (`_padding_1`) + 32 (`lb_pair: pubkey`) + 70 * 144
/// (`bins`).
pub const BIN_ARRAY_LEN: usize = 8 + 8 + 1 + 7 + 32 + (MAX_BIN_PER_ARRAY as usize) * BIN_LEN;
const OFFSET_BIN_ARRAY_INDEX: usize = 8;
const OFFSET_BIN_ARRAY_LB_PAIR: usize = 8 + 8 + 1 + 7;
const OFFSET_BIN_ARRAY_BINS: usize = OFFSET_BIN_ARRAY_LB_PAIR + 32;

// Byte offsets into the LbPair account (8-byte Anchor discriminator, then
// bytemuck/repr(C) fields — every alignment gap is an explicit IDL field):
const OFFSET_BASE_FACTOR: usize = 8; // StaticParameters.base_factor, u16
const OFFSET_BASE_FEE_POWER_FACTOR: usize = 8 + 26; // StaticParameters.base_fee_power_factor, u8, offset 34
const OFFSET_ACTIVE_ID: usize = 76;
const OFFSET_BIN_STEP: usize = 80;
const OFFSET_TOKEN_X_MINT: usize = 88;
const OFFSET_TOKEN_Y_MINT: usize = 120;
const OFFSET_RESERVE_X: usize = 152;
const OFFSET_RESERVE_Y: usize = 184;

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_i32(data: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_i64(data: &[u8], offset: usize) -> Option<i64> {
    Some(i64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn read_u64(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn read_pubkey(data: &[u8], offset: usize) -> Option<Pubkey> {
    let bytes: [u8; 32] = data.get(offset..offset + 32)?.try_into().ok()?;
    Some(Pubkey::from(bytes))
}

/// Exact port of `commons/src/extensions/bin_array.rs::bin_id_to_bin_array_index`
/// — truncating division adjusted for negative `bin_id`, not
/// `div_euclid`/`.floor()`, to match the real program's arithmetic exactly
/// rather than an equivalent-looking reimplementation.
pub fn bin_array_index_for(active_id: i32) -> i64 {
    let divisor = MAX_BIN_PER_ARRAY as i32;
    let idx = active_id / divisor;
    let rem = active_id % divisor;
    let idx = if active_id.is_negative() && rem != 0 {
        idx - 1
    } else {
        idx
    };
    idx as i64
}

/// PDA derivation verified against `commons/src/pda.rs::derive_bin_array_pda`
/// and `commons/src/seeds.rs::BIN_ARRAY` — computed locally, no discovery
/// RPC call needed once `lb_pair` and `active_id` are known.
pub fn derive_bin_array_pda(lb_pair: &Pubkey, bin_array_index: i64) -> Pubkey {
    let program_id = Pubkey::from_str(PROGRAM_ID).expect("PROGRAM_ID is a valid pubkey");
    let (pda, _bump) = Pubkey::find_program_address(
        &[
            b"bin_array",
            lb_pair.as_ref(),
            &bin_array_index.to_le_bytes(),
        ],
        &program_id,
    );
    pda
}

pub struct MeteoraDlmm;

impl DexAdapter for MeteoraDlmm {
    fn decode(&self, account_data: &[u8]) -> Option<DecodedAccount> {
        match account_data.len() {
            LB_PAIR_LEN => decode_lb_pair(account_data)
                .map(|(meta, curve_state)| DecodedAccount::ConfigWithLiveState(meta, curve_state)),
            BIN_ARRAY_LEN => decode_bin_array(account_data),
            spl::TOKEN_ACCOUNT_LEN => {
                spl::token_account_amount(account_data).map(DecodedAccount::VaultAmount)
            }
            _ => None,
        }
    }

    // See module docs for the full reasoning. Real cross-bin walk when a
    // window of `nearby_bins` has been decoded; falls back to the coarser
    // reserve-based discount (fixed 2026-07-27, see git history) when no
    // bin data has arrived yet.
    fn quote(&self, meta: &PoolMetadata, state: &PoolState, amount_in: u64, a_to_b: bool) -> Quote {
        let CurveState::Dlmm {
            active_id,
            bin_step,
            reserve_a,
            reserve_b,
            ref nearby_bins,
        } = state.curve_state
        else {
            return Quote {
                amount_out: 0,
                fee: 0,
                price_impact_bps: 10_000,
            };
        };
        let (Some(reserve_a), Some(reserve_b)) = (reserve_a, reserve_b) else {
            return Quote {
                amount_out: 0,
                fee: 0,
                price_impact_bps: 10_000,
            };
        };

        let fee = (amount_in as u128 * meta.fee_bps as u128) / 10_000;
        let amount_in_after_fee = (amount_in - fee as u64) as f64;
        let spot_price = (1.0 + bin_step as f64 / 10_000.0).powi(active_id); // token_b per token_a

        let (amount_out, price_impact_bps) = if nearby_bins.is_empty() {
            let amount_out_at_flat_price = if a_to_b {
                amount_in_after_fee * spot_price
            } else {
                amount_in_after_fee / spot_price
            };
            let reserve_in = if a_to_b { reserve_a } else { reserve_b };
            let impact_bps = if reserve_in == 0 {
                10_000
            } else {
                ((amount_in as f64 / (reserve_in as f64 + amount_in as f64)) * 10_000.0)
                    .min(10_000.0) as u16
            };
            let amount_out = amount_out_at_flat_price * (1.0 - impact_bps as f64 / 10_000.0);
            (amount_out, impact_bps)
        } else {
            walk_bins(
                bin_step,
                active_id,
                nearby_bins,
                amount_in_after_fee,
                a_to_b,
            )
        };

        Quote {
            amount_out: amount_out.max(0.0) as u64,
            fee: fee as u64,
            price_impact_bps,
        }
    }
}

/// Real cross-bin swap simulation — see module docs for why the crossing
/// direction is a determined economic fact, not a guess. Stops when
/// `amount_in` is fully spent or `nearby_bins` runs out of known data,
/// whichever comes first; never assumes liquidity beyond what's actually
/// been decoded.
fn walk_bins(
    bin_step: u16,
    active_id: i32,
    nearby_bins: &[(i32, u64, u64)],
    amount_in_after_fee: f64,
    a_to_b: bool,
) -> (f64, u16) {
    let bins: std::collections::HashMap<i32, (u64, u64)> =
        nearby_bins.iter().map(|&(id, x, y)| (id, (x, y))).collect();

    // Selling token_a walks toward decreasing bin_id (worse price for the
    // seller); selling token_b walks toward increasing bin_id. See module docs.
    let step: i32 = if a_to_b { -1 } else { 1 };
    let mut bin_id = active_id;
    let mut remaining_in = amount_in_after_fee;
    let mut filled_out = 0.0_f64;

    // Backstop, not the real limiting factor — that's `nearby_bins` running out.
    const MAX_STEPS: u32 = 1000;
    for _ in 0..MAX_STEPS {
        if remaining_in <= 0.0 {
            break;
        }
        let Some(&(amount_x, amount_y)) = bins.get(&bin_id) else {
            break; // outside the known window — stop, don't guess further
        };
        let available_out = if a_to_b { amount_y } else { amount_x } as f64;
        if available_out <= 0.0 {
            bin_id += step;
            continue;
        }
        let bin_price = (1.0 + bin_step as f64 / 10_000.0).powi(bin_id);
        let input_to_drain_bin = if a_to_b {
            available_out / bin_price
        } else {
            available_out * bin_price
        };
        if input_to_drain_bin >= remaining_in {
            let out = if a_to_b {
                remaining_in * bin_price
            } else {
                remaining_in / bin_price
            };
            filled_out += out;
            remaining_in = 0.0;
        } else {
            filled_out += available_out;
            remaining_in -= input_to_drain_bin;
            bin_id += step;
        }
    }

    // Rate over the FULL requested input, not just whatever filled — running
    // out of known bins before `amount_in` is spent must show up as impact
    // too, not just genuine cross-bin price degradation. Otherwise a trade
    // that fills 0.05% of what was asked, at a fine per-bin price, would
    // misreport as "no impact."
    let spot_price = (1.0 + bin_step as f64 / 10_000.0).powi(active_id);
    let spot_rate = if a_to_b { spot_price } else { 1.0 / spot_price };
    let naive_rate = if amount_in_after_fee > 0.0 {
        filled_out / amount_in_after_fee
    } else {
        0.0
    };
    let impact_bps = if spot_rate <= 0.0 {
        10_000
    } else {
        (((spot_rate - naive_rate) / spot_rate) * 10_000.0).clamp(0.0, 10_000.0) as u16
    };

    (filled_out, impact_bps)
}

fn decode_lb_pair(data: &[u8]) -> Option<(PoolMetadata, CurveState)> {
    let base_factor = read_u16(data, OFFSET_BASE_FACTOR)?;
    let base_fee_power_factor = *data.get(OFFSET_BASE_FEE_POWER_FACTOR)?;
    let active_id = read_i32(data, OFFSET_ACTIVE_ID)?;
    let bin_step = read_u16(data, OFFSET_BIN_STEP)?;

    // base_fee_rate = base_factor * bin_step * 10 * 10^base_fee_power_factor,
    // relative to FEE_PRECISION (1e9); fee_bps = base_fee_rate / 100_000.
    let base_fee_rate = (base_factor as u128)
        .checked_mul(bin_step as u128)?
        .checked_mul(10)?
        .checked_mul(10u128.checked_pow(base_fee_power_factor as u32)?)?;
    let fee_bps = (base_fee_rate / 100_000).min(u16::MAX as u128) as u16;

    let token_x_mint = read_pubkey(data, OFFSET_TOKEN_X_MINT)?;
    let token_y_mint = read_pubkey(data, OFFSET_TOKEN_Y_MINT)?;
    let reserve_x = read_pubkey(data, OFFSET_RESERVE_X)?;
    let reserve_y = read_pubkey(data, OFFSET_RESERVE_Y)?;

    let meta = PoolMetadata {
        id: Pubkey::default(), // filled in by the caller
        dex: arb_core::Dex::Meteora,
        curve: arb_core::Curve::Dlmm,
        token_a: token_x_mint,
        token_b: token_y_mint,
        vault_a: reserve_x,
        vault_b: reserve_y,
        fee_bps,
    };
    let curve_state = CurveState::Dlmm {
        active_id,
        bin_step,
        reserve_a: None,
        reserve_b: None,
        nearby_bins: Vec::new(),
    };
    Some((meta, curve_state))
}

/// Decodes a `BinArray` account into `DecodedAccount::MeteoraBinArray` — the
/// `lb_pair` field embedded in the account itself is the pool id, so no
/// separate index mapping a `BinArray` account's own pubkey back to its pool
/// is needed (unlike vault accounts, which need the caller's bookkeeping).
fn decode_bin_array(data: &[u8]) -> Option<DecodedAccount> {
    let bin_array_index = read_i64(data, OFFSET_BIN_ARRAY_INDEX)?;
    let lb_pair = read_pubkey(data, OFFSET_BIN_ARRAY_LB_PAIR)?;
    let mut bins = Vec::with_capacity(MAX_BIN_PER_ARRAY as usize);
    for i in 0..MAX_BIN_PER_ARRAY as usize {
        let bin_offset = OFFSET_BIN_ARRAY_BINS + i * BIN_LEN;
        let amount_x = read_u64(data, bin_offset)?;
        let amount_y = read_u64(data, bin_offset + 8)?;
        bins.push((amount_x, amount_y));
    }
    Some(DecodedAccount::MeteoraBinArray {
        lb_pair,
        bin_array_index,
        bins,
    })
}

/// Finds every Meteora DLMM pool on-chain, via `getProgramAccounts` filtered
/// to the account's exact byte length. Same "run every 10-30min, not on the
/// hot path" reasoning as Raydium's `discover_pools`.
/// Returns `active_id`/`bin_step` alongside metadata — `decode_lb_pair`
/// already parses them during discovery (same account, no extra call), so
/// the cache can be seeded with real values instead of the `0, 0` placeholder
/// used before the first WS notification arrives.
pub async fn discover_pools(rpc_url: &str) -> anyhow::Result<Vec<(PoolMetadata, CurveState)>> {
    let accounts =
        arb_core::rpc::get_program_accounts_by_size(rpc_url, PROGRAM_ID, LB_PAIR_LEN).await?;
    Ok(accounts
        .into_iter()
        .filter_map(|(pubkey, data)| {
            let (mut meta, curve_state) = decode_lb_pair(&data)?;
            meta.id = pubkey;
            Some((meta, curve_state))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_lb_pair(base_factor: u16, bin_step: u16, active_id: i32) -> Vec<u8> {
        let mut data = vec![0u8; LB_PAIR_LEN];
        data[OFFSET_BASE_FACTOR..OFFSET_BASE_FACTOR + 2]
            .copy_from_slice(&base_factor.to_le_bytes());
        // base_fee_power_factor left at 0
        data[OFFSET_ACTIVE_ID..OFFSET_ACTIVE_ID + 4].copy_from_slice(&active_id.to_le_bytes());
        data[OFFSET_BIN_STEP..OFFSET_BIN_STEP + 2].copy_from_slice(&bin_step.to_le_bytes());
        data[OFFSET_TOKEN_X_MINT..OFFSET_TOKEN_X_MINT + 32].copy_from_slice(&[3u8; 32]);
        data[OFFSET_TOKEN_Y_MINT..OFFSET_TOKEN_Y_MINT + 32].copy_from_slice(&[4u8; 32]);
        data[OFFSET_RESERVE_X..OFFSET_RESERVE_X + 32].copy_from_slice(&[1u8; 32]);
        data[OFFSET_RESERVE_Y..OFFSET_RESERVE_Y + 32].copy_from_slice(&[2u8; 32]);
        data
    }

    #[test]
    fn decodes_config_with_live_state_from_a_904_byte_account() {
        let data = synthetic_lb_pair(100, 10, 50);
        let decoded = MeteoraDlmm.decode(&data).unwrap();
        let DecodedAccount::ConfigWithLiveState(meta, curve_state) = decoded else {
            panic!("expected ConfigWithLiveState");
        };
        assert_eq!(meta.vault_a, Pubkey::from([1u8; 32]));
        assert_eq!(meta.vault_b, Pubkey::from([2u8; 32]));
        assert_eq!(meta.token_a, Pubkey::from([3u8; 32]));
        assert_eq!(meta.token_b, Pubkey::from([4u8; 32]));
        let CurveState::Dlmm {
            active_id,
            bin_step,
            ..
        } = curve_state
        else {
            panic!("expected Dlmm curve state");
        };
        assert_eq!(active_id, 50);
        assert_eq!(bin_step, 10);
    }

    #[test]
    fn decodes_vault_amount_from_a_165_byte_account() {
        let mut data = vec![0u8; spl::TOKEN_ACCOUNT_LEN];
        data[64..72].copy_from_slice(&77_000u64.to_le_bytes());
        let decoded = MeteoraDlmm.decode(&data).unwrap();
        assert_eq!(decoded, DecodedAccount::VaultAmount(77_000));
    }

    #[test]
    fn rejects_unknown_account_shapes() {
        assert!(MeteoraDlmm.decode(&[0u8; 10]).is_none());
    }

    fn synthetic_bin_array(index: i64, lb_pair: Pubkey, bins: &[(u64, u64)]) -> Vec<u8> {
        let mut data = vec![0u8; BIN_ARRAY_LEN];
        data[OFFSET_BIN_ARRAY_INDEX..OFFSET_BIN_ARRAY_INDEX + 8]
            .copy_from_slice(&index.to_le_bytes());
        data[OFFSET_BIN_ARRAY_LB_PAIR..OFFSET_BIN_ARRAY_LB_PAIR + 32]
            .copy_from_slice(lb_pair.as_ref());
        for (i, (amount_x, amount_y)) in bins.iter().enumerate() {
            let offset = OFFSET_BIN_ARRAY_BINS + i * BIN_LEN;
            data[offset..offset + 8].copy_from_slice(&amount_x.to_le_bytes());
            data[offset + 8..offset + 16].copy_from_slice(&amount_y.to_le_bytes());
        }
        data
    }

    #[test]
    fn decodes_bin_array_from_a_10136_byte_account() {
        let lb_pair = Pubkey::new_unique();
        let mut bins = vec![(0u64, 0u64); MAX_BIN_PER_ARRAY as usize];
        bins[5] = (123_456, 789_012);
        let data = synthetic_bin_array(3, lb_pair, &bins);

        let decoded = MeteoraDlmm.decode(&data).unwrap();
        let DecodedAccount::MeteoraBinArray {
            lb_pair: decoded_lb_pair,
            bin_array_index,
            bins: decoded_bins,
        } = decoded
        else {
            panic!("expected MeteoraBinArray");
        };
        assert_eq!(decoded_lb_pair, lb_pair);
        assert_eq!(bin_array_index, 3);
        assert_eq!(decoded_bins.len(), MAX_BIN_PER_ARRAY as usize);
        assert_eq!(decoded_bins[5], (123_456, 789_012));
        assert_eq!(decoded_bins[0], (0, 0));
    }

    #[test]
    fn bin_array_index_matches_the_sdks_truncate_then_adjust_algorithm() {
        // Ported from commons/src/extensions/bin_array.rs::bin_id_to_bin_array_index.
        assert_eq!(bin_array_index_for(0), 0);
        assert_eq!(bin_array_index_for(69), 0);
        assert_eq!(bin_array_index_for(70), 1);
        assert_eq!(bin_array_index_for(139), 1);
        assert_eq!(bin_array_index_for(-1), -1);
        assert_eq!(bin_array_index_for(-70), -1);
        assert_eq!(bin_array_index_for(-71), -2);
    }

    #[test]
    fn derive_bin_array_pda_is_deterministic() {
        let lb_pair = Pubkey::new_unique();
        let a = derive_bin_array_pda(&lb_pair, 5);
        let b = derive_bin_array_pda(&lb_pair, 5);
        let different_index = derive_bin_array_pda(&lb_pair, 6);
        assert_eq!(a, b);
        assert_ne!(a, different_index);
    }

    fn dlmm_meta(fee_bps: u16) -> PoolMetadata {
        PoolMetadata {
            id: Pubkey::new_unique(),
            dex: arb_core::Dex::Meteora,
            curve: arb_core::Curve::Dlmm,
            token_a: Pubkey::new_unique(),
            token_b: Pubkey::new_unique(),
            vault_a: Pubkey::new_unique(),
            vault_b: Pubkey::new_unique(),
            fee_bps,
        }
    }

    fn dlmm_state(reserve_a: u64, reserve_b: u64, nearby_bins: Vec<(i32, u64, u64)>) -> PoolState {
        PoolState {
            curve_state: CurveState::Dlmm {
                active_id: 0, // price = (1+bin_step/10000)^0 = 1.0 regardless of bin_step
                bin_step: 10,
                reserve_a: Some(reserve_a),
                reserve_b: Some(reserve_b),
                nearby_bins,
            },
            last_slot: 1,
        }
    }

    #[test]
    fn quote_against_deep_reserves_is_close_to_the_flat_price_without_bin_data() {
        let meta = dlmm_meta(30);
        // amount_in is 0.1% of reserve_a — price_impact_bps should be tiny.
        let state = dlmm_state(1_000_000_000, 1_000_000_000, Vec::new());

        let quote = MeteoraDlmm.quote(&meta, &state, 1_000_000, true);

        // fee = 0.3% of 1_000_000 = 3_000, so amount_out_at_flat_price = 997_000.
        // price_impact_bps = 1_000_000 / 1_001_000_000 * 10_000 -> 9 (truncated),
        // so amount_out is 997_000 * (1 - 9/10_000) = 996_102 (truncated) — close
        // to, but strictly less than, the flat-price amount.
        assert_eq!(quote.amount_out, 996_102);
        assert!(
            quote.amount_out < 997_000,
            "even against deep reserves, amount_out must never equal or exceed the \
             zero-slippage flat price now that it's discounted by price_impact_bps"
        );
        assert!(quote.price_impact_bps < 50, "impact should be small here");
    }

    #[test]
    fn quote_against_shallow_reserves_is_heavily_discounted_without_bin_data() {
        let meta = dlmm_meta(30);
        // amount_in is 10x the entire reserve — a trade that size should
        // quote close to nothing, not the ~infinite-liquidity flat price
        // this used to return before the fix.
        let state = dlmm_state(100, 100, Vec::new());

        let quote = MeteoraDlmm.quote(&meta, &state, 1_000, true);

        assert!(
            quote.price_impact_bps > 9_000,
            "reserve is 10x smaller than the trade — impact must be reported as severe"
        );
        assert!(
            quote.amount_out < 100,
            "amount_out must reflect that same severity, not quote near the flat-price \
             amount of ~997"
        );
    }

    #[test]
    fn quote_with_real_bin_data_fills_fully_at_zero_impact_when_bin_has_enough_liquidity() {
        let meta = dlmm_meta(30);
        // The active bin alone has far more than enough of the output token
        // (amount_y) to fill this trade — real DLMM bins are constant-price
        // internally, so this should report *zero* impact, not a discount.
        let state = dlmm_state(1_000_000_000, 1_000_000_000, vec![(0, 500, 10_000_000)]);

        let quote = MeteoraDlmm.quote(&meta, &state, 1_000_000, true);

        assert_eq!(
            quote.amount_out, 997_000,
            "fully filled within one bin at a constant price must not be discounted"
        );
        assert_eq!(quote.price_impact_bps, 0);
    }

    #[test]
    fn quote_caps_fill_when_no_neighboring_bin_data_is_known() {
        let meta = dlmm_meta(30);
        // The active bin only has 500 of the output token (amount_y), and no
        // neighboring bins are known — the walk can't invent liquidity it
        // hasn't observed, so the fill (and the impact it reports) must
        // reflect that shortfall.
        let state = dlmm_state(1_000_000_000, 1_000_000_000, vec![(0, 500, 500)]);

        let quote = MeteoraDlmm.quote(&meta, &state, 1_000_000, true);

        assert_eq!(
            quote.amount_out, 500,
            "amount_out must be capped at known liquidity, not the flat-price amount \
             an infinite-liquidity model would return"
        );
        assert!(
            quote.price_impact_bps > 9_900,
            "filling only 500 of a ~997_000 theoretical trade must report as severe impact"
        );
    }

    #[test]
    fn quote_walks_into_a_neighboring_bin_when_the_active_bin_runs_out() {
        let meta = dlmm_meta(30);
        // Active bin (id 0) is thin, but the next bin in the selling
        // direction (id -1, since a_to_b walks toward decreasing bin_id) has
        // plenty — this is exactly the real-world case `verify_pricing`
        // caught (active bin thin, real liquidity one bin over).
        let state = dlmm_state(
            1_000_000_000,
            1_000_000_000,
            vec![(0, 500, 500), (-1, 500, 10_000_000)],
        );

        let quote = MeteoraDlmm.quote(&meta, &state, 1_000_000, true);

        assert!(
            quote.amount_out > 500,
            "must cross into bin -1 for the remaining liquidity instead of stopping at \
             the active bin's own 500, amount_out={}",
            quote.amount_out
        );
        // 500 from bin 0, then the rest from bin -1 at its own (slightly
        // lower, price = 1.001^-1) price — not the same flat 1.0 rate,
        // hence not exactly 997_000. Computed independently, not asserted
        // from the implementation's own output.
        assert_eq!(
            quote.amount_out, 996_004,
            "bin -1 has enough to fill the rest at its own (slightly worse) price"
        );
        assert!(
            quote.price_impact_bps > 0,
            "crossing into a worse-priced bin must show up as nonzero impact, even \
             though the trade fully filled"
        );
    }

    #[test]
    fn quote_is_zero_when_pool_state_is_not_yet_ready() {
        let meta = dlmm_meta(30);
        let state = PoolState {
            curve_state: CurveState::Dlmm {
                active_id: 0,
                bin_step: 10,
                reserve_a: None, // vault balance hasn't arrived yet
                reserve_b: Some(1_000_000_000),
                nearby_bins: Vec::new(),
            },
            last_slot: 1,
        };

        let quote = MeteoraDlmm.quote(&meta, &state, 1_000_000, true);
        assert_eq!(quote.amount_out, 0);
        assert_eq!(quote.price_impact_bps, 10_000);
    }
}
