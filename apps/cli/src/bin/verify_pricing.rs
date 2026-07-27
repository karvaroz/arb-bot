//! Level 1 validation: does our `DexAdapter::quote()` match an independent
//! source (Jupiter), not just internal consistency? Queries Jupiter's quote
//! API first to learn which *exact* pool it routed through (`ammKey`), then
//! fetches and decodes that same real pool on-chain and compares our
//! `quote()` output against Jupiter's `outAmount` for the same trade —
//! apples-to-apples, not a hardcoded pool we might not actually be comparing
//! against what Jupiter used.
//!
//! Endpoint verified live before use (not guessed):
//! `https://lite-api.jup.ag/swap/v1/quote` — legacy `quote-api.jup.ag/v6`
//! returned nothing when checked 2026-07-27.
//!
//! Usage: `verify_pricing` (reads `HELIUS_API_KEY` from `.env`)

use arb_core::{CurveState, DecodedAccount, Dex, DexAdapter, PoolMetadata, PoolState, spl};
use dex_meteora::MeteoraDlmm;
use dex_orca::OrcaWhirlpool;
use dex_raydium::RaydiumAmmV4;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

const SOL: &str = "So11111111111111111111111111111111111111112";
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const AMOUNT_IN: u64 = 1_000_000_000; // 1 SOL

struct JupiterQuote {
    amm_key: Pubkey,
    out_amount: u64,
}

