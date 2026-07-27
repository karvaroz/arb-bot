//! Orca Whirlpool (concentrated liquidity) decoder + pricing.
//!
//! Program ID: whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc (verified
//! 2026-07-27, audited by Kudelski Security and Neodyme).
//!
//! `Whirlpool` account layout verified against
//! https://github.com/orca-so/whirlpools/blob/main/programs/whirlpool/src/state/whirlpool.rs
//! — an Anchor `#[account]` (8-byte discriminator prefix, then Borsh-encoded
//! fields, no alignment padding). Total size: 653 bytes.
//!
//! Unlike Raydium: this ONE account carries both immutable config (mints,
//! vaults, fee) AND live state (`liquidity`, `sqrt_price`,
//! `tick_current_index`) that changes on every swap — there's no separate
//! "config never changes" account here. `decode()` returns
//! `DecodedAccount::ConfigWithLiveState` every time, not just once.
//!
//! Reserves still come from vault accounts (`token_vault_a`/`token_vault_b`,
//! plain SPL Token accounts) exactly like Raydium — the `arb_core::spl`
//! reader is reused as-is.

use arb_core::{CurveState, DecodedAccount, DexAdapter, PoolMetadata, PoolState, Quote, spl};
use solana_sdk::pubkey::Pubkey;

pub const PROGRAM_ID: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";
const WHIRLPOOL_LEN: usize = 653;

// Byte offsets into the Whirlpool account (8-byte Anchor discriminator,
// then Borsh fields in declaration order, no padding):
const OFFSET_FEE_RATE: usize = 45;
const OFFSET_LIQUIDITY: usize = 49;
const OFFSET_SQRT_PRICE: usize = 65;
const OFFSET_TICK_CURRENT_INDEX: usize = 81;
const OFFSET_TOKEN_MINT_A: usize = 101;
const OFFSET_TOKEN_VAULT_A: usize = 133;
const OFFSET_TOKEN_MINT_B: usize = 181;
const OFFSET_TOKEN_VAULT_B: usize = 213;

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u128(data: &[u8], offset: usize) -> Option<u128> {
    Some(u128::from_le_bytes(
        data.get(offset..offset + 16)?.try_into().ok()?,
    ))
}

