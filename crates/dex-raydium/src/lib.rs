//! Raydium legacy AMM v4 ("Liquidity Pool V4") decoder + pricing.
//!
//! Program ID: 675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8 (verified
//! 2026-07-27, https://docs.raydium.io/reference/program-addresses).
//!
//! `AmmInfo` layout verified against
//! https://github.com/raydium-io/raydium-amm/blob/master/program/src/state.rs
//! (`#[repr(C, packed)]`, so byte offsets are a flat cumulative sum of field
//! sizes — no alignment padding). Total size: 752 bytes.
//!
//! Reserves are NOT in this account: `AmmInfo` only stores `coin_vault`/
//! `pc_vault` pubkeys. The actual balances live in those two SPL Token
//! accounts, decoded separately via `arb_core::spl::token_account_amount`
//! (see IMPLEMENTATION_PLAN.md's note on this).

use arb_core::{CurveState, DecodedAccount, DexAdapter, PoolMetadata, PoolState, Quote, spl};
use solana_sdk::pubkey::Pubkey;

pub const PROGRAM_ID: &str = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";
const AMM_INFO_LEN: usize = 752;

// Byte offsets into `AmmInfo`, `#[repr(C, packed)]` so no padding:
// 16 leading u64 fields (0..128), then `fees: Fees` (8x u64, 128..192), then
// `state_data: StateData` (144 bytes, 192..336), then the Pubkey fields.
const OFFSET_TRADE_FEE_NUMERATOR: usize = 128 + 2 * 8; // 144
const OFFSET_TRADE_FEE_DENOMINATOR: usize = 128 + 3 * 8; // 152
const OFFSET_COIN_VAULT: usize = 336;
const OFFSET_PC_VAULT: usize = 336 + 32; // 368
const OFFSET_COIN_VAULT_MINT: usize = 336 + 64; // 400
const OFFSET_PC_VAULT_MINT: usize = 336 + 96; // 432

