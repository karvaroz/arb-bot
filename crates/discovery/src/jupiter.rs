//! Pool/token discovery via Jupiter's Token API v2.
//! Verified live 2026-07-27: GET https://lite-api.jup.ag/tokens/v2/tag?query=verified
//! returns a JSON array of token objects (no API key required for this endpoint).

use std::collections::HashSet;
use std::str::FromStr;

use arb_core::Token;
use serde::Deserialize;
use solana_sdk::pubkey::Pubkey;

const VERIFIED_TOKENS_URL: &str = "https://lite-api.jup.ag/tokens/v2/tag?query=verified";

#[derive(Debug, Deserialize)]
struct JupiterToken {
    id: String,
    symbol: String,
    decimals: u8,
    #[serde(default)]
    #[serde(rename = "organicScoreLabel")]
    organic_score_label: Option<String>,
}

/// Refresh cadence: called every 10-30min by the discovery loop, not per pool update.
pub async fn fetch_verified_tokens(client: &reqwest::Client) -> anyhow::Result<Vec<Token>> {
    let raw: Vec<JupiterToken> = client
        .get(VERIFIED_TOKENS_URL)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let tokens = raw
        .into_iter()
        .filter_map(|t| {
            let mint = Pubkey::from_str(&t.id).ok()?;
            Some(Token {
                mint,
                decimals: t.decimals,
                symbol: t.symbol,
            })
        })
        .collect();

    Ok(tokens)
}

/// "Verified" (4,318+ tokens) turned out to be far broader than "liquid
/// enough to matter" — running the full discovery pipeline against it still
/// left 33k pools / 85k accounts, more than one WS connection survives
/// subscribing to (see IMPLEMENTATION_PLAN.md Phase 2). Jupiter's own
/// `organicScoreLabel` ("high"/"medium"/"low", already in the same response,
/// no extra API call) is a much tighter, free signal for "is this token
/// actually traded enough to be worth tracking."
/// `accepted_labels` (e.g. `["high", "medium"]`) is config-driven, not
/// hardcoded — it's a pragmatic subscription-budget knob, not a claim about
/// which pools are actually worth trading (that's the Research Engine's call
/// once real replay/PnL data exists, see IMPLEMENTATION_PLAN.md Phase 4).
pub async fn fetch_liquid_token_mints(
    client: &reqwest::Client,
    accepted_labels: &[String],
) -> anyhow::Result<HashSet<Pubkey>> {
    let raw: Vec<JupiterToken> = client
        .get(VERIFIED_TOKENS_URL)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    Ok(raw
        .into_iter()
        .filter(|t| {
            t.organic_score_label
                .as_deref()
                .is_some_and(|label| accepted_labels.iter().any(|a| a == label))
        })
        .filter_map(|t| Pubkey::from_str(&t.id).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Hits the real Jupiter API — not run by default (`cargo test --workspace`
    // stays network-free/CI-safe). Run explicitly with:
    //   cargo test -p discovery -- --ignored
    #[tokio::test]
    #[ignore]
    async fn fetches_verified_tokens_including_sol() {
        let client = reqwest::Client::new();
        let tokens = fetch_verified_tokens(&client).await.unwrap();
        assert!(!tokens.is_empty());
        assert!(tokens.iter().any(|t| t.symbol == "SOL"));
    }

    #[tokio::test]
    #[ignore]
    async fn liquid_token_set_is_meaningfully_smaller_than_verified() {
        let client = reqwest::Client::new();
        let verified = fetch_verified_tokens(&client).await.unwrap();
        let accepted = vec!["high".to_string(), "medium".to_string()];
        let liquid = fetch_liquid_token_mints(&client, &accepted).await.unwrap();
        println!(
            "verified: {}, liquid (high/medium organicScore): {}",
            verified.len(),
            liquid.len()
        );
        assert!(!liquid.is_empty());
        assert!(
            liquid.len() < verified.len(),
            "the whole point is that this filters something out"
        );
    }
}
