//! Decodes a REAL, live Orca Whirlpool USDC/SOL pool account
//! (HJPjoWUrhoZzkNfRpHuieeFk9WcZWjwy6PBjZ81ngndJ — verified via
//! GeckoTerminal, 2026-07-27) fetched from Helius, proving the byte offsets
//! in `decode_whirlpool` are correct against mainnet.
//!
//!   cargo test -p dex-orca --test live_pool -- --ignored --nocapture

use arb_core::{CurveState, DecodedAccount, DexAdapter};
use base64::Engine;
use dex_orca::OrcaWhirlpool;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

const USDC_SOL_WHIRLPOOL: &str = "HJPjoWUrhoZzkNfRpHuieeFk9WcZWjwy6PBjZ81ngndJ";

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
async fn decodes_real_usdc_sol_whirlpool() {
    dotenvy::dotenv().ok();
    let key = std::env::var("HELIUS_API_KEY").expect("HELIUS_API_KEY not set in .env");
    let rpc_url = format!("https://mainnet.helius-rpc.com/?api-key={key}");

    let data = get_account_data(&rpc_url, USDC_SOL_WHIRLPOOL).await;
    println!("account length: {}", data.len());

    let decoded = OrcaWhirlpool
        .decode(&data)
        .expect("failed to decode a real mainnet Whirlpool account");
    let DecodedAccount::ConfigWithLiveState(meta, curve_state) = decoded else {
        panic!("expected ConfigWithLiveState, byte-length branch is wrong");
    };
    let CurveState::Whirlpool {
        sqrt_price,
        liquidity,
        tick_current_index,
    } = curve_state
    else {
        panic!("expected Whirlpool curve state");
    };

    println!("fee_bps: {}", meta.fee_bps);
    println!("token_a: {}", meta.token_a);
    println!("token_b: {}", meta.token_b);
    println!("vault_a: {}", meta.vault_a);
    println!("vault_b: {}", meta.vault_b);
    println!("liquidity: {liquidity}");
    println!("sqrt_price: {sqrt_price}");
    println!("tick_current_index: {tick_current_index}");

    assert!(
        meta.fee_bps > 0 && meta.fee_bps < 1_000,
        "fee_bps looks wrong: {}",
        meta.fee_bps
    );
    assert!(
        liquidity > 0,
        "a pool with $200k+ TVL shouldn't have zero liquidity"
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
        let decoded_vault = OrcaWhirlpool
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
}