async fn jupiter_quote(client: &reqwest::Client, dex_label: &str) -> anyhow::Result<JupiterQuote> {
    let url = format!(
        "https://lite-api.jup.ag/swap/v1/quote?inputMint={SOL}&outputMint={USDC}&amount={AMOUNT_IN}&slippageBps=50&onlyDirectRoutes=true&dexes={}",
        urlencoding_space(dex_label)
    );
    let resp: serde_json::Value = client.get(&url).send().await?.json().await?;
    let leg = &resp["routePlan"][0]["swapInfo"];
    let amm_key = Pubkey::from_str(
        leg["ammKey"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no ammKey in Jupiter response: {resp}"))?,
    )?;
    let out_amount: u64 = resp["outAmount"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no outAmount in Jupiter response: {resp}"))?
        .parse()?;
    Ok(JupiterQuote {
        amm_key,
        out_amount,
    })
}

// Jupiter's `dexes` filter takes a comma list; a space in a label ("Meteora
// DLMM") is fine unencoded over HTTPS in practice, but encode it properly
// rather than rely on that.
fn urlencoding_space(s: &str) -> String {
    s.replace(' ', "%20")
}

async fn get_account_data(rpc_url: &str, pubkey: &Pubkey) -> Vec<u8> {
    arb_core::rpc::get_account_data(rpc_url, &pubkey.to_string())
        .await
        .unwrap_or_else(|e| panic!("failed to fetch {pubkey}: {e}"))
}

async fn build_raydium_state(
    rpc_url: &str,
    pool: &Pubkey,
) -> anyhow::Result<(PoolMetadata, PoolState)> {
    let data = get_account_data(rpc_url, pool).await;
    let Some(DecodedAccount::PoolConfig(mut meta)) = RaydiumAmmV4.decode(&data) else {
        anyhow::bail!("failed to decode Raydium AmmInfo at {pool}");
    };
    meta.id = *pool;

    let vault_a_data = get_account_data(rpc_url, &meta.vault_a).await;
    let vault_b_data = get_account_data(rpc_url, &meta.vault_b).await;
    let reserve_a = spl::token_account_amount(&vault_a_data);
    let reserve_b = spl::token_account_amount(&vault_b_data);

    let state = PoolState {
        curve_state: CurveState::ConstantProduct {
            reserve_a,
            reserve_b,
        },
        last_slot: 0,
    };
    Ok((meta, state))
}

async fn build_orca_state(
    rpc_url: &str,
    pool: &Pubkey,
) -> anyhow::Result<(PoolMetadata, PoolState)> {
    let data = get_account_data(rpc_url, pool).await;
    let Some(DecodedAccount::ConfigWithLiveState(mut meta, curve_state)) =
        OrcaWhirlpool.decode(&data)
    else {
        anyhow::bail!("failed to decode Orca Whirlpool at {pool}");
    };
    meta.id = *pool;
    Ok((
        meta,
        PoolState {
            curve_state,
            last_slot: 0,
        },
    ))
}

async fn build_meteora_state(
    rpc_url: &str,
    pool: &Pubkey,
) -> anyhow::Result<(PoolMetadata, PoolState)> {
    let data = get_account_data(rpc_url, pool).await;
    let Some(DecodedAccount::ConfigWithLiveState(mut meta, curve_state)) =
        MeteoraDlmm.decode(&data)
    else {
        anyhow::bail!("failed to decode Meteora LbPair at {pool}");
    };
    meta.id = *pool;
    let CurveState::Dlmm {
        active_id,
        bin_step,
        ..
    } = curve_state
    else {
        anyhow::bail!("expected Dlmm curve state");
    };

    let vault_a_data = get_account_data(rpc_url, &meta.vault_a).await;
    let vault_b_data = get_account_data(rpc_url, &meta.vault_b).await;
    let reserve_a = spl::token_account_amount(&vault_a_data);
    let reserve_b = spl::token_account_amount(&vault_b_data);

    // Real per-bin liquidity, same as the live pipeline: the active array
    // plus one neighbor on each side, so the walk has room to cross a
    // depleted active bin in either direction.
    let active_index = dex_meteora::bin_array_index_for(active_id);
    let mut nearby_bins = Vec::new();
    for array_index in [active_index - 1, active_index, active_index + 1] {
        let bin_array_pubkey = dex_meteora::derive_bin_array_pda(&meta.id, array_index);
        // A neighboring array may not exist on-chain at all (never
        // initialized, no liquidity ever seeded there) — that's not an
        // error, just no data for that range.
        let Ok(bin_array_data) =
            arb_core::rpc::get_account_data(rpc_url, &bin_array_pubkey.to_string()).await
        else {
            continue;
        };
        if let Some(DecodedAccount::MeteoraBinArray { bins, .. }) =
            MeteoraDlmm.decode(&bin_array_data)
        {
            let base_bin_id = array_index * dex_meteora::MAX_BIN_PER_ARRAY;
            for (i, (amount_x, amount_y)) in bins.into_iter().enumerate() {
                nearby_bins.push(((base_bin_id + i as i64) as i32, amount_x, amount_y));
            }
        }
    }

    let state = PoolState {
        curve_state: CurveState::Dlmm {
            active_id,
            bin_step,
            reserve_a,
            reserve_b,
            nearby_bins,
        },
        last_slot: 0,
    };
    Ok((meta, state))
}

async fn check_one(client: &reqwest::Client, rpc_url: &str, dex: Dex, jupiter_label: &str) {
    println!("\n=== {jupiter_label} ===");
    let jup = match jupiter_quote(client, jupiter_label).await {
        Ok(j) => j,
        Err(e) => {
            println!("  Jupiter quote failed: {e}");
            return;
        }
    };
    println!(
        "  Jupiter: pool={} out_amount={}",
        jup.amm_key, jup.out_amount
    );

    let built = match dex {
        Dex::Raydium => build_raydium_state(rpc_url, &jup.amm_key).await,
        Dex::Orca => build_orca_state(rpc_url, &jup.amm_key).await,
        Dex::Meteora => build_meteora_state(rpc_url, &jup.amm_key).await,
    };
    let (meta, state) = match built {
        Ok(v) => v,
        Err(e) => {
            println!("  failed to build local state for {}: {e}", jup.amm_key);
            return;
        }
    };
    if !state.is_ready() {
        println!("  local pool state not ready (missing reserves) — skipping comparison");
        return;
    }

    let sol = Pubkey::from_str(SOL).unwrap();
    let a_to_b = meta.token_a == sol;
    let adapter: &dyn DexAdapter = match dex {
        Dex::Raydium => &RaydiumAmmV4,
        Dex::Orca => &OrcaWhirlpool,
        Dex::Meteora => &MeteoraDlmm,
    };
    let quote = adapter.quote(&meta, &state, AMOUNT_IN, a_to_b);

    let diff = quote.amount_out as i128 - jup.out_amount as i128;
    let diff_pct = 100.0 * diff as f64 / jup.out_amount as f64;
    println!(
        "  ours:    out_amount={} price_impact_bps={}",
        quote.amount_out, quote.price_impact_bps
    );
    println!("  diff:    {diff} ({diff_pct:+.4}%)");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let key = std::env::var("HELIUS_API_KEY").expect("HELIUS_API_KEY not set in .env");
    let rpc_url = format!("https://mainnet.helius-rpc.com/?api-key={key}");
    let client = reqwest::Client::new();

    check_one(&client, &rpc_url, Dex::Raydium, "Raydium").await;
    check_one(&client, &rpc_url, Dex::Orca, "Whirlpool").await;
    check_one(&client, &rpc_url, Dex::Meteora, "Meteora DLMM").await;

    Ok(())
}
