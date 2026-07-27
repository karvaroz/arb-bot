//! Decodes a REAL, live Meteora DLMM SOL/USDC pool account
//! (5XRqv7LCoC5FhWKk5JN8n4kCrJs3e4KH1XsYzKeMd5Nt — verified via
//! GeckoTerminal, 2026-07-27) fetched from Helius, proving the byte offsets
//! in `decode_lb_pair` are correct against mainnet.
//!
//!   cargo test -p dex-meteora --test live_pool -- --ignored --nocapture

use arb_core::{CurveState, DecodedAccount, DexAdapter};
use base64::Engine;
use dex_meteora::MeteoraDlmm;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

const SOL_USDC_LB_PAIR: &str = "5XRqv7LCoC5FhWKk5JN8n4kCrJs3e4KH1XsYzKeMd5Nt";

async fn get_account_data(rpc_url: &str, pubkey: &str) -> Vec<u8> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getAccountInfo",
        "params": [pubkey, { "encoding": "base64" }]
    });
    let resp: serde_json::Value = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let data_b64 = resp["result"]["value"]["data"][0]
        .as_str()
        .expect("account not found");
    base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .unwrap()
}

#[tokio::test]
#[ignore]
async fn decodes_real_sol_usdc_lb_pair() {
    dotenvy::dotenv().ok();
    let key = std::env::var("HELIUS_API_KEY").expect("HELIUS_API_KEY not set in .env");
    let rpc_url = format!("https://mainnet.helius-rpc.com/?api-key={key}");

    let data = get_account_data(&rpc_url, SOL_USDC_LB_PAIR).await;
    println!("account length: {}", data.len());

    let decoded = MeteoraDlmm
        .decode(&data)
        .expect("failed to decode a real mainnet LbPair account");
    let DecodedAccount::ConfigWithLiveState(meta, curve_state) = decoded else {
        panic!("expected ConfigWithLiveState, byte-length branch is wrong");
    };
    let CurveState::Dlmm {
        active_id,
        bin_step,
        ..
    } = curve_state
    else {
        panic!("expected Dlmm curve state");
    };

    println!("fee_bps: {}", meta.fee_bps);
    println!("token_a: {}", meta.token_a);
    println!("token_b: {}", meta.token_b);
    println!("vault_a: {}", meta.vault_a);
    println!("vault_b: {}", meta.vault_b);
    println!("active_id: {active_id}");
    println!("bin_step: {bin_step}");

    assert!(
        bin_step > 0 && bin_step < 1_000,
        "bin_step looks wrong: {bin_step}"
    );
    assert!(
        meta.fee_bps < 10_000,
        "fee_bps looks wrong: {}",
        meta.fee_bps
    );

    let usdc = Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap();
    let wsol = Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap();
    assert!(
        (meta.token_a == usdc || meta.token_b == usdc)
            && (meta.token_a == wsol || meta.token_b == wsol),
        "expected USDC/SOL mints, got {} / {} — mint offsets are likely wrong",
        meta.token_a,
        meta.token_b
    );

    for (label, vault) in [("a", meta.vault_a), ("b", meta.vault_b)] {
        let vault_data = get_account_data(&rpc_url, &vault.to_string()).await;
        let decoded_vault = MeteoraDlmm
            .decode(&vault_data)
            .expect("vault should decode as a 165-byte SPL token account");
        let DecodedAccount::VaultAmount(amount) = decoded_vault else {
            panic!("expected VaultAmount for vault {label}");
        };
        println!("vault {label} ({vault}) balance: {amount}");
        assert!(
            amount > 0,
            "vault {label} balance is zero — offsets or account are wrong"
        );
    }

    // Sanity check: price implied by active_id/bin_step should land in a
    // plausible SOL/USD range once decimals are applied (SOL=9, USDC=6).
    let price_raw = (1.0 + bin_step as f64 / 10_000.0).powi(active_id);
    let price_adjusted = if meta.token_a == wsol {
        price_raw * 1000.0
    } else {
        1000.0 / price_raw
    };
    println!("implied price: {price_adjusted}");
    assert!(
        price_adjusted > 1.0 && price_adjusted < 100_000.0,
        "implied price {price_adjusted} is not plausible"
    );
}

/// Proves the `BinArray` byte layout, the bin-array-index formula, and the
/// PDA derivation are all correct against a REAL mainnet account — not just
/// internally consistent with each other. Derives the address locally (no
/// discovery RPC call), fetches it, and checks the decoded `lb_pair`/`index`
/// match what was asked for.
///
///   cargo test -p dex-meteora --test live_pool -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn decodes_real_bin_array_for_sol_usdc() {
    dotenvy::dotenv().ok();
    let key = std::env::var("HELIUS_API_KEY").expect("HELIUS_API_KEY not set in .env");
    let rpc_url = format!("https://mainnet.helius-rpc.com/?api-key={key}");

    let lb_pair = Pubkey::from_str(SOL_USDC_LB_PAIR).unwrap();
    let lb_pair_data = get_account_data(&rpc_url, SOL_USDC_LB_PAIR).await;
    let DecodedAccount::ConfigWithLiveState(_meta, CurveState::Dlmm { active_id, .. }) =
        MeteoraDlmm.decode(&lb_pair_data).unwrap()
    else {
        panic!("expected ConfigWithLiveState/Dlmm");
    };
    println!("active_id: {active_id}");

    let bin_array_index = dex_meteora::bin_array_index_for(active_id);
    println!("bin_array_index: {bin_array_index}");
    let bin_array_pubkey = dex_meteora::derive_bin_array_pda(&lb_pair, bin_array_index);
    println!("derived bin_array pubkey: {bin_array_pubkey}");

    let bin_array_data = get_account_data(&rpc_url, &bin_array_pubkey.to_string()).await;
    println!("bin_array account length: {}", bin_array_data.len());
    assert_eq!(
        bin_array_data.len(),
        dex_meteora::BIN_ARRAY_LEN,
        "derived PDA doesn't have the expected BinArray size — index formula or seeds are wrong"
    );

    let decoded = MeteoraDlmm
        .decode(&bin_array_data)
        .expect("failed to decode a real mainnet BinArray account");
    let DecodedAccount::MeteoraBinArray {
        lb_pair: decoded_lb_pair,
        bin_array_index: decoded_index,
        bins,
    } = decoded
    else {
        panic!("expected MeteoraBinArray, byte-length branch is wrong");
    };

    assert_eq!(
        decoded_lb_pair, lb_pair,
        "BinArray's embedded lb_pair doesn't match the pool we derived it for"
    );
    assert_eq!(decoded_index, bin_array_index);
    assert_eq!(bins.len(), dex_meteora::MAX_BIN_PER_ARRAY as usize);

    let local_index = active_id as i64 - bin_array_index * dex_meteora::MAX_BIN_PER_ARRAY;
    let (active_amount_x, active_amount_y) = bins[local_index as usize];
    println!(
        "active bin (local index {local_index}): amount_x={active_amount_x}, amount_y={active_amount_y}"
    );
    assert!(
        active_amount_x > 0 || active_amount_y > 0,
        "SOL/USDC's active bin having zero liquidity on both sides is implausible for a real, actively-traded pool"
    );
}
