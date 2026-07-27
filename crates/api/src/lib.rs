//! REST + WebSocket API for the research/monitoring dashboard
//! (IMPLEMENTATION_PLAN.md Phase 5). Runs in the same process as
//! `apps/cli`'s pipeline — no IPC, just `Arc`-shared state (`PoolCache` is
//! already `DashMap`-backed; `storage::Db` needs a `Mutex` since
//! `rusqlite::Connection` isn't `Sync`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use arb_core::{Metrics, events::Event};
use axum::{
    Json, Router,
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
    routing::get,
};
use market_data::PoolCache;
use solana_sdk::pubkey::Pubkey;
use storage::Db;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

/// The `scanner.*` thresholds that decide what counts as an `Opportunity` —
/// exposed so the dashboard can explain its own filtering in terms of the
/// real configured values instead of duplicating (and risking drift from)
/// numbers already loaded server-side (see `apps/cli/src/main.rs`).
#[derive(Clone, Copy, serde::Serialize)]
pub struct ScannerConfig {
    pub max_hops: u8,
    pub min_profit_bps: i32,
}

#[derive(Clone)]
pub struct ApiState {
    pub pool_cache: Arc<PoolCache>,
    pub db: Arc<Mutex<Db>>,
    pub metrics: Arc<RwLock<Metrics>>,
    pub events: broadcast::Sender<Event>,
    pub scanner_config: ScannerConfig,
    /// The built dashboard (`apps/dashboard`'s `vite build` output), served
    /// same-origin in production so the single Docker container is the
    /// whole app — no separate dashboard container/CDN (see
    /// IMPLEMENTATION_PLAN.md Phase 5 deployment decision). `None` in dev:
    /// Vite's own dev server (`:5173`) serves the dashboard there instead,
    /// which is why CORS below is permissive — that's a cross-origin setup
    /// only in dev.
    pub static_dir: Option<PathBuf>,
}

pub fn router(state: ApiState) -> Router {
    let static_dir = state.static_dir.clone();
    // Permissive: the dashboard is served from a different origin in dev
    // (Vite on :5173) than the API (:8080). Fine for a 2-user internal
    // tool — the plan's own deployment decision puts real access control
    // (Caddy + HTTP Basic Auth) in front in production, not CORS.
    let router = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .route("/config", get(scanner_config))
        .route("/pools", get(pools))
        .route("/opportunities", get(opportunities))
        .route("/ws", get(ws_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    match static_dir {
        // Anything not matched by an API route above falls through to the
        // static file server; a path with no file extension (client-side
        // route like `/pools`) falls further through to `index.html` so the
        // SPA's own router handles it — same trick every SPA-behind-a-
        // static-server setup needs.
        Some(dir) => {
            let index = dir.join("index.html");
            router.fallback_service(ServeDir::new(dir).not_found_service(ServeFile::new(index)))
        }
        None => router,
    }
}

async fn health() -> &'static str {
    "ok"
}

async fn metrics(State(state): State<ApiState>) -> Json<Metrics> {
    Json(*state.metrics.read().unwrap())
}

async fn scanner_config(State(state): State<ApiState>) -> Json<ScannerConfig> {
    Json(state.scanner_config)
}

#[derive(serde::Serialize)]
struct PoolSummary {
    #[serde(with = "arb_core::pubkey_json")]
    id: Pubkey,
    dex: arb_core::Dex,
    #[serde(with = "arb_core::pubkey_json")]
    token_a: Pubkey,
    #[serde(with = "arb_core::pubkey_json")]
    token_b: Pubkey,
    is_ready: bool,
}

/// Real server-side pagination — `/pools` and `/opportunities` can both grow
/// past what's reasonable to ship in one response (1000+ pools). `total` is
/// the count AFTER filtering (`q`), so a client can compute page count
/// without ever holding the full list.
#[derive(serde::Serialize)]
struct Paginated<T> {
    total: usize,
    items: Vec<T>,
}

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

#[derive(serde::Deserialize)]
struct PoolsQuery {
    limit: Option<usize>,
    offset: Option<usize>,
    /// Case-insensitive substring match against the pool address.
    q: Option<String>,
}

async fn pools(
    State(state): State<ApiState>,
    Query(query): Query<PoolsQuery>,
) -> Json<Paginated<PoolSummary>> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let offset = query.offset.unwrap_or(0);
    let needle = query.q.map(|q| q.to_lowercase());

    let mut all: Vec<PoolSummary> = state
        .pool_cache
        .pool_ids()
        .into_iter()
        .filter_map(|id| {
            let meta = state.pool_cache.get_metadata(&id)?;
            if let Some(needle) = &needle
                && !id.to_string().to_lowercase().contains(needle.as_str())
            {
                return None;
            }
            let is_ready = state
                .pool_cache
                .get(&id)
                .map(|s| s.is_ready())
                .unwrap_or(false);
            Some(PoolSummary {
                id,
                dex: meta.dex,
                token_a: meta.token_a,
                token_b: meta.token_b,
                is_ready,
            })
        })
        .collect();
    let total = all.len();
    // Stable order across pages — `pool_ids()` comes from a `DashMap`, whose
    // iteration order isn't guaranteed to stay put between calls.
    all.sort_by_key(|p| p.id);
    let items = all.into_iter().skip(offset).take(limit).collect();
    Json(Paginated { total, items })
}

