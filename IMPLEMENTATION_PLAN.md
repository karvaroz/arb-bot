# Solana Arbitrage Bot — Implementation Plan

## Principle

Market data engine first, trading bot second. The execution engine (Phase 6) is
just another consumer of the scanner once simulation proves an edge. Nothing in
Phases 1–5 touches a wallet or signs a transaction.

## Workspace layout

```
arb-bot/
├── Cargo.toml
├── crates/
│   ├── core/           # Token, Pool, Edge, Quote, Route, Opportunity, Curve, events.rs
│   ├── dex-raydium/    # impl DexAdapter (decode + quote)
│   ├── dex-orca/       # impl DexAdapter (decode + quote, tick-crossing)
│   ├── dex-meteora/    # impl DexAdapter (decode + quote, bin-based)
│   ├── discovery/      # jupiter.rs, raydium.rs, orca.rs, meteora.rs — finds which
│   │                   # pools exist, refreshes every 10-30min, hands the list to market-data
│   ├── market-data/    # rpc/ subscriptions/ cache/ recorder/ (subscription is
│   │                   # DEX-agnostic — lives here, not in DexAdapter)
│   ├── scanner/        # graph builder (built once), edge-weight updater, route enumerator
│   ├── research/       # replay engine, PnL calculator (formerly "simulator")
│   ├── storage/        # sqlite persistence
│   └── api/            # REST + WebSocket, translates core::Event for the dashboard
├── apps/
│   ├── dashboard/      # React + TS, read-only consumer
│   └── cli/            # scanner commands
├── tests/
│   ├── integration/
│   ├── fixtures/
│   └── replay/         # recorded slot -> replay -> assert expected opportunities
```

`executor/` is deliberately not scaffolded — added at the start of Phase 6, not before.
The scanner has no HTTP/WebSocket code at all — `api` is the only crate that
knows how to talk to the outside world; scanner and research only emit
`core::Event` values.

## Core domain model (`crates/core`)

```rust
struct Token { mint: Pubkey, decimals: u8, symbol: String }

enum Curve { ConstantProduct, Whirlpool, Dlmm }

// Immutable, set once at pool discovery. vault_a/vault_b: most AMMs (Raydium
// confirmed) keep config in one account and reserves in two SEPARATE SPL
// Token vault accounts — these are what market-data subscribes to next.
struct PoolMetadata {
    id: Pubkey,
    dex: Dex,
    curve: Curve,
    token_a: Pubkey,
    token_b: Pubkey,
    vault_a: Pubkey,
    vault_b: Pubkey,
    fee_bps: u16,
}

// Per-curve pricing state — one variant per curve shape, not a flat struct
// of Option<_> fields shared across every DEX. Adding a DEX means adding a
// variant, not adding more fields that are None for every other curve.
enum CurveState {
    // reserve_a/reserve_b start None, set independently as each vault's
    // balance arrives (separate accounts, separate notifications).
    ConstantProduct { reserve_a: Option<u64>, reserve_b: Option<u64> },
    // These three always arrive together (same account, same decode) — no
    // Option needed; the pool just isn't in the cache until first decoded.
    Whirlpool { sqrt_price: u128, liquidity: u128, tick_current_index: i32 },
}

struct PoolState {
    curve_state: CurveState,
    last_slot: u64,
}
impl PoolState {
    // Meaning differs per curve — both vault sides loaded vs. always-true.
    fn is_ready(&self) -> bool {
        match &self.curve_state {
            CurveState::ConstantProduct { reserve_a, reserve_b } => reserve_a.is_some() && reserve_b.is_some(),
            CurveState::Whirlpool { .. } => true,
        }
    }
}

enum Side { A, B }

// A decoder can't always return a full PoolState from one account. Two
// shapes confirmed so far: Raydium splits config from vault-only reserves
// (PoolConfig once, VaultAmount repeatedly); Orca's config account IS the
// live state (ConfigWithLiveState every time). A decoder only reads bytes —
// matching a vault to its pool/side is the caller's (market-data's)
// bookkeeping via the PoolConfig/ConfigWithLiveState decoded earlier.
enum DecodedAccount {
    PoolConfig(PoolMetadata),
    VaultAmount(u64),
    ConfigWithLiveState(PoolMetadata, CurveState), // last_slot is the cache's concern, not the decoder's
}

// Implemented once per DEX (Raydium/Orca/Meteora) — decode + pricing only.
// Subscription is deliberately NOT part of this trait: accountSubscribe is
// the same RPC call regardless of DEX, so it belongs to market-data.
trait DexAdapter {
    fn decode(&self, account_data: &[u8]) -> Option<DecodedAccount>;
    // &PoolState, not flat reserve numbers: constant-product and
    // concentrated-liquidity DEXs need genuinely different fields from it.
    // The scanner checks is_ready() before calling this.
    fn quote(&self, meta: &PoolMetadata, state: &PoolState, amount_in: u64, a_to_b: bool) -> Quote;
}

struct Quote { amount_out: u64, fee: u64, price_impact_bps: u16 }

struct Route { pools: Vec<Pubkey>, tokens: Vec<Pubkey> } // 2-3 hops, starts/ends on a base token

struct Opportunity { route: Route, expected_profit: i64, slot: u64 }

// core/events.rs — the one type scanner, research, api, dashboard and cli all share.
enum Event {
    PoolUpdated { pool_id: Pubkey, slot: u64 },
    OpportunityFound(Opportunity),
    SimulationFinished { opportunity: Opportunity, actual_profit: i64 },
    MetricsUpdated(Metrics),
}
```

No `ExecutionEngine` trait yet — it would have zero implementors and zero
callers today. The extension point already exists implicitly: `research`
produces `Opportunity` values, and Phase 6's executor consumes them. The
trait gets written when Phase 6 starts, not before.

Same reasoning applies to two other proposed abstractions, rejected for now:
a `StorageFormat` trait behind the recorder (one implementation — `bincode`
— exists, no second one is needed) and a `Storage` trait for a hypothetical
SQLite→Postgres swap (the `storage` crate boundary already hides SQL from
every other crate; a trait on top of that boundary has nothing to abstract
until Postgres is an actual, not hypothetical, second backend). Both are
mechanical refactors inside a single crate whenever a real second case shows
up — not worth the indirection today.

Meteora's `quote()` implementation internally walks multiple bins — real
remaining work, budget time for it. Orca's is done as a single-tick-range
approximation (see Phase 2 checklist); full tick-array crossing is deferred.

