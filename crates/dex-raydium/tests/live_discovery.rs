//! Verifies `discover_pools` against real Helius — specifically, that
//! `getProgramAccounts` isn't blocked on the free tier (many RPC providers
//! disable/throttle it since it's expensive to serve). Not assumed to work
//! just because the code compiles.
//!
//!   cargo test -p dex-raydium --test live_discovery -- --ignored --nocapture

use dex_raydium::discover_pools;

#[tokio::test]
#[ignore]
async fn discovers_at_least_one_real_pool() {
    dotenvy::dotenv().ok();
    let key = std::env::var("HELIUS_API_KEY").expect("HELIUS_API_KEY not set in .env");
    let rpc_url = format!("https://mainnet.helius-rpc.com/?api-key={key}");

    let pools = discover_pools(&rpc_url)
        .await
        .expect("getProgramAccounts call failed");
    println!("discovered {} Raydium AMM v4 pools", pools.len());

    assert!(
        !pools.is_empty(),
        "expected at least one pool — Raydium has thousands live"
    );
    let sample = &pools[0];
    println!(
        "sample pool: id={} token_a={} token_b={}",
        sample.id, sample.token_a, sample.token_b
    );
}
