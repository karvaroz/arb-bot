//! Hits a real Helius WS endpoint — needs HELIUS_API_KEY in .env, not run by
//! default. Proves subscribe/ack/notification parsing works end-to-end
//! against a live feed, not just against hand-written fixture messages.
//!
//!   cargo test -p market-data --test live_helius -- --ignored --nocapture

use std::str::FromStr;
use std::time::Duration;

use solana_sdk::pubkey::Pubkey;
use tokio::sync::mpsc;

#[tokio::test]
#[ignore]
async fn receives_at_least_one_account_update() {
    dotenvy::dotenv().ok();
    let key = std::env::var("HELIUS_API_KEY").expect("HELIUS_API_KEY not set in .env");
    let ws_url = format!("wss://mainnet.helius-rpc.com/?api-key={key}");

    // Wrapped SOL mint — high enough transaction volume to update within the timeout.
    let wsol = Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap();

    let (tx, mut rx) = mpsc::channel(16);
    let (_add_tx, add_rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(market_data::subscriptions::run(
        ws_url,
        vec![wsol],
        tx,
        add_rx,
    ));

    let update = tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("no account update received within 30s")
        .expect("channel closed unexpectedly");

    assert_eq!(update.account_pubkey, wsol);
    assert!(update.slot > 0);

    handle.abort();
}