`PoolMetadata` and `PoolState` are stored separately: metadata almost never
changes (only on new-pool discovery), state changes on every relevant slot.
Mixing them into one struct means every state update forces you to think
about metadata invariants that didn't actually change.

## Data flow

```
Helius WS (accountSubscribe)
        │
        v
market-data:: decode raw account -> (PoolMetadata, PoolState)
        │
        ├──> recorder:: append raw update as a bincode-encoded, length-prefixed
        │                record to an append-only log file (off hot path,
        │                reuses serde — no bespoke wire format)
        │
        v
DashMap<Pubkey, PoolState>   (single source of truth, metadata cached separately)
        │
        ├─ broadcast::channel<PoolId>  (lightweight "changed" signal only)
        │
        v
scanner:: graph topology built ONCE at startup (+ refreshed every 5-30 min
          for new/delisted pools). On PoolId signal, look up
          HashMap<PoolId, Vec<EdgeIndex>> and re-price only those edges —
          never rebuild the whole graph.
        │
        v
route enumerator:: fixed 2-3 hop cycles, start/end bounded to configured
                   base tokens (SOL, USDC, USDT) — not any arbitrary token.
                   Bellman-Ford is not needed at this hop count; revisit only
                   if path length becomes unbounded.
        │
        v
simulator:: quote each hop with same DexAdapter as future executor
        │
        v
if net_profit > threshold -> Opportunity
        │
        v
storage (SQLite, async, off hot path) + api crate translates core::Event -> WS push
```

Key rules from the architecture review:
- The broadcast channel never carries pool state, only a `PoolId` changed
  signal. Consumers always re-read current state from the `DashMap`. This
  avoids `RecvError::Lagged` silently causing stale-price decisions.
- Topology and state are different lifecycles: build the graph once, update
  edge weights via the `PoolId -> Vec<EdgeIndex>` inverse index. Rebuilding
  the graph on every update was the biggest inefficiency in the first draft.
- Route enumeration replaces Bellman-Ford for now: with cycles bounded to
  2-3 hops AND bounded to base-token start/end, direct adjacency-list
  enumeration is cheaper and simpler than a general negative-cycle search.
  If hop count or start-token set grows unbounded later, Bellman-Ford
  becomes worth the complexity again — not before.
- Latency is instrumented from Phase 1, not added later: tag each stage
  (RPC receipt, decode, cache write, scanner, simulation) with a span/
  timestamp so bottlenecks are visible from day one, not discovered by guesswork.
- The recorder makes replay a first-class capability: same scanner and
  simulator code run against a recorded log or live RPC, no mainnet
  connection required to debug a specific historical window.
- **Multi-account pools, discovered mid-Phase-2 while decoding Raydium
  (confirmed against real mainnet data, not a hypothetical):** a pool's
  reserves are NOT in its config account — they're in separate SPL Token
  vault accounts, whose pubkeys aren't known until the config account is
  decoded. Consequences applied workspace-wide: `PoolState.reserve_a`/
  `reserve_b` are `Option<u64>` (not defaulted to 0 — "not loaded yet" and
  "genuinely empty" must stay distinguishable, or the scanner could quote
  against an unloaded pool), with `PoolState::is_ready()` gating whether a
  pool is usable; `DexAdapter::decode` returns `DecodedAccount::{PoolConfig,
  VaultAmount}` rather than assuming one account = one full pool state;
  `market-data::subscriptions::run` takes an `add_rx` channel so vault
  pubkeys can be subscribed to *after* their config account decodes,
  re-subscribing the full accumulated set on reconnect; `DexAdapter::quote`
  takes plain `reserve_in`/`reserve_out: u64` rather than `&PoolState` — the
  scanner resolves readiness and side-order once, before calling quote, not
  every DEX impl re-deriving it from an `Option`-laden state.
