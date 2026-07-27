use serde::Deserialize;
use std::str::FromStr;
use tracing::info;

mod pipeline;

#[derive(Debug, Deserialize)]
struct RpcSettings {
    url: String,
    ws: String,
}

/// Verified endpoint shape: https://www.helius.dev/docs/api-reference/endpoints
fn apply_helius_override(rpc: &mut RpcSettings) {
    if let Ok(key) = std::env::var("HELIUS_API_KEY")
        && !key.is_empty()
    {
        rpc.url = format!("https://mainnet.helius-rpc.com/?api-key={key}");
        rpc.ws = format!("wss://mainnet.helius-rpc.com/?api-key={key}");
    }
}

/// Never log a URL with `?api-key=...` in it — strip the query string first.
fn redact(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

#[derive(Debug, Deserialize)]
struct ScannerSettings {
    base_tokens: Vec<String>,
    max_hops: u8,
    min_profit: f64,
}

#[derive(Debug, Deserialize)]
struct ResearchSettings {
    simulation_size: u32,
}

#[derive(Debug, Deserialize)]
struct DiscoverySettings {
    accepted_organic_scores: Vec<String>,
    max_subscriptions: usize,
    #[serde(default)]
    record_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiSettings {
    addr: String,
    db_path: String,
    #[serde(default)]
    static_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Settings {
    rpc: RpcSettings,
    scanner: ScannerSettings,
    research: ResearchSettings,
    discovery: DiscoverySettings,
    api: ApiSettings,
}

fn load_settings() -> Result<Settings, config::ConfigError> {
    config::Config::builder()
        .add_source(config::File::with_name("config/default"))
        .add_source(config::File::with_name("config/local").required(false))
        .add_source(config::File::with_name("config/production").required(false))
        .add_source(config::Environment::with_prefix("ARB").separator("__"))
        .build()?
        .try_deserialize()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok(); // .env is optional — fine if it's absent (e.g. in CI)

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut settings = load_settings()?;
    apply_helius_override(&mut settings.rpc);

    info!(
        rpc_url = redact(&settings.rpc.url),
        rpc_ws = redact(&settings.rpc.ws),
        base_tokens = settings.scanner.base_tokens.len(),
        max_hops = settings.scanner.max_hops,
        min_profit = settings.scanner.min_profit,
        simulation_size = settings.research.simulation_size,
        "loaded config"
    );

    // Latency instrumentation scaffolding (Phase 1): every later stage
    // (decode, cache, scanner, simulate) wraps its work in a span like this
    // one so bottlenecks are visible from day one, per IMPLEMENTATION_PLAN.md.
    let _span = tracing::info_span!("stage", name = "startup").entered();

    let base_tokens = settings
        .scanner
        .base_tokens
        .iter()
        .map(|s| solana_sdk::pubkey::Pubkey::from_str(s))
        .collect::<Result<Vec<_>, _>>()?;

    // `min_profit` is a percentage in config (e.g. 0.15 = 0.15%) — converted
    // to basis points here since that's the unit `Opportunity::
    // profit_margin_bps` (and thus `find_opportunities`'s threshold) uses.
    let min_profit_bps = (settings.scanner.min_profit * 100.0).round() as i32;

    pipeline::run(pipeline::RunConfig {
        rpc_url: settings.rpc.url,
        ws_url: settings.rpc.ws,
        base_tokens,
        accepted_organic_scores: settings.discovery.accepted_organic_scores,
        max_subscriptions: settings.discovery.max_subscriptions,
        record_dir: settings.discovery.record_dir,
        api_addr: settings.api.addr,
        db_path: settings.api.db_path,
        max_hops: settings.scanner.max_hops as usize,
        min_profit_bps,
        static_dir: settings.api.static_dir,
    })
    .await
}
