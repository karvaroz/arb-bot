//! Solana `accountSubscribe` JSON-RPC message shapes.
//! Verified against https://solana.com/docs/rpc/websocket/accountsubscribe (2026-07-27).

use base64::Engine;
use serde::Deserialize;
use serde_json::json;

pub fn subscribe_request(id: u64, pubkey: &str) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "accountSubscribe",
        "params": [
            pubkey,
            { "encoding": "base64", "commitment": "processed" }
        ]
    })
}

#[derive(Debug, Deserialize)]
pub struct AccountNotification {
    pub method: String,
    pub params: Option<NotificationParams>,
}

/// The initial response to a subscribe request: `{"result": <sub_id>, "id": <req_id>}`.
/// Needed to map the subscription id carried by later notifications back to
/// the pool pubkey that was subscribed with request id `id`.
#[derive(Debug, Deserialize)]
pub struct SubscribeAck {
    pub result: u64,
    pub id: u64,
}

#[derive(Debug, Deserialize)]
pub struct NotificationParams {
    pub result: NotificationResult,
    pub subscription: u64,
}

#[derive(Debug, Deserialize)]
pub struct NotificationResult {
    pub context: NotificationContext,
    pub value: AccountValue,
}

#[derive(Debug, Deserialize)]
pub struct NotificationContext {
    pub slot: u64,
}

#[derive(Debug, Deserialize)]
pub struct AccountValue {
    /// `["<base64 data>", "base64"]` per the requested encoding.
    pub data: (String, String),
    pub owner: String,
}

/// Decodes the base64 payload out of an `accountNotification`, if this
/// message is one (subscribe-ack responses are filtered out by the caller).
pub fn decode_account_data(notification: &AccountNotification) -> Option<(u64, Vec<u8>)> {
    let params = notification.params.as_ref()?;
    if notification.method != "accountNotification" {
        return None;
    }
    let slot = params.result.context.slot;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&params.result.value.data.0)
        .ok()?;
    Some((slot, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_subscribe_request() {
        let req = subscribe_request(1, "So11111111111111111111111111111111111111112");
        assert_eq!(req["method"], "accountSubscribe");
        assert_eq!(
            req["params"][0],
            "So11111111111111111111111111111111111111112"
        );
    }

    #[test]
    fn parses_account_notification() {
        let raw = r#"{
            "jsonrpc": "2.0",
            "method": "accountNotification",
            "params": {
                "result": {
                    "context": { "slot": 123 },
                    "value": {
                        "data": ["aGVsbG8=", "base64"],
                        "owner": "11111111111111111111111111111111"
                    }
                },
                "subscription": 5
            }
        }"#;
        let notif: AccountNotification = serde_json::from_str(raw).unwrap();
        let (slot, bytes) = decode_account_data(&notif).unwrap();
        assert_eq!(slot, 123);
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn ignores_non_notification_messages() {
        let raw = r#"{"jsonrpc":"2.0","result":5,"id":1}"#;
        let notif: AccountNotification = serde_json::from_str(raw).unwrap_or(AccountNotification {
            method: String::new(),
            params: None,
        });
        assert!(decode_account_data(&notif).is_none());
    }
}