- **Second correction, found while decoding Orca (also confirmed against
  real mainnet data): not every DEX splits config from state the way Raydium
  does.** A Whirlpool account IS both — `liquidity`/`sqrt_price`/
  `tick_current_index` live inside the same account as the mints/vaults/fee,
  and change on every swap. This broke the just-established
  `quote(reserve_in, reserve_out, amount_in)` signature: concentrated
  liquidity has no "reserves" to pass in, it needs `liquidity`+`sqrt_price`.
  Reverted `DexAdapter::quote` to take `&PoolState` + `a_to_b: bool` again;
  added `DecodedAccount::ConfigWithLiveState` for DEXs where the config
  account IS the live state, alongside the existing `PoolConfig`/
  `VaultAmount` variants for DEXs where it isn't; `PoolCache::
  update_clmm_state` patches the cache on every re-decode of that account,
  not just once at discovery.
- **Third correction, applied before Meteora, not after:** the first pass at
  fixing the above added `sqrt_price`/`liquidity`/`tick_current_index` as
  more `Option<_>` fields on a shared flat `PoolState`. Right call to avoid
  cross-curve field explosion (`PoolState` would keep growing every DEX,
  and Meteora's bins don't fit as scalar fields at all): `PoolState.
  curve_state` is now `CurveState::{ConstantProduct{reserve_a,reserve_b},
  Whirlpool{sqrt_price,liquidity,tick_current_index}}` — one variant per
  curve shape, no field is ever `None` because it belongs to a different
  DEX. `reserve_a`/`reserve_b` stay `Option` *within* `ConstantProduct`
  (that optionality is real — the two vaults genuinely arrive at different
  times); `Whirlpool`'s three fields aren't `Option` at all, since they only
  ever arrive together in one decode. `PoolMetadata` stays a shared struct
  for now (Raydium and Orca need the exact same 7 fields) — enum-ize it only
  if Meteora's real config doesn't fit, not speculatively. **Resolved**:
  Meteora's `LbPair` also maps cleanly onto the same 7 fields (`vault_a`/
  `vault_b` = `reserve_x`/`reserve_y`) — the speculative case never
  materialized, confirming the wait-and-see call was right.

## Phase breakdown

### Phase 1 — Infrastructure ✅
- [x] Cargo workspace, `crates/core` (package `arb-core`) with domain types above (`PoolMetadata`/`PoolState` split)
- [x] Async runtime (tokio), config loading (RPC url, thresholds, base-token list), tracing/logging
- [x] Latency instrumentation scaffolding (span per stage: rpc → decode → cache → scanner → simulate)
- [x] CI: `cargo fmt --check` + `cargo clippy -D warnings` + `cargo test` on push (GitHub Actions)

### Phase 2 — Discovery + Market Data
- [x] `discovery::jupiter`: verified token list, live-tested against `https://lite-api.jup.ag/tokens/v2/tag?query=verified`
- [x] Per-DEX pool discovery — lives in each `dex-*` crate as `discover_pools(rpc_url)` (not in `discovery`, per the note that shipped with this crate: discovery logic belongs next to the decoder that knows the account shape). Shared `arb_core::rpc::get_program_accounts_by_size` (added here, not `discovery` — every `dex-*` crate already depends on `core`, none depend on `discovery`, so this is the only cycle-free shared home) calls `getProgramAccounts` filtered by exact account byte length. Live-tested: `dex-raydium::discover_pools` against real Helius returned **705,858 real Raydium pools** in ~30s — `getProgramAccounts` is NOT blocked on the Helius free tier (a real risk, many providers restrict it, checked rather than assumed). **Open finding, not solved here**: 705k pools is too many to decode/subscribe to wholesale every cycle — Phase 3 needs a filter (e.g. only pools touching a configured base token, or cross-referencing Jupiter's volume/TVL data) before deciding what to actually track, not "discover everything, subscribe to everything."
- [x] WS client (`market-data::subscriptions`): `accountSubscribe`/`accountNotification` per Solana's documented JSON-RPC shape, subscription-id -> pool_id mapping, reconnect with exponential backoff (1s -> 30s cap), keepalive ping every 60s (Helius drops idle WS after ~10min) — live-tested end-to-end against Helius mainnet with a real free-tier key (`cargo test -p market-data --test live_helius -- --ignored`)
- [x] Raydium AMM v4 decoder (`dex-raydium`): `AmmInfo` byte layout verified against the program source, live-tested against the real SOL/USDC pool (`58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2`) — decoded fee_bps/mints/vaults match reality, both vault balances non-zero and plausible. **Correction found here, applied workspace-wide**: `AmmInfo` does NOT store reserves — only vault pubkeys. `PoolMetadata` gained `vault_a`/`vault_b`; `PoolCache` now patches one reserve side at a time via a vault->pool_id index (`register_vaults`/`update_vault_amount`); `DexAdapter::decode` returns `DecodedAccount::{PoolConfig, VaultAmount}` instead of a single `(PoolMetadata, PoolState)` pair. Generic SPL Token balance reader lives in `arb_core::spl` (shared — Orca/Meteora vaults are also plain SPL accounts).
- [x] Orca Whirlpool decoder (`dex-orca`): `Whirlpool` Anchor account layout verified against the program source, live-tested against the real USDC/SOL pool (`HJPjoWUrhoZzkNfRpHuieeFk9WcZWjwy6PBjZ81ngndJ`) — decoded `sqrt_price` implies ~75.5 USDC/SOL after decimal adjustment, matching real market price; fee_bps/mints/vaults all correct, vault balances non-zero. `quote()` implements the single-tick-range (no tick-crossing) constant-liquidity swap formula as a deliberate Phase-2 approximation — full tick-array walking is real future work, not done here.
- [x] Meteora DLMM decoder (`dex-meteora`): `LbPair` bytemuck/`repr(C)` layout verified against the official Anchor IDL (every alignment gap is an explicit `_padding*` field, so offsets are a flat cumulative sum), fee/price formulas verified against the SDK's own source. Live-tested against the real SOL/USDC pool (`5XRqv7LCoC5FhWKk5JN8n4kCrJs3e4KH1XsYzKeMd5Nt`) — account length matches exactly (904 bytes), mints/vaults correct, implied price ~75.5 USDC/SOL (consistent with Orca's independently-decoded price for the same pair). **Known, deliberate gap, not fixed here**: real DLMM liquidity is bin-based (`BinArray` accounts, not decoded) — `quote()` uses a single pool-wide price from `active_id`/`bin_step` with zero slippage model, weaker than Raydium/Orca's approximations. Reused the `PoolConfig`/`VaultAmount`/`ConfigWithLiveState` shapes and `arb_core::spl` vault reader as-is — no new abstractions needed for a third DEX, confirming the `CurveState` enum redesign was the right call.
- [x] `DashMap<Pubkey, PoolState>` cache (`market-data::cache::PoolCache`) — pair index deferred to Phase 3 (scanner is the one that needs it to build the graph). Gained a second `DashMap<Pubkey, PoolMetadata>` (`register_metadata`/`get_metadata`/`pool_ids`) — needed once an actual orchestrator existed and had to look pools up by id; wasn't obvious until that wiring was written.
- [x] Recorder: bincode-encoded, length-prefixed append-only log (`market-data::recorder`), round-trip tested
- [x] **End-to-end wiring** (`apps/cli/src/pipeline.rs`): discovery → filter → subscribe → decode → cache, actually running as one process — this didn't exist even though every individual piece was tested in isolation. Byte-length dispatch (165/653/752/904) is enough to route a notification to the right decoder; no separate "which DEX is this pubkey" registry needed. Per-DEX subscription set differs on purpose: Raydium subscribes only to its 2 vaults (its config account has nothing pricing-relevant); Orca subscribes only to the Whirlpool account itself (vaults aren't needed for its pricing); Meteora subscribes to LbPair + both vaults (its config account IS live state, unlike Raydium's).
- [x] **Real, run-and-found finding, not assumed**: `discover_pools` + verified-token filter + base-token filter still left 33k pools / 85k accounts — running it live got the WS connection reset twice trying to subscribe to that many accounts on one connection. Jupiter's "verified" tag (4,318 tokens) is far broader than "liquid enough to matter." **Decision (user-chosen, see options weighed)**: ship a curated list of 3 known-good pools (the same ones already live-verified per-DEX) to validate the scanner end-to-end first; defer a real filter (liquidity/TVL threshold, most likely) to when it's needed to scale past the curated list — the `discover_pools`/verified-token code stays in place, just isn't wired into the default pipeline yet.
- [x] Live-tested the full pipeline for 45s against real Helius: stable connection (no drops, vs. two resets at 85k/106k accounts), 360 real Raydium vault updates + 2 real Orca Whirlpool state updates observed flowing into the cache.

### Phase 3 — Scanner ✅
- [x] Graph topology (`scanner::graph::PoolGraph`, `petgraph::UnGraph`): built from `PoolCache::pool_ids()`/`get_metadata()` on each scan tick (every 5s in the pipeline for now — periodic-refresh-for-new/delisted-pools timing is the same tick, not separately implemented yet, fine at 3-pool scale). Nodes are token mints, edges are pool ids.
- [x] Pool->edge lookup: `HashMap<Pubkey, EdgeIndex>`, not `Vec<EdgeIndex>` as originally sketched — a pool is always exactly one edge (one token pair), so a plain map is correct and simpler. Incremental edge-weight patching on a `PoolId` signal wasn't built separately: since routes are quoted on-demand from the live `PoolCache` (not from a cached numeric edge weight), there's no separate weight to keep in sync — the cache *is* the up-to-date source `quote()` reads from every tick.
- [x] Route enumeration (`scanner::routes::enumerate_cycles`): DFS bounded to 2-3 hops (config `max_hops`), starting/ending on a configured base token, no pool reused within one route. Unit-tested (parallel-pool 2-hop, triangle 3-hop, max_hops respected) AND live-verified: the curated 3-pool set produced exactly 12 routes (3 pools × 2 directions × 2 base tokens with a graph node — SOL and USDC; USDT isn't in the graph since no curated pool touches it — matches by hand).
- [x] Fee-aware, pre-slippage opportunity candidates (`scanner::detector::find_opportunities`): walks each route hop-by-hop through the same `DexAdapter::quote` a future executor would use, skips routes touching a not-`is_ready()` pool (doesn't quote as zero). Caught a real bug in its own tests while writing this — two tests registered vaults with pubkeys that didn't match the metadata's actual `vault_a`/`vault_b`, so `update_vault_amount` silently no-op'd and the tests "passed" for the wrong reason (pool never ready) rather than verifying real profit math. Fixed; the profitable-route test is now checked by hand (mispriced pool pair, ~9410 lamport profit on a 10,000 lamport trade).
- [x] Wired into `apps/cli/src/pipeline.rs`: a 5s scan tick rebuilds the graph, enumerates routes, and logs opportunities against the live cache. Live-verified for 40s against real Helius data: 0 opportunities found across the 3 most liquid mainnet SOL/USDC pools — the correct, expected answer at this sample size (real cross-DEX arbitrage this narrow gets closed in milliseconds by other bots), not a bug.

### Phase 4 — Research Engine (replay, PnL, strategy validation — not just "simulation")
- [x] **Prerequisite, resolved before the rest of Phase 4 could mean anything**: the curated 3-pool list wasn't enough to validate the strategy — needed real pool coverage. Two-stage filter, verified live against real Helius data (not assumed): (1) `discovery::jupiter::fetch_liquid_token_mints` — Jupiter's own `organicScoreLabel` (high/medium), free, same API call as the verified list, cuts 4,318 verified tokens to ~349 actually-liquid ones; (2) one-pool-per-`(Dex, token pair)` dedup in `apps/cli/src/pipeline.rs` — Meteora especially lets many bin-step/fee-tier variants of the same pair coexist, and liquidity-filtering alone still left 29,400 subscriptions. Combined: **1,419 pools, 2,986 subscriptions**, stable for 90s against real Helius (a hard `MAX_SUBSCRIPTIONS = 5_000` safety check now guards against silently repeating the earlier WS-reset mistake).
- [x] **Dedup upgraded from "first pool returned" to "deepest pool by real balance"**: the placeholder above picked whichever pool `getProgramAccounts` happened to return first per pair — not necessarily the most liquid. Fixed in `apps/cli/src/pipeline.rs` (`keep_deepest_by_vaults`/`keep_deepest_dlmm_by_vaults`/`keep_deepest_by_liquidity`): Orca ranks by `liquidity`, already decoded from the Whirlpool account during discovery (free, `dex-orca::discover_pools` now returns `Vec<(PoolMetadata, u128)>` instead of discarding it) — no extra RPC. Raydium and Meteora need real vault balances (not in their config accounts), fetched once per DEX via a new `arb_core::rpc::get_multiple_accounts_data` (batched `getMultipleAccounts`, chunked at 100) against every surviving candidate, then the pair keeps whichever candidate has the highest `reserve_a + reserve_b`. Bonus fix: `dex-meteora::discover_pools` now also returns the real `active_id`/`bin_step` (was decoded and discarded before) so the cache seeds DLMM pools with real values instead of a `0, 0` placeholder before the first WS notification arrives. Live-verified against real Helius: **1,420 pools, 2,984 subscriptions** (same order of magnitude as before — ranking changes *which* pool wins per pair, not how many pools survive), stable, WS subscribed cleanly, 25,702 routes/tick, 0 opportunities (expected). Cost: discovery startup time grew from near-instant to ~90s (Raydium ranking ~43s, Meteora ~39s) — the batched vault-balance RPC calls are the trade-off for real depth data; one-time at startup, not recurring.
- [ ] Considered and explicitly deferred (not needed yet, discovery isn't a bottleneck): using `getProgramAccounts` `memcmp` filters on known mint-offset bytes to fetch only pools for the ~349 liquid/base tokens instead of all pools then filtering client-side. Real RPC savings, but `getProgramAccounts` only ANDs filters (can't OR "mint is A OR mint is B"), so it would cost one RPC call per candidate token per DEX (~1,000+ calls) instead of 3 — worse on a rate-limited provider unless discovery time itself becomes a measured problem.
- [x] `MAX_SUBSCRIPTIONS` and the Jupiter `organicScoreLabel` allow-list moved from hardcoded constants to `config/default.toml`'s new `[discovery]` section (`accepted_organic_scores`, `max_subscriptions`) — both are pragmatic budget knobs, not claims about pool quality, so they shouldn't live in source. `discovery::jupiter::fetch_liquid_token_mints` now takes `accepted_labels: &[String]` instead of a hardcoded `"high"|"medium"` match.
- [ ] **Known, deliberately unresolved gap**: collapsing Meteora to one pool per `(token_a, token_b)` pair is a subscription-budget trade-off, not a market-structure truth. Different `bin_step` configs of the same pair are economically distinct instruments (different fee tiers, like Uniswap v3's 0.05%/0.3% pools) — they can be arbitraged *against each other*, and collapsing to the single deepest one throws that away permanently, with no way to notice a small/wide-spread pool was profitable. Correct fix is Research-Engine-driven (Phase 4): once real opportunity/PnL data exists, decide per-pair whether keeping >1 bin-step variant is worth the extra subscription cost — not a heuristic picked before that data exists. Not blocking Phase 4 from starting.
- [x] Replay engine against recorder log (offline): `apps/cli/src/bin/replay.rs`. `pipeline::run` now takes `discovery.record_dir` (config, unset by default) — when set, writes a one-time `PoolMetadata` snapshot (`pools.bin`) plus an append-only raw-update log (`updates.bin`, via the pre-existing but previously-unwired `market_data::recorder`). Decode-and-cache-update dispatch was pulled out of `pipeline.rs`'s inline match into `market_data::apply_update` — the one function both the live WS loop and the offline replay binary call, so they can't silently drift apart. Replay reconstructs the cache from the snapshot, replays every update through `apply_update`, and runs the scanner every N updates (default 500), persisting each `Opportunity` to SQLite via the new `storage` crate. Live-verified: recorded ~90s of real Helius traffic (1,426 pools, 1,770 updates), replayed it back — 0 opportunities (matches the live run), `storage::Db` validation queries (`avg_profit`, `profitable_count`) correct on both populated and empty tables.
      - **Bug found and fixed during this verification, not just a synthetic test**: killing the recording process abruptly (the normal way a live process stops — crash, restart, `SIGKILL`) leaves a partial trailing record (length prefix written, payload not fully flushed). `recorder::read_one` treated that as a hard I/O error instead of a clean end-of-stream, so replay crashed on any log from a non-gracefully-stopped run. Fixed: a mid-payload EOF is now treated the same as a between-records EOF (`Ok(None)`), with a regression test. This will happen on every real run — the recording process isn't going to shut down gracefully every time.
      - Observed once during live replay, not yet investigated: a 170-byte account (not 165/653/752/904) — falls through to the "unrecognized shape" branch and is logged+skipped, doesn't crash anything. Likely a Token-2022 vault with extension data appended past the base 165-byte SPL layout. Not blocking; noted for whenever Token-2022 pools matter.
- [x] PnL calculator reusing `DexAdapter::quote` (same code path as future executor): no new code needed — `scanner::detector::find_opportunities` already walks each route through `DexAdapter::quote` and produces `Opportunity::expected_profit`; both the live pipeline and the replay engine call the identical function. "PnL calculator" and "opportunity detector" are the same code path by design, not two implementations to keep in sync.
- [x] Slippage model, success-probability heuristic: `Opportunity` gained `max_price_impact_bps` (worst single-hop `Quote::price_impact_bps` across the route — real number every `DexAdapter::quote` already computed, just discarded before this) and `profit_margin_bps` (`expected_profit` as bps of `amount_in`). `Opportunity::is_fragile()` flags routes where `max_price_impact_bps >= profit_margin_bps` — the AMM's own quoted price impact already consumes the whole theoretical edge, so any real execution latency or size is expected to erase the profit. Deliberately a boolean red flag, not a fabricated numeric probability: there's no executor yet, so no real fill data exists to calibrate an actual success rate against — inventing a percentage now would be made up, not measured. `storage::Db` persists both fields plus `fragile`, and adds `profitable_and_not_fragile_count()` alongside the existing `profitable_count()` for a stricter, more honest read once real recorded data comes in.
- [x] Metrics defined now, even though nothing scrapes them yet: `arb_core::Metrics` expanded from 2 fields to all named in the original plan (`rpc_latency_ms`, `decode_latency_ms`, `cache_latency_ms`, `scanner_latency_ms`, `simulation_latency_ms`, `pool_updates_sec`, `opportunities_found`, `simulations_completed`, `false_positive_rate`). Actually measured, not stubbed: `market_data::apply_update` now returns `ApplyTiming { decode_latency_ms, cache_latency_ms }` (real `Instant` timing split around each branch's decode call vs. cache-write call); `pipeline.rs` accumulates these across each 5s tick and times `scanner_latency_ms` (graph build + route enumeration) and `simulation_latency_ms` (`find_opportunities`) directly; `rpc_latency_ms` covers the one-time discovery RPC total, logged once at "discovery complete", not per tick. `replay.rs` does the same per its `scan_every_n`-update window. Both log the full `Metrics` struct via `tracing` (`info!(?metrics, ...)`) — satisfies "defined now" without building an exporter (Phase 5 territory). Live-verified: real numbers observed (`scanner_latency_ms≈300ms` for ~26k routes, `simulation_latency_ms≈15ms`, `pool_updates_sec=16.2`, `decode_latency_ms`/`cache_latency_ms` sub-millisecond), not fabricated placeholders. `false_positive_rate` stays `Option<f64>` = `None` always — same reasoning as `Opportunity::is_fragile`: no executor exists yet, so there are no real fills to compare quotes against; the field name is reserved, not faked.
- [x] Validation: query SQLite directly (`avg(net_profit)`, `count(*) where net_profit > 0`) — done via `storage::Db::avg_profit`/`profitable_count`/`profitable_and_not_fragile_count`, exercised against the real 30-minute recording both before and after the Meteora fix (see below): 291/0 profitable-and-not-fragile pre-fix, 0/0 post-fix. Current honest answer: no positive edge demonstrated in this recorded window, on either DEX combination.
      — this answers "does the strategy work", no dashboard required to get there
- [x] **Validation, run against real data**: recorded 30 minutes of real Helius traffic (1,434 pools, ~35MB / 94,929 updates), replayed it. Result: **291 opportunities found, 0 profitable-and-not-fragile** (`profitable_and_not_fragile_count() == 0` out of 291 `expected_profit > 0` rows). `profit_margin_bps` ranged 0-51 (avg ~15 bps) while `max_price_impact_bps` ranged 1,974-7,648 (avg ~4,568 bps, i.e. ~46% average quoted impact on a 1 SOL trade) — the AMM's own quoted price impact dwarfs the theoretical edge on every single opportunity found. **Honest answer to the question this phase exists to answer: no real edge demonstrated with this architecture.**
- [x] **Root cause found and confirmed** (`apps/cli/src/bin/analyze_fragility.rs`, cross-referencing every persisted opportunity's route against the pool-metadata snapshot by DEX): **100% of the 291 opportunities touch Meteora** (164 Meteora+Orca, 127 Meteora+Raydium, 0 pure Raydium+Orca). Reading `dex-meteora`'s `quote()` (`crates/dex-meteora/src/lib.rs:109-122`) confirms why, not just correlation: `amount_out` is computed from a **flat pool-wide price with zero slippage** (`amount_in_after_fee * price`, completely independent of `reserve_in`) — the exact gap flagged since Phase 2 as "deliberate, not started." Meanwhile `price_impact_bps` is a *separate, unrelated* heuristic (`amount_in / (reserve_in + amount_in)`) that has no bearing on `amount_out` at all. Net effect: Meteora quotes assume infinite liquidity while simultaneously reporting (via the unrelated heuristic) that the real reserve is tiny relative to the trade — the ~46% average "impact" is confessing thin real liquidity that `amount_out` never accounted for. **These are not real tradeable opportunities — they're an artifact of an incomplete pricing model.** Executing any of these with real money would almost certainly return far less than quoted, likely a loss after real slippage. **Do not proceed to execution (Phase 6) using routes that touch Meteora until its `quote()` accounts for real depth** (decode `BinArray` accounts for actual bin liquidity, or at minimum make `amount_out` degrade with trade size the same way the impact heuristic already implies it should). Raydium+Orca alone found zero opportunities in 30 minutes — consistent with an efficient market where real mispricing this narrow gets closed in milliseconds by other bots, not evidence of a bug on those two DEXs.
- [x] **Fixed and re-verified against the same real data**: `dex-meteora::quote()` now discounts `amount_out` by its own `price_impact_bps` estimate instead of returning a flat, zero-slippage number — still an approximation (real per-bin `BinArray` liquidity isn't decoded, that remains real future work), but no longer internally contradicts its own impact estimate. Unit tests cover deep reserves (small discount, `amount_out` strictly `< ` the old flat-price value), shallow reserves (heavy discount — a trade 10x the reserve quotes near-zero, not near-full), and the not-yet-ready guard clause. **Re-ran the exact same 30-minute recorded log through `replay` after the fix: 291 opportunities -> 0.** Every previously-found "opportunity" was the pricing bug, not real mispricing — direct before/after proof on the same data, not just a unit-test claim.
- [x] **Real bin-depth model, not just the stopgap discount**: decoded Meteora's `BinArray` account for real per-bin liquidity, replacing the pool-wide-reserve heuristic wherever a fresh `BinArray` notification is available. Layout, `MAX_BIN_PER_ARRAY` (70 — the IDL's own doc comment claiming 600 is stale/wrong, the type definition and the SDK's `commons/src/constants.rs` agree on 70), the `bin_id_to_bin_array_index` formula, and the PDA seeds were all verified against the real `dlmm-sdk` source (cloned, not guessed), then live-verified against a real mainnet `BinArray` account (SOL/USDC, `active_id=-518` → `bin_array_index=-8`, derived PDA fetched and decoded correctly, real non-zero bin liquidity found). `quote()` now caps `amount_out` at the active bin's real liquidity when available (0% impact if the trade fully fits — correct, since DLMM bins are genuinely constant-price internally; not a bug this time), falling back to the reserve-based discount otherwise.
      - **Deliberately scoped, not a full swap simulator**: only the single bin at `active_id` is modeled — no cross-bin walk. Real DLMM swaps continue into adjacent bins once one is depleted, but simulating that correctly requires knowing the crossing direction precisely, and getting it wrong would produce a confidently-wrong number with real money on the line. Capping at the active bin's own liquidity can only *under*-state real fillable size, never overstate it — the safe direction to be wrong in. Real future work, not a silent gap.
      - **No dynamic re-subscription yet**: the pipeline subscribes to the `BinArray` covering `active_id` *as observed at discovery time*. If `active_id` drifts into a different array during a live run, quoting silently falls back to the reserve-based estimate for that pool until a fresh subscription is added — the `add_tx` channel `pipeline.rs` already has wired for exactly this is not yet used for it. Real future work.
      - Live-verified end-to-end in the full pipeline: subscriptions grew from 2,984 to 3,732 (one extra `BinArray` account per Meteora pool), WS subscribed cleanly, no crashes, no decode warnings for the new account size, `pool_updates_sec` ramping normally.

### Level 1 validation — independent pricing cross-check (2026-07-27)

Prompted by external review material arguing "if the pricing engine is wrong, every later statistic is worthless" — a real gap: every prior verification checked byte-decode correctness against real accounts, but never cross-checked `quote()`'s actual output against an independent pricing source. Built `apps/cli/src/bin/verify_pricing.rs`: queries Jupiter's quote API (`https://lite-api.jup.ag/swap/v1/quote`, confirmed live — the legacy `quote-api.jup.ag/v6` endpoint returns nothing now) restricted to one DEX at a time (`dexes=Raydium`/`Whirlpool`/`Meteora DLMM`, `onlyDirectRoutes=true`), reads the *exact* pool Jupiter routed through from `routePlan[0].swapInfo.ammKey`, fetches and decodes that same real pool ourselves, and compares `quote().amount_out` against Jupiter's `outAmount` for the same trade (1 SOL, SOL→USDC) — apples-to-apples against a real pool, not a hardcoded address that might not be the one Jupiter actually used.

- [x] **Raydium: `-0.0009%` diff** — effectively exact. Confirms the constant-product `quote()` is correct, not just the byte decode.
- [x] **Orca Whirlpool: `-0.0050%` diff** — also excellent, and the small remaining gap is expected (single-tick-range approximation, documented gap).
- [x] **Meteora DLMM: `-99.99%` diff** — `amount_out=6,144` vs Jupiter's real `75,738,224`. **Found and confirmed a severe practical limitation of the just-built bin-depth model**, not a new bug: this specific pool's *active* bin had little liquidity of its own, but the pool's real liquidity was concentrated in *neighboring* bins — exactly the cross-bin-walk gap already flagged as deliberately deferred (real future work, not a silent gap). The single-active-bin cap is conservative by design (can only under-state, never over-state, real fillable size — the safe direction for real money), but this result shows that conservatism can be extreme: **as it stands, the scanner is effectively blind to real Meteora liquidity whenever the active bin itself is thin, even if the pool overall is deep.** Practical implication: don't trust Meteora `quote()` magnitudes for real decisions yet beyond "roughly this direction" until the cross-bin walk is built — a bigger gap in practice than the unit tests alone suggested, exactly what this cross-check was for.

### Phase 4.5 — Replay tests
- [x] `tests/replay`: recorded slot -> replay through scanner+research -> assert expected opportunities. The `research` crate was an empty `cargo new` stub before this — its first real content is `research::replay`, the core replay loop (apply every recorded update via `market_data::apply_update`, run `scanner::find_opportunities` every `scan_every_n` updates) extracted out of `apps/cli/src/bin/replay.rs` so the binary and the test exercise the *same* function, not a hand-copied reimplementation that could drift. Persistence (SQLite) and logging stay the binary's concern, passed in as an `on_scan` callback — keeps the core loop testable with nothing but an in-memory `Cursor<Vec<u8>>`, no files, no live RPC.
- [x] Deterministic regression suite, independent of live RPC availability: `crates/research/src/replay.rs` tests. `replays_recorded_log_into_expected_opportunity` reuses the exact mispriced-two-pool numbers `scanner::detector`'s own test already validated by hand (pool_cheap 1:1, pool_expensive 2x — sell-high-buy-low round trip), but drives it through a real recorded log (`market_data::recorder::append`/`read_one` round-trip) instead of direct cache calls — this is what "replay through scanner+research" means end-to-end. `empty_log_applies_nothing_and_finds_no_opportunities` covers the zero-updates edge (the final scan must still run once). Both pass with zero network access.

### Phase 5 — Research & Monitoring UI (renamed from "Dashboard" — it's not an app to administer, it's a tool to answer "does this strategy have a real edge?")
- [ ] **Process architecture decision**: `crates/api`'s Axum server runs *in the same process* as `apps/cli`'s pipeline, not a separate service — no IPC/transport needed. `PoolCache` is already `Arc`-friendly (`DashMap` inside), shared directly between the scan loop and the API's request handlers. The pipeline emits `arb_core::Event::PoolUpdated`/`OpportunityFound`/`SimulationFinished`/`MetricsUpdated` (enum already exists, nothing currently publishes it) onto a `tokio::sync::broadcast` channel; each WS client gets its own `Receiver` off that channel. Explicitly deferred, not now: splitting into separate processes (would need a real transport — socket, file, Redis) — no benefit yet at this scale (2 users, one server); revisit only if the API process needs to scale/restart independently from the scanner.
- [ ] `crates/api`: REST (`/health`, `/metrics`, `/pools`, `/opportunities`, `/simulations`) + WebSocket pushing `core::Event`. Per-endpoint data source: `/health` trivial 200; `/metrics` reads a shared `Arc<RwLock<Metrics>>` updated each scan tick; `/pools` reads `PoolCache` directly (`pool_ids()` + `get_metadata`/`get`); `/opportunities` reads `storage::Db` (SQLite, historical — not in-memory); WS is a `broadcast::Receiver<Event>` per connection.
- [ ] Dashboard stack committed today: React 19 + TypeScript + Vite + Mantine + Zustand + TanStack Query + TanStack Table + Apache ECharts — evaluated against the codebase as of 2026-07-27 (`crates/api` still an empty stub, no dashboard app exists), locked in since nothing about the choice is contentious
- [ ] 4 pages, not 5: Overview, Pools, Opportunities, Research. Explicitly NOT built: a "watch a replay like it's live" page — that needs the backend to trigger and stream a replay job, real added scope, and doesn't serve the stated goal any better than the aggregate charts the Research page already covers (`avg_profit`, `profitable_and_not_fragile_count`, latency/slippage histograms). No CPU/RAM system metrics either — would need a new dependency (`sysinfo`), not justified yet.
- [ ] Pool health view (last update slot, staleness)
- [ ] **Deployment decision (2 users now, not 1 — needs auth)**: Oracle Cloud Free Tier VM (ARM, Ubuntu), one Docker container — Axum serves the REST/WS API *and* the built React static files, no separate dashboard container. SQLite persists via a mounted volume. Caddy in front for automatic TLS + **HTTP Basic Auth** (2 known users) — not a custom login system: no sessions/JWT/user table to build in Rust for 2 people, `basic_auth` is a 3-line Caddy directive. CI/CD (GitHub Actions + SSH deploy) explicitly deferred — nothing exists yet to automate deploying.

### Phase 6 — Execution (only after Phase 4 shows statistically significant edge)
- [ ] `executor/` crate created at this point, not before
- [ ] Wallet management, atomic multi-leg transaction builder
- [ ] Dynamic priority fee estimator
- [ ] Jito bundle submission
- [ ] Risk limits: max position size, daily loss cap, per-token exposure

#### Research notes for this phase (2026-07-27, from a vendor blog post — read critically, not as fact)

Reviewed 3 articles from rpcfast.com/Dysnix (a company selling dedicated Solana RPC infra) about Solana arb and liquidation bots. **The specific benchmark numbers in them (380ms/110ms/38ms latency, 96% bundle landing rate, 8x more profitable attempts, etc.) are the vendor's own self-reported marketing numbers for their own product — not independently verified, not to be treated as real capacity-planning inputs.** The general Solana/MEV mechanics they describe are standard, well-known, and worth keeping for Phase 6 design:

- **Geyser gRPC (Yellowstone) vs. WebSocket for account streaming**: real distinction — Geyser pushes updates from validator memory, WebSocket (what this project uses today, via Helius) is slower and can miss updates under load. Not relevant *yet*: we're still proving there's an edge to capture (Phase 4). Worth evaluating when Phase 6 execution latency actually starts mattering — premature before that, per this project's own "don't build ahead" pattern.
- **`processed` commitment, not `confirmed`/`finalized`, for reading state in a latency-sensitive path** — real tradeoff (freshest-but-riskier vs. safer-but-stale). Worth explicitly checking what commitment level Helius's WS subscriptions default to before Phase 6, not assuming.
- **Simulate before submitting** (`simulateTransaction`) — cheap, catches routes that looked profitable at quote time but changed by execution time. Directly applicable design point for whatever `executor/` ends up building — simulate every leg before spending a real Jito tip.
- **Jito tip economics — the one number worth taking seriously despite the vendor framing**: bots reportedly pay 50-60% of *expected* profit as tip to win bundle inclusion. Cross-referenced against **our own real, measured data** (not the vendor's): Phase 4's 30-minute recording showed real `profit_margin_bps` in the 0-51 range (avg ~15 bps) even before the Meteora bug was found. If tips alone claim half of an already-thin margin, most of what this scanner would find is likely unprofitable after tips — a real strategic risk to keep in view once Phase 6 is on the table, not just a vendor talking point.
- **Subscribe only to the specific accounts needed, filtered server-side, not broadcast-and-filter-client-side** — this project already does this correctly (per-pool `accountSubscribe`, not a firehose). Nothing to change; good confirmation the existing design matches known best practice.
- **Multi-region parallel bundle submission** (US East/EU/Tokyo) — a real Phase 6 execution detail, not relevant before then.
- **Liquidation-bot article (Kamino/MarginFi/Drift/Save health-factor liquidations) is a different strategy entirely, not applicable to this project** — this bot does cross-DEX arbitrage, not lending-protocol liquidations. Kept only because its core thesis ("infrastructure decides the outcome once the logic is commoditized, not clever strategy") is the same lesson as the arb articles, reinforced from an unrelated MEV niche.
- **Metrics named in these articles worth having before going live** (route detection latency, tx construction time, bundle acceptance rate, revert rate) mostly already exist here: `scanner_latency_ms`/`simulation_latency_ms` are the equivalent of "route detection latency", already measured (Phase 4). Bundle acceptance rate and revert rate don't exist yet because there's no executor — add them when `executor/` is built, not before.

## Technology stack

### Backend (locked in, needed from Phase 1)

| Crate | Purpose |
|---|---|
| `tokio` | Async runtime |
| `solana-sdk`, `solana-client` | Solana primitives + RPC |
| `tokio-tungstenite` | WebSocket client (Helius) |
| `reqwest` | HTTP client (Jupiter API) |
| `dashmap` | Concurrent pool cache |
| `serde` | Serialization |
| `tracing` | Structured logging + latency spans (Phase 1 requirement) |
| `sqlx` (SQLite) | Async storage, cheap migration path to Postgres later |
| `config` | Layered config: `config/default.toml`, `local.toml`, `production.toml` |
| `bincode` | Recorder wire format — reuses `serde`, no bespoke binary spec |

`tonic` (gRPC) is a **future** dependency for Yellowstone/Geyser — not added
until that data source is actually built, not before.

### Frontend + observability — decide at Phase 5/6, not now

Committing these dependencies today would mean building dashboard pages and
an observability platform before the scanner has proven a single profitable
opportunity. Candidates to evaluate when each phase actually starts:

- **Dashboard app:** React + TypeScript + Vite. UI library (Mantine vs
  Tailwind-only vs Ant Design), charts (Apache ECharts vs lighter
  alternatives), tables (TanStack Table), state (Zustand), data fetching
  (TanStack Query) — pick based on what Phase 5's actual (small) page set
  needs, not the full 9-page vision up front.
- **Observability:** Prometheus + Grafana + OpenTelemetry are real options
  for Phase 6 (live execution monitoring). Until then, `tracing` writing
  spans to SQLite is sufficient — no extra infrastructure to run.
- **Local dev orchestration:** revised — Docker Compose adopted from day one
  (see Deployment section below), justified by a concrete need (consistent
  environment for two collaborators), not by speculative future scale.

## Deployment (Milestone 1)

### Containers

Two services, no more:

```
docker-compose.yml
├── backend    # arb-bot binary: market-data + scanner + research + api
│               # SQLite on a named volume
└── dashboard  # React build, served statically (nginx or the api crate itself)
```

`git clone && cp .env.example .env && docker compose up` is the whole setup
for a second developer — no local Rust/Node/SQLite install required.

### Hosting — verified, not assumed

Two claims from an earlier draft turned out to be stale as of mid-2026 and
were dropped after checking:

- **Fly.io is not a free option anymore.** New accounts get a 2-hour/7-day
  trial, then usage-based billing from the first resource — not a fit for
  "free 24/7 scanner."
- **Oracle Cloud Always Free still exists but is not a reliable primary
  plan.** The ARM tier was quietly cut from 4 OCPU/24GB to 2 OCPU/12GB
  (~June 2026), and new-account signup frequently hits fraud-review holds or
  "out of host capacity" errors in the free ARM shape — can take days of
  retries or an outright rejection.

**Recommendation:** Hetzner CPX11 (~€4/month) as the reliable base — no
approval gate, no capacity lottery. Try Oracle Always Free in parallel as a
free bonus if the account clears review; if it doesn't, Hetzner already
works, nothing blocked. Same `docker compose up` runs on either.

Railway/Render free tiers: correctly ruled out — they sleep on inactivity
and don't suit a WS connection that needs to stay open 24/7.

### Process + security

- `systemd` restarts the container/binary on crash; `journalctl` for logs —
  no ELK/Loki needed at this stage.
- Dashboard is never exposed raw on the internet — basic auth or a reverse
  proxy with auth in front, even for Milestone 1. It exposes live strategy
  and profit data once the scanner is working.
- CI: GitHub Actions runs `cargo fmt`/`clippy`/`test`, builds the Docker
  image, pushes to a registry. Deploy step (SSH + `docker compose pull && up`,
  or a registry webhook) added once the VPS is provisioned — not before.

## Status

Phases 1-3 done: `discovery::jupiter`, `market-data` (subscriptions/cache/
recorder), `dex-raydium`, `dex-orca`, `dex-meteora` (pricing + per-DEX
`discover_pools`), `scanner` (graph/routes/detector), and
`apps/cli/src/pipeline.rs` (the orchestrator running all of it as one
process, now including a 5s scan tick) — all implemented and live-tested
against real mainnet data. 33 unit tests passing, 7 live/network tests
runnable with `--ignored`. The full pipeline ran 40s stable against real
Helius data producing 12 real candidate routes and correctly finding 0
profitable opportunities among the 3 most liquid mainnet SOL/USDC pools.

Known, explicitly flagged gaps carried forward, not silently missing:
Orca's tick-crossing (single-tick-range for now), Meteora's bin-array depth
(single pool-wide price, zero slippage model for now), the pipeline running
on a curated 3-pool list rather than real discovery (scaling deferred until
the scanner is proven, per the tradeoff chosen when 33k-pool/85k-account
discovery got the WS connection reset twice), and periodic topology refresh
sharing the 5s scan tick rather than its own cadence.

Next: Phase 4 (Research Engine) — replay, PnL validation, and the actual
question this whole project exists to answer: does the strategy find
profitable opportunities consistently, once the curated list grows past 3
pools.
