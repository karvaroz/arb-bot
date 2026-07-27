// Talks to `crates/api` (Axum, same process as the pipeline). Base URL is
// configurable since the dashboard may be served from a different origin
// than the API (see IMPLEMENTATION_PLAN.md Phase 5 deployment decision).
const API_BASE = import.meta.env.VITE_API_BASE ?? "http://127.0.0.1:8080";

export interface Metrics {
  rpc_latency_ms: number;
  decode_latency_ms: number;
  cache_latency_ms: number;
  scanner_latency_ms: number;
  simulation_latency_ms: number;
  pool_updates_sec: number;
  opportunities_found: number;
  simulations_completed: number;
  false_positive_rate: number | null;
}

export interface Pool {
  id: string;
  dex: "Raydium" | "Orca" | "Meteora";
  token_a: string;
  token_b: string;
  is_ready: boolean;
}

export interface OpportunityRow {
  slot: number;
  route_pools: string[];
  expected_profit: number;
  profit_margin_bps: number;
  max_price_impact_bps: number;
  fragile: boolean;
}

// The real `scanner.*` thresholds an opportunity must clear — read from the
// backend, not duplicated as static copy, so this can't drift from
// config/default.toml.
export interface ScannerConfig {
  max_hops: number;
  min_profit_bps: number;
}

// Real server-side pagination — `/pools` and `/opportunities` never return
// more than `limit` rows, `total` is the count after filtering.
export interface Paginated<T> {
  total: number;
  items: T[];
}

async function getJson<T>(path: string): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`);
  if (!res.ok) throw new Error(`${path}: ${res.status} ${res.statusText}`);
  return res.json();
}

// `/health` responds `text/plain` ("ok"), not JSON — the other endpoints do.
export async function fetchHealth(): Promise<string> {
  const res = await fetch(`${API_BASE}/health`);
  if (!res.ok) throw new Error(`/health: ${res.status} ${res.statusText}`);
  return res.text();
}
export const fetchMetrics = () => getJson<Metrics>("/metrics");
export const fetchConfig = () => getJson<ScannerConfig>("/config");

export function fetchPools(params: { limit: number; offset: number; q?: string }) {
  const qs = new URLSearchParams({
    limit: String(params.limit),
    offset: String(params.offset),
    ...(params.q ? { q: params.q } : {}),
  });
  return getJson<Paginated<Pool>>(`/pools?${qs}`);
}

export function fetchOpportunities(params: { limit: number; offset: number }) {
  const qs = new URLSearchParams({ limit: String(params.limit), offset: String(params.offset) });
  return getJson<Paginated<OpportunityRow>>(`/opportunities?${qs}`);
}

export function wsUrl(): string {
  return `${API_BASE.replace(/^http/, "ws")}/ws`;
}
