//! Minimal shared Solana JSON-RPC helper. Lives here (not in `discovery` or
//! any single `dex-*` crate) because all three DEX discovery modules need
//! the exact same call — `getProgramAccounts` filtered by exact byte length
//! — and `core` is the one crate every one of them already depends on,
//! without introducing a cycle (`discovery`/`dex-*` -> `core`, never back).

use base64::Engine;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;

/// Fetches a single account's raw data — the same `getAccountInfo` call
/// every live test in this workspace was duplicating ad hoc; promoted here
/// once a third caller needed it (the curated-pool-list pipeline).
pub async fn get_account_data(rpc_url: &str, pubkey: &str) -> anyhow::Result<Vec<u8>> {
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
        .await?
        .json()
        .await?;
    let data_b64 = resp["result"]["value"]["data"][0]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("account not found: {pubkey}"))?;
    Ok(base64::engine::general_purpose::STANDARD.decode(data_b64)?)
}

/// Fetches every account owned by `program_id` whose data is exactly
/// `data_size` bytes — the cheapest correct filter for "which pools of this
/// exact account type exist", since RPC providers reject unfiltered
/// `getProgramAccounts` calls on large programs.
pub async fn get_program_accounts_by_size(
    rpc_url: &str,
    program_id: &str,
    data_size: usize,
) -> anyhow::Result<Vec<(Pubkey, Vec<u8>)>> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getProgramAccounts",
        "params": [
            program_id,
            {
                "encoding": "base64",
                "filters": [{ "dataSize": data_size }]
            }
        ]
    });
    let resp: serde_json::Value = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    let results = resp["result"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("unexpected getProgramAccounts response: {resp}"))?;

    let mut accounts = Vec::with_capacity(results.len());
    for entry in results {
        let pubkey_str = entry["pubkey"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing pubkey"))?;
        let data_b64 = entry["account"]["data"][0]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing account data"))?;
        let pubkey = Pubkey::from_str(pubkey_str)?;
        let data = base64::engine::general_purpose::STANDARD.decode(data_b64)?;
        accounts.push((pubkey, data));
    }
    Ok(accounts)
}

/// Fetches raw data for many accounts in one round-trip via `getMultipleAccounts`
/// (chunked at 100 — Solana RPC's hard limit for this method). Used for
/// liquidity-ranking dedup: reading vault balances for every same-pair
/// candidate up front is far cheaper than opening a WS subscription per
/// candidate just to find out which one is deepest. Missing/closed accounts
/// are simply absent from the returned map, not an error.
pub async fn get_multiple_accounts_data(
    rpc_url: &str,
    pubkeys: &[Pubkey],
) -> anyhow::Result<HashMap<Pubkey, Vec<u8>>> {
    let client = reqwest::Client::new();
    let mut out = HashMap::with_capacity(pubkeys.len());

    for chunk in pubkeys.chunks(100) {
        let keys: Vec<String> = chunk.iter().map(|k| k.to_string()).collect();
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getMultipleAccounts",
            "params": [keys, { "encoding": "base64" }]
        });
        let resp: serde_json::Value = client
            .post(rpc_url)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        let values = resp["result"]["value"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("unexpected getMultipleAccounts response: {resp}"))?;
        for (pubkey, value) in chunk.iter().zip(values) {
            if let Some(data_b64) = value["data"][0].as_str()
                && let Ok(data) = base64::engine::general_purpose::STANDARD.decode(data_b64)
            {
                out.insert(*pubkey, data);
            }
        }
    }
    Ok(out)
}
