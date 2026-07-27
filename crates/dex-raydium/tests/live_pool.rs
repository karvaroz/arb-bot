//! Decodes a REAL, live Raydium SOL/USDC AMM v4 pool account
//! (58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2 — verified via GeckoTerminal/
//! Solscan, 2026-07-27) fetched from Helius, proving the byte offsets in
//! `decode_amm_info` are correct against mainnet, not just synthetic bytes.
//!
//!   cargo test -p dex-raydium --test live_pool -- --ignored --nocapture

use arb_core::{DecodedAccount, DexAdapter};
use base64::Engine;
use dex_raydium::RaydiumAmmV4;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

const SOL_USDC_POOL: &str = "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2";

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
async fn decodes_real_sol_usdc_pool_config() {
    dotenvy::dotenv().ok();
    let key = std::env::var("HELIUS_API_KEY").expect("HELIUS_API_KEY not set in .env");
    let rpc_url = format!("https://mainnet.helius-rpc.com/?api-key={key}");

    let data = get_account_data(&rpc_url, SOL_USDC_POOL).await;
    println!("account length: {}", data.len());

    let decoded = RaydiumAmmV4
        .decode(&data)
        .expect("failed to decode a real mainnet AmmInfo account");
    let DecodedAccount::PoolConfig(meta) = decoded else {
        panic!("expected PoolConfig, byte-length branch is wrong");
    };

    println!("fee_bps: {}", meta.fee_bps);
    println!("token_a (coin mint): {}", meta.token_a);
    println!("token_b (pc mint): {}", meta.token_b);
    println!("vault_a: {}", meta.vault_a);
    println!("vault_b: {}", meta.vault_b);

    // Fee should be a sane basis-points value, not garbage from a wrong offset.
    assert!(
        meta.fee_bps > 0 && meta.fee_bps < 1_000,
        "fee_bps looks wrong: {}",
        meta.fee_bps
    );

    // One side must be Wrapped SOL — confirms the mint offsets are correct.
    let wsol = Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap();
    assert!(
        meta.token_a == wsol || meta.token_b == wsol,
        "neither token is WSOL — mint offsets are likely wrong"
    );

    // Fetch both vaults and confirm they decode to non-zero balances.
    for (label, vault) in [("coin", meta.vault_a), ("pc", meta.vault_b)] {
        let vault_data = get_account_data(&rpc_url, &vault.to_string()).await;
        let decoded_vault = RaydiumAmmV4
            .decode(&vault_data)
            .expect("vault should decode as a 165-byte SPL token account");
        let DecodedAccount::VaultAmount(amount) = decoded_vault else {
            panic!("expected VaultAmount for {label} vault");
        };
        println!("{label} vault ({vault}) balance: {amount}");
        assert!(
            amount > 0,
            "{label} vault balance is zero — offsets or account are wrong"
        );
    }
}