fn read_i32(data: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_pubkey(data: &[u8], offset: usize) -> Option<Pubkey> {
    let bytes: [u8; 32] = data.get(offset..offset + 32)?.try_into().ok()?;
    Some(Pubkey::from(bytes))
}

pub struct OrcaWhirlpool;

impl DexAdapter for OrcaWhirlpool {
    fn decode(&self, account_data: &[u8]) -> Option<DecodedAccount> {
        match account_data.len() {
            WHIRLPOOL_LEN => decode_whirlpool(account_data)
                .map(|(meta, curve_state)| DecodedAccount::ConfigWithLiveState(meta, curve_state)),
            spl::TOKEN_ACCOUNT_LEN => {
                spl::token_account_amount(account_data).map(DecodedAccount::VaultAmount)
            }
            _ => None,
        }
    }

    // Single-tick-range approximation: assumes the swap stays within the
    // pool's current liquidity and doesn't cross a tick boundary. This is
    // the standard Uniswap-v3-style formula for that case. Real execution
    // would need to walk tick arrays for swaps that cross boundaries —
    // deferred to Phase 4/6, same "not more precise than filtering needs"
    // reasoning as Raydium's price_impact_bps estimate.
    fn quote(&self, meta: &PoolMetadata, state: &PoolState, amount_in: u64, a_to_b: bool) -> Quote {
        let CurveState::Whirlpool {
            sqrt_price,
            liquidity,
            ..
        } = state.curve_state
        else {
            return Quote {
                amount_out: 0,
                fee: 0,
                price_impact_bps: 10_000,
            };
        };

        let fee = (amount_in as u128 * meta.fee_bps as u128) / 10_000;
        let amount_in_after_fee = (amount_in as u128 - fee) as f64;
        let l = liquidity as f64;
        // Q64.64 fixed point -> real number.
        let p = sqrt_price as f64 / (2f64.powi(64));

        let amount_out = if a_to_b {
            // token A in, price decreases: Δ(1/√P) = Δx / L
            let new_p = 1.0 / (1.0 / p + amount_in_after_fee / l);
            l * (p - new_p)
        } else {
            // token B in, price increases: Δ√P = Δy / L
            let new_p = p + amount_in_after_fee / l;
            l * (1.0 / p - 1.0 / new_p)
        };

        let price_impact_bps = ((amount_in_after_fee / l) * 10_000.0).min(10_000.0) as u16;

        Quote {
            amount_out: amount_out.max(0.0) as u64,
            fee: fee as u64,
            price_impact_bps,
        }
    }
}

fn decode_whirlpool(data: &[u8]) -> Option<(PoolMetadata, CurveState)> {
    let fee_rate = read_u16(data, OFFSET_FEE_RATE)?;
    let fee_bps = fee_rate / 100; // fee_rate is hundredths-of-a-bp; bps = fee_rate/100

    let liquidity = read_u128(data, OFFSET_LIQUIDITY)?;
    let sqrt_price = read_u128(data, OFFSET_SQRT_PRICE)?;
    let tick_current_index = read_i32(data, OFFSET_TICK_CURRENT_INDEX)?;

    let token_mint_a = read_pubkey(data, OFFSET_TOKEN_MINT_A)?;
    let token_vault_a = read_pubkey(data, OFFSET_TOKEN_VAULT_A)?;
    let token_mint_b = read_pubkey(data, OFFSET_TOKEN_MINT_B)?;
    let token_vault_b = read_pubkey(data, OFFSET_TOKEN_VAULT_B)?;

    let meta = PoolMetadata {
        id: Pubkey::default(), // filled in by the caller
        dex: arb_core::Dex::Orca,
        curve: arb_core::Curve::Whirlpool,
        token_a: token_mint_a,
        token_b: token_mint_b,
        vault_a: token_vault_a,
        vault_b: token_vault_b,
        fee_bps,
    };
    let curve_state = CurveState::Whirlpool {
        sqrt_price,
        liquidity,
        tick_current_index,
    };
    Some((meta, curve_state))
}

/// Finds every Orca Whirlpool pool on-chain, via `getProgramAccounts`
/// filtered to the account's exact byte length. Same "run every 10-30min,
/// not on the hot path" reasoning as Raydium's `discover_pools`.
/// Returns liquidity alongside each pool's metadata — Whirlpool's config
/// account IS its live state, so `decode_whirlpool` already parses it during
/// discovery; discarding it and re-fetching later just to rank same-pair
/// duplicates would waste an RPC round-trip for data already in hand.
pub async fn discover_pools(rpc_url: &str) -> anyhow::Result<Vec<(PoolMetadata, u128)>> {
    let accounts =
        arb_core::rpc::get_program_accounts_by_size(rpc_url, PROGRAM_ID, WHIRLPOOL_LEN).await?;
    Ok(accounts
        .into_iter()
        .filter_map(|(pubkey, data)| {
            let (mut meta, curve_state) = decode_whirlpool(&data)?;
            meta.id = pubkey;
            let arb_core::CurveState::Whirlpool { liquidity, .. } = curve_state else {
                return None;
            };
            Some((meta, liquidity))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_whirlpool(liquidity: u128, sqrt_price: u128) -> Vec<u8> {
        let mut data = vec![0u8; WHIRLPOOL_LEN];
        data[OFFSET_FEE_RATE..OFFSET_FEE_RATE + 2].copy_from_slice(&3000u16.to_le_bytes()); // 0.3%
        data[OFFSET_LIQUIDITY..OFFSET_LIQUIDITY + 16].copy_from_slice(&liquidity.to_le_bytes());
        data[OFFSET_SQRT_PRICE..OFFSET_SQRT_PRICE + 16].copy_from_slice(&sqrt_price.to_le_bytes());
        data[OFFSET_TICK_CURRENT_INDEX..OFFSET_TICK_CURRENT_INDEX + 4]
            .copy_from_slice(&0i32.to_le_bytes());
        data[OFFSET_TOKEN_MINT_A..OFFSET_TOKEN_MINT_A + 32].copy_from_slice(&[3u8; 32]);
        data[OFFSET_TOKEN_VAULT_A..OFFSET_TOKEN_VAULT_A + 32].copy_from_slice(&[1u8; 32]);
        data[OFFSET_TOKEN_MINT_B..OFFSET_TOKEN_MINT_B + 32].copy_from_slice(&[4u8; 32]);
        data[OFFSET_TOKEN_VAULT_B..OFFSET_TOKEN_VAULT_B + 32].copy_from_slice(&[2u8; 32]);
        data
    }

    #[test]
    fn decodes_config_with_live_state_from_a_653_byte_account() {
        // sqrt_price for price = 1.0 is 2^64 exactly.
        let sqrt_price_at_price_one: u128 = 1u128 << 64;
        let data = synthetic_whirlpool(1_000_000_000, sqrt_price_at_price_one);
        let decoded = OrcaWhirlpool.decode(&data).unwrap();
        let DecodedAccount::ConfigWithLiveState(meta, curve_state) = decoded else {
            panic!("expected ConfigWithLiveState");
        };
        assert_eq!(meta.fee_bps, 30); // 3000 hundredths-of-bp / 100 = 30 bps = 0.3%
        assert_eq!(meta.vault_a, Pubkey::from([1u8; 32]));
        assert_eq!(meta.vault_b, Pubkey::from([2u8; 32]));
        let CurveState::Whirlpool { liquidity, .. } = curve_state else {
            panic!("expected Whirlpool curve state");
        };
        assert_eq!(liquidity, 1_000_000_000);
    }

    #[test]
    fn decodes_vault_amount_from_a_165_byte_account() {
        let mut data = vec![0u8; spl::TOKEN_ACCOUNT_LEN];
        data[64..72].copy_from_slice(&99_000u64.to_le_bytes());
        let decoded = OrcaWhirlpool.decode(&data).unwrap();
        assert_eq!(decoded, DecodedAccount::VaultAmount(99_000));
    }

    #[test]
    fn rejects_unknown_account_shapes() {
        assert!(OrcaWhirlpool.decode(&[0u8; 10]).is_none());
    }

    #[test]
    fn quote_at_price_one_is_roughly_one_to_one_minus_fee() {
        let meta = PoolMetadata {
            id: Pubkey::new_unique(),
            dex: arb_core::Dex::Orca,
            curve: arb_core::Curve::Whirlpool,
            token_a: Pubkey::new_unique(),
            token_b: Pubkey::new_unique(),
            vault_a: Pubkey::new_unique(),
            vault_b: Pubkey::new_unique(),
            fee_bps: 30,
        };
        let state = PoolState {
            curve_state: CurveState::Whirlpool {
                sqrt_price: 1u128 << 64, // price = 1.0
                liquidity: 1_000_000_000_000,
                tick_current_index: 0,
            },
            last_slot: 1,
        };

        let quote = OrcaWhirlpool.quote(&meta, &state, 1_000_000, true);

        // fee = 0.3% of 1_000_000 = 3_000; at price 1.0 with huge liquidity
        // relative to amount_in, amount_out should be very close to
        // amount_in - fee (997_000), well within 1%.
        let expected = 997_000i64;
        let diff = (quote.amount_out as i64 - expected).abs();
        assert!(
            diff < 100,
            "amount_out {} too far from ~{}",
            quote.amount_out,
            expected
        );
    }
}