#[derive(serde::Deserialize)]
struct PageQuery {
    limit: Option<usize>,
    offset: Option<usize>,
}

/// Historical, from SQLite — not the in-memory cache. Newest first.
async fn opportunities(
    State(state): State<ApiState>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Paginated<storage::OpportunityRow>>, (axum::http::StatusCode, String)> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let offset = query.offset.unwrap_or(0);

    let mut rows = state
        .db
        .lock()
        .unwrap()
        .all_rows()
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    rows.reverse();
    let total = rows.len();
    let items = rows.into_iter().skip(offset).take(limit).collect();
    Ok(Json(Paginated { total, items }))
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<ApiState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_stream(socket, state.events.subscribe()))
}

async fn ws_stream(mut socket: WebSocket, mut rx: broadcast::Receiver<Event>) {
    // A slow client or the gap between broadcast::send and this client's
    // subscribe causes Lagged — these are point-in-time updates, not a log
    // a client needs every entry of, so skip forward instead of dropping
    // the connection.
    loop {
        match rx.recv().await {
            Ok(event) => {
                let Ok(text) = serde_json::to_string(&event) else {
                    continue;
                };
                if socket.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::util::ServiceExt;

    fn test_state() -> ApiState {
        let (events, _) = broadcast::channel(16);
        ApiState {
            pool_cache: Arc::new(PoolCache::new()),
            db: Arc::new(Mutex::new(Db::open(":memory:").unwrap())),
            metrics: Arc::new(RwLock::new(Metrics::default())),
            events,
            scanner_config: ScannerConfig {
                max_hops: 3,
                min_profit_bps: 15,
            },
            static_dir: None,
        }
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let resp = router(test_state())
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn pools_and_opportunities_are_paginated_and_empty_on_a_fresh_state() {
        for path in ["/pools", "/opportunities"] {
            let resp = router(test_state())
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), axum::http::StatusCode::OK);
            let body = resp.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(
                body.as_ref(),
                br#"{"total":0,"items":[]}"#,
                "path {path} should start empty"
            );
        }
    }

    #[tokio::test]
    async fn pools_respects_limit_offset_and_q() {
        let state = test_state();
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        state.pool_cache.register_metadata(
            a,
            arb_core::PoolMetadata {
                id: a,
                dex: arb_core::Dex::Raydium,
                curve: arb_core::Curve::ConstantProduct,
                token_a: Pubkey::new_unique(),
                token_b: Pubkey::new_unique(),
                vault_a: Pubkey::new_unique(),
                vault_b: Pubkey::new_unique(),
                fee_bps: 25,
            },
        );
        state.pool_cache.register_metadata(
            b,
            arb_core::PoolMetadata {
                id: b,
                dex: arb_core::Dex::Orca,
                curve: arb_core::Curve::Whirlpool,
                token_a: Pubkey::new_unique(),
                token_b: Pubkey::new_unique(),
                vault_a: Pubkey::new_unique(),
                vault_b: Pubkey::new_unique(),
                fee_bps: 25,
            },
        );

        let resp = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/pools?limit=1&offset=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(
            body["total"], 2,
            "total must count both pools, not just the page"
        );
        assert_eq!(
            body["items"].as_array().unwrap().len(),
            1,
            "limit=1 must return one item"
        );

        let resp = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!("/pools?q={a}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(
            body["total"], 1,
            "q filter must narrow to the matching pool only"
        );
    }
}
