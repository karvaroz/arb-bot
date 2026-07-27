//! WS connection to the RPC provider's `accountSubscribe`, with reconnect
//! backoff. Emits raw `(account_pubkey, slot, bytes)` updates on `tx` — this
//! module has no idea what a "pool" is, and deliberately so: for Raydium (and
//! presumably Orca/Meteora) a pool spans multiple accounts (config + vaults),
//! and the vault pubkeys aren't known until the config account is decoded.
//! So subscriptions grow at runtime via `add_tx`, not just at startup —
//! decoding and mapping accounts back to a pool is the caller's job
//! (dex-* + market-data::cache), not this module's.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use solana_sdk::pubkey::Pubkey;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::rpc::{AccountNotification, SubscribeAck, decode_account_data, subscribe_request};

pub struct RawAccountUpdate {
    pub account_pubkey: Pubkey,
    pub slot: u64,
    pub data: Vec<u8>,
}

/// Runs until the process shuts down, reconnecting with exponential backoff
/// (capped at 30s) on any WS error. Never returns `Err` — a broken feed
/// retries instead of killing the scanner. `add_rx` lets the caller register
/// new pubkeys (e.g. vaults discovered from a just-decoded pool config)
/// after the connection is already up; the full accumulated set is
/// re-subscribed on every reconnect.
pub async fn run(
    ws_url: String,
    initial: Vec<Pubkey>,
    tx: mpsc::Sender<RawAccountUpdate>,
    mut add_rx: mpsc::UnboundedReceiver<Pubkey>,
) {
    let mut known = initial;
    let mut backoff = Duration::from_secs(1);
    loop {
        while let Ok(pk) = add_rx.try_recv() {
            known.push(pk);
        }
        match connect_and_stream(&ws_url, &mut known, &tx, &mut add_rx).await {
            Ok(()) => backoff = Duration::from_secs(1), // clean disconnect, reset backoff
            Err(e) => warn!(error = %e, "market-data WS connection lost, reconnecting"),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

async fn connect_and_stream(
    ws_url: &str,
    known: &mut Vec<Pubkey>,
    tx: &mpsc::Sender<RawAccountUpdate>,
    add_rx: &mut mpsc::UnboundedReceiver<Pubkey>,
) -> anyhow::Result<()> {
    let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url).await?;
    let (mut write, mut read) = ws_stream.split();

    // request id -> account pubkey, until the ack arrives
    let mut pending: HashMap<u64, Pubkey> = HashMap::new();
    // confirmed subscription id -> account pubkey, used for every notification after
    let mut subscriptions: HashMap<u64, Pubkey> = HashMap::new();
    let mut next_req_id: u64 = 0;

    for pubkey in known.iter() {
        send_subscribe(&mut write, &mut pending, &mut next_req_id, *pubkey).await?;
    }
    info!(count = known.len(), "sent accountSubscribe requests");

    // Helius (and most providers) drop idle WS connections after ~10min;
    // pinging every minute keeps it alive. Verified against
    // https://www.helius.dev/docs/api-reference/endpoints (2026-07-27).
    let mut keepalive = tokio::time::interval(Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = keepalive.tick() => {
                write.send(Message::Ping(Vec::new())).await?;
            }
            Some(pubkey) = add_rx.recv() => {
                known.push(pubkey);
                send_subscribe(&mut write, &mut pending, &mut next_req_id, pubkey).await?;
            }
            msg = read.next() => {
                let Some(msg) = msg else { break };
                let msg = msg?;
                let Message::Text(text) = msg else { continue };

                if let Ok(ack) = serde_json::from_str::<SubscribeAck>(&text) {
                    if let Some(pubkey) = pending.remove(&ack.id) {
                        subscriptions.insert(ack.result, pubkey);
                    }
                    continue;
                }

                let Ok(notification) = serde_json::from_str::<AccountNotification>(&text) else {
                    continue;
                };
                let Some(sub_id) = notification.params.as_ref().map(|p| p.subscription) else {
                    continue;
                };
                let Some(&account_pubkey) = subscriptions.get(&sub_id) else {
                    continue;
                };
                let Some((slot, data)) = decode_account_data(&notification) else {
                    continue;
                };

                if tx.send(RawAccountUpdate { account_pubkey, slot, data }).await.is_err() {
                    break; // receiver dropped, e.g. during shutdown
                }
            }
        }
    }

    Ok(())
}

async fn send_subscribe(
    write: &mut (impl SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin),
    pending: &mut HashMap<u64, Pubkey>,
    next_req_id: &mut u64,
    pubkey: Pubkey,
) -> anyhow::Result<()> {
    let req_id = *next_req_id;
    *next_req_id += 1;
    pending.insert(req_id, pubkey);
    let req = subscribe_request(req_id, &pubkey.to_string());
    write.send(Message::Text(req.to_string())).await?;
    Ok(())
}
