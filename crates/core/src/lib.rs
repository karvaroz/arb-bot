pub mod events;
pub mod pubkey_json;
pub mod rpc;
pub mod spl;

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    pub mint: Pubkey,
    pub decimals: u8,
    pub symbol: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Dex {
    Raydium,
    Orca,
    Meteora,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Curve {
    ConstantProduct,
    Whirlpool,
    Dlmm,
}

/// Which side of a pool a vault/reserve update belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    A,
    B,
}

/// Immutable, set once at pool discovery.
///
/// `vault_a`/`vault_b` are here, not just `token_a`/`token_b`: most AMMs
/// (Raydium included) keep the pool's config in one account and the actual
/// reserves in two separate SPL Token vault accounts. `vault_a`/`vault_b`
/// are what market-data subscribes to next, after decoding this account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolMetadata {
    pub id: Pubkey,
    pub dex: Dex,
    pub curve: Curve,
    pub token_a: Pubkey,
    pub token_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub fee_bps: u16,
}

/// Per-curve pricing state — one variant per curve shape, not a flat struct
/// of `Option<_>` fields. Constant-product and concentrated-liquidity need
/// genuinely different data (reserves vs. liquidity+sqrt_price), and a bin-
/// based DEX (Meteora) needs a collection (active bin + surrounding bins),
/// which wouldn't fit as scalar fields on a shared struct anyway. Adding a
/// DEX means adding a variant, not adding more `Option<_>` fields that are
/// `None` for every other curve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CurveState {
    /// `reserve_a`/`reserve_b` start `None` and each side is set
    /// independently as its own vault account's balance arrives — the two
    /// vaults are separate accounts, updated by separate notifications.
    /// Deliberately NOT defaulted to 0: a missing reading and a genuinely
    /// empty vault must stay distinguishable.
    ConstantProduct {
        reserve_a: Option<u64>,
        reserve_b: Option<u64>,
    },
    /// Orca Whirlpool: these three always arrive together (same account,
    /// same decode), so no `Option` needed here — the pool simply isn't in
    /// the cache yet until the first successful decode.
    Whirlpool {
        sqrt_price: u128,
        liquidity: u128,
        tick_current_index: i32,
    },
    /// Meteora DLMM: `active_id`/`bin_step` arrive together (same account
    /// as config, like Whirlpool) and are enough to derive price
    /// (`(1 + bin_step/10000)^active_id`). `reserve_a`/`reserve_b` are the
    /// pool's two vaults, same asynchronous-arrival reasoning as
    /// `ConstantProduct`.
    ///
    /// `nearby_bins` is real `(bin_id, amount_x, amount_y)` data decoded from
    /// whichever `BinArray` accounts have reported in — see `dex-meteora` for
    /// the byte layout (verified against the official IDL) and the swap walk
    /// that crosses these bins in the economically-determined direction
    /// (selling more of a token always moves price against the seller — not
    /// a guess, the same invariant every correct AMM already encodes).
    /// Empty until the first `BinArray` notification arrives for a bin near
    /// `active_id` — quoting falls back to the reserve-based estimate until
    /// then. Not exhaustive: only as many bins as have been subscribed to
    /// and decoded, pruned to a window around `active_id` (see
    /// `market_data::cache`) — a trade that would need more depth than this
    /// window holds is quoted with whatever's known, capped, not guessed.
    Dlmm {
        active_id: i32,
        bin_step: u16,
        reserve_a: Option<u64>,
        reserve_b: Option<u64>,
        nearby_bins: Vec<(i32, u64, u64)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolState {
    pub curve_state: CurveState,
    pub last_slot: u64,
}

impl PoolState {
    /// The scanner must check this before quoting a pool — meaning differs
    /// per curve (both vault sides loaded vs. always-true for Whirlpool).
    pub fn is_ready(&self) -> bool {
        match &self.curve_state {
            CurveState::ConstantProduct {
                reserve_a,
                reserve_b,
            } => reserve_a.is_some() && reserve_b.is_some(),
            CurveState::Whirlpool { .. } => true,
            CurveState::Dlmm {
                reserve_a,
                reserve_b,
                ..
            } => reserve_a.is_some() && reserve_b.is_some(),
        }
    }
}

/// What a single decoded account turned out to be — a DEX's `decode()` can't
/// always return a full `PoolState` from one account, since reserves often
/// live in accounts separate from the pool config. A decoder only knows how
/// to read bytes, not which pool/side a vault belongs to — that mapping
/// comes from the `PoolConfig` decoded earlier (`vault_a`/`vault_b`), and is
/// the caller's (market-data's) bookkeeping, not the decoder's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecodedAccount {
    /// Raydium-style: config account, immutable, decoded once at discovery.
    PoolConfig(PoolMetadata),
    /// A vault account's balance — caller maps it to a pool/side via the
    /// vault index it built from an earlier `PoolConfig`.
    VaultAmount(u64),
    /// Orca Whirlpool-style: the "config" account IS the live state too
    /// (liquidity/sqrt_price/tick change on every swap), so every
    /// notification carries both parts — no separate vault-only update path
    /// for these fields. Carries `CurveState`, not a full `PoolState`:
    /// `last_slot` is the cache's concern to stamp, not the decoder's.
    ConfigWithLiveState(PoolMetadata, CurveState),
    /// Meteora-specific: a `BinArray` account, decoded into every bin's
    /// `(amount_x, amount_y)`. `lb_pair` is the pool id this belongs to —
    /// embedded in the account itself (unlike a vault, no separate index is
    /// needed to know which pool a `BinArray` update is for).
    MeteoraBinArray {
        lb_pair: Pubkey,
        bin_array_index: i64,
        bins: Vec<(u64, u64)>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    pub amount_out: u64,
    pub fee: u64,
    pub price_impact_bps: u16,
}

/// Implemented once per DEX (Raydium/Orca/Meteora) — decode + pricing only.
/// Subscription is deliberately NOT part of this trait: `accountSubscribe` is
/// the same RPC call regardless of DEX, so it belongs to `market-data`.
pub trait DexAdapter {
    /// Distinguishes a pool-config account from a vault account by shape
    /// (e.g. exact byte length) — see the impl for the specifics of each DEX.
    fn decode(&self, account_data: &[u8]) -> Option<DecodedAccount>;

    /// Takes the full `PoolState`: constant-product and concentrated-
    /// liquidity DEXs need genuinely different fields from it (reserves vs.
    /// sqrt_price+liquidity) — a flat `(reserve_in, reserve_out)` pair can't
    /// represent both. The scanner still must check `PoolState::is_ready()`
    /// before calling this.
    fn quote(&self, meta: &PoolMetadata, state: &PoolState, amount_in: u64, a_to_b: bool) -> Quote;
}

/// 2-3 hops, starts/ends on a configured base token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    #[serde(with = "crate::pubkey_json::vec")]
    pub pools: Vec<Pubkey>,
    #[serde(with = "crate::pubkey_json::vec")]
    pub tokens: Vec<Pubkey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opportunity {
    pub route: Route,
    pub expected_profit: i64,
    pub slot: u64,
    /// Worst single-hop `Quote::price_impact_bps` across the route — a real
    /// number each `DexAdapter::quote` already computes (exact for
    /// constant-product, an approximation for Orca/Meteora's single-tick/bin
    /// model — see their module docs), just not surfaced past the quote loop
    /// before this.
    pub max_price_impact_bps: u16,
    /// `expected_profit` as basis points of `amount_in` — the denominator
    /// `is_fragile` compares `max_price_impact_bps` against.
    pub profit_margin_bps: i32,
}

impl Opportunity {
    /// True when the route's own quoted price impact already consumes the
    /// entire theoretical edge — meaning any execution slower than instant,
    /// or a real trade instead of a quote, is expected to erase the profit.
    /// This is a heuristic grounded in numbers already computed by real
    /// quote math, not a fabricated probability: there is no execution data
    /// yet to calibrate an actual success rate against (no executor exists),
    /// so this stays a boolean red flag, not a percentage, until Phase 6
    /// gives it something real to check against.
    pub fn is_fragile(&self) -> bool {
        i32::from(self.max_price_impact_bps) >= self.profit_margin_bps
    }
}

/// One snapshot per scan tick (live pipeline or replay) — logged via
/// `tracing`, not scraped by anything yet (no metrics exporter exists;
/// IMPLEMENTATION_PLAN.md Phase 5 is where that would live). `f64`, not
/// `Eq`-able, since these are real measured durations/rates, not counts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Metrics {
    /// Discovery-time RPC round-trips only (`getProgramAccounts`,
    /// `getMultipleAccounts`) — one-time at startup, not per tick.
    pub rpc_latency_ms: f64,
    /// Average per-update decode time (byte parsing) since the last tick.
    pub decode_latency_ms: f64,
    /// Average per-update cache-write time since the last tick.
    pub cache_latency_ms: f64,
    /// Time to rebuild the graph + enumerate routes this tick.
    pub scanner_latency_ms: f64,
    /// Time spent quoting routes (`find_opportunities`) this tick.
    pub simulation_latency_ms: f64,
    pub pool_updates_sec: f64,
    pub opportunities_found: u64,
    pub simulations_completed: u64,
    /// Always `None` for now — computing a real false-positive rate needs
    /// actual fills to compare quotes against, and no executor exists yet
    /// (see `Opportunity::is_fragile` for the same reasoning). The name is
    /// reserved here so nothing has to change shape when Phase 6 makes this
    /// measurable.
    pub false_positive_rate: Option<f64>,
}