fn read_u64(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn read_pubkey(data: &[u8], offset: usize) -> Option<Pubkey> {
    let bytes: [u8; 32] = data.get(offset..offset + 32)?.try_into().ok()?;
    Some(Pubkey::from(bytes))
}

pub struct RaydiumAmmV4;

impl DexAdapter for RaydiumAmmV4 {
    fn decode(&self, account_data: &[u8]) -> Option<DecodedAccount> {
        match account_data.len() {
            AMM_INFO_LEN => decode_amm_info(account_data).map(DecodedAccount::PoolConfig),
            spl::TOKEN_ACCOUNT_LEN => {
                spl::token_account_amount(account_data).map(DecodedAccount::VaultAmount)
            }
            _ => None,
        }
    }

    fn quote(&self, meta: &PoolMetadata, state: &PoolState, amount_in: u64, a_to_b: bool) -> Quote {
        let CurveState::ConstantProduct {
            reserve_a,
            reserve_b,
        } = &state.curve_state
        else {
            return Quote {
                amount_out: 0,
                fee: 0,
                price_impact_bps: 10_000,
            };
        };
        let (reserve_in, reserve_out) = if a_to_b {
            (reserve_a.unwrap_or(0), reserve_b.unwrap_or(0))
        } else {
            (reserve_b.unwrap_or(0), reserve_a.unwrap_or(0))
        };

        let fee = (amount_in as u128 * meta.fee_bps as u128) / 10_000;
        let amount_in_after_fee = amount_in as u128 - fee;

        let amount_out = if reserve_in == 0 && reserve_out == 0 {
            0
        } else {
            (reserve_out as u128 * amount_in_after_fee) / (reserve_in as u128 + amount_in_after_fee)
        };

        // Rough impact estimate (fraction of the input-side pool consumed).
        // Phase 4's slippage model refines this — this is deliberately not
        // more precise than "pre-slippage candidate filtering" needs.
        let price_impact_bps = if reserve_in == 0 {
            10_000
        } else {
            ((amount_in as u128 * 10_000) / (reserve_in as u128 + amount_in as u128)).min(10_000)
                as u16
        };

        Quote {
            amount_out: amount_out as u64,
            fee: fee as u64,
            price_impact_bps,
        }
    }
}

fn decode_amm_info(data: &[u8]) -> Option<PoolMetadata> {
    let trade_fee_numerator = read_u64(data, OFFSET_TRADE_FEE_NUMERATOR)?;
    let trade_fee_denominator = read_u64(data, OFFSET_TRADE_FEE_DENOMINATOR)?;
    let fee_bps = trade_fee_numerator
        .checked_mul(10_000)
        .and_then(|n| n.checked_div(trade_fee_denominator))
        .unwrap_or(0) as u16;

    let coin_vault = read_pubkey(data, OFFSET_COIN_VAULT)?;
    let pc_vault = read_pubkey(data, OFFSET_PC_VAULT)?;
    let coin_mint = read_pubkey(data, OFFSET_COIN_VAULT_MINT)?;
    let pc_mint = read_pubkey(data, OFFSET_PC_VAULT_MINT)?;

    Some(PoolMetadata {
        id: Pubkey::default(), // filled in by the caller — this account's own pubkey isn't in its data
        dex: arb_core::Dex::Raydium,
        curve: arb_core::Curve::ConstantProduct,
        token_a: coin_mint,
        token_b: pc_mint,
        vault_a: coin_vault,
        vault_b: pc_vault,
        fee_bps,
    })
}

// coin_decimals/pc_decimals also live in AmmInfo (offsets 32, 40) but aren't
// read here: per-token decimals belong on `arb_core::Token`, populated by
// discovery (Jupiter already gives us decimals for every mint) — decoding
// them again here would be a second, possibly-conflicting source of truth.

/// Finds every Raydium AMM v4 pool that exists on-chain, via
/// `getProgramAccounts` filtered to `AmmInfo`'s exact byte length — cheap
/// and correct because 752 bytes is specific enough not to catch unrelated
/// accounts owned by the same program. Meant to run every 10-30min (new
/// pools are rare, not a per-block concern), not on the hot path.
pub async fn discover_pools(rpc_url: &str) -> anyhow::Result<Vec<PoolMetadata>> {
    let accounts =
        arb_core::rpc::get_program_accounts_by_size(rpc_url, PROGRAM_ID, AMM_INFO_LEN).await?;
    Ok(accounts
        .into_iter()
        .filter_map(|(pubkey, data)| {
            let mut meta = decode_amm_info(&data)?;
            meta.id = pubkey;
            Some(meta)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_amm_info() -> Vec<u8> {
        let mut data = vec![0u8; AMM_INFO_LEN];
        data[OFFSET_TRADE_FEE_NUMERATOR..OFFSET_TRADE_FEE_NUMERATOR + 8]
            .copy_from_slice(&25u64.to_le_bytes());
        data[OFFSET_TRADE_FEE_DENOMINATOR..OFFSET_TRADE_FEE_DENOMINATOR + 8]
            .copy_from_slice(&10_000u64.to_le_bytes());
        data[OFFSET_COIN_VAULT..OFFSET_COIN_VAULT + 32].copy_from_slice(&[1u8; 32]);
        data[OFFSET_PC_VAULT..OFFSET_PC_VAULT + 32].copy_from_slice(&[2u8; 32]);
        data[OFFSET_COIN_VAULT_MINT..OFFSET_COIN_VAULT_MINT + 32].copy_from_slice(&[3u8; 32]);
        data[OFFSET_PC_VAULT_MINT..OFFSET_PC_VAULT_MINT + 32].copy_from_slice(&[4u8; 32]);
        data
    }

    #[test]
    fn decodes_pool_config_from_a_752_byte_account() {
        let data = synthetic_amm_info();
        let decoded = RaydiumAmmV4.decode(&data).unwrap();
        let DecodedAccount::PoolConfig(meta) = decoded else {
            panic!("expected PoolConfig");
        };
        assert_eq!(meta.fee_bps, 25);
        assert_eq!(meta.vault_a, Pubkey::from([1u8; 32]));
        assert_eq!(meta.vault_b, Pubkey::from([2u8; 32]));
        assert_eq!(meta.token_a, Pubkey::from([3u8; 32]));
        assert_eq!(meta.token_b, Pubkey::from([4u8; 32]));
    }

    #[test]
    fn decodes_vault_amount_from_a_165_byte_account() {
        let mut data = vec![0u8; spl::TOKEN_ACCOUNT_LEN];
        data[64..72].copy_from_slice(&42_000u64.to_le_bytes());
        let decoded = RaydiumAmmV4.decode(&data).unwrap();
        assert_eq!(decoded, DecodedAccount::VaultAmount(42_000));
    }

    #[test]
    fn rejects_unknown_account_shapes() {
        assert!(RaydiumAmmV4.decode(&[0u8; 10]).is_none());
    }

    #[test]
    fn quotes_constant_product_swap_with_fee() {
        let meta = PoolMetadata {
            id: Pubkey::new_unique(),
            dex: arb_core::Dex::Raydium,
            curve: arb_core::Curve::ConstantProduct,
            token_a: Pubkey::new_unique(),
            token_b: Pubkey::new_unique(),
            vault_a: Pubkey::new_unique(),
            vault_b: Pubkey::new_unique(),
            fee_bps: 25, // 0.25%
        };
        let state = PoolState {
            curve_state: CurveState::ConstantProduct {
                reserve_a: Some(1_000_000),
                reserve_b: Some(1_000_000),
            },
            last_slot: 1,
        };
        let quote = RaydiumAmmV4.quote(&meta, &state, 10_000, true);

        assert_eq!(quote.fee, 25);
        // amount_in_after_fee = 10_000 - 25 = 9_975
        // amount_out = 1_000_000 * 9_975 / (1_000_000 + 9_975) = 9_876 (floor)
        assert_eq!(quote.amount_out, 9_876);
    }
}
