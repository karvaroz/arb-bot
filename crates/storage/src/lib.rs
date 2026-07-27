//! SQLite persistence for opportunities found by the scanner (live or
//! replay) — the "does the strategy work" question is answered by querying
//! this directly (`avg(net_profit)`, `count(*) where net_profit > 0`), not
//! by building a dashboard first (IMPLEMENTATION_PLAN.md Phase 4).

use arb_core::Opportunity;
use rusqlite::{Connection, params};
use solana_sdk::pubkey::Pubkey;

pub struct Db {
    conn: Connection,
}

/// One persisted opportunity, with `route_pools` already parsed back into
/// real `Pubkey`s — used by offline analysis (e.g. "do fragile opportunities
/// cluster on a specific DEX?"), not by the hot path.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OpportunityRow {
    pub slot: u64,
    #[serde(with = "arb_core::pubkey_json::vec")]
    pub route_pools: Vec<Pubkey>,
    pub expected_profit: i64,
    pub profit_margin_bps: i32,
    pub max_price_impact_bps: u16,
    pub fragile: bool,
}

impl Db {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS opportunities (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                slot INTEGER NOT NULL,
                route_pools TEXT NOT NULL,
                expected_profit INTEGER NOT NULL,
                profit_margin_bps INTEGER NOT NULL,
                max_price_impact_bps INTEGER NOT NULL,
                fragile INTEGER NOT NULL
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    pub fn insert_opportunity(&self, opp: &Opportunity) -> anyhow::Result<()> {
        let route_pools = serde_json::to_string(&opp.route.pools)?;
        self.conn.execute(
            "INSERT INTO opportunities
                (slot, route_pools, expected_profit, profit_margin_bps, max_price_impact_bps, fragile)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                opp.slot as i64,
                route_pools,
                opp.expected_profit,
                opp.profit_margin_bps,
                opp.max_price_impact_bps,
                opp.is_fragile(),
            ],
        )?;
        Ok(())
    }

    /// Same as `profitable_count`, but excludes routes where the AMM's own
    /// quoted price impact already consumed the entire theoretical edge
    /// (see `Opportunity::is_fragile`) — a rougher but more honest read of
    /// "how many of these would likely survive real execution."
    pub fn profitable_and_not_fragile_count(&self) -> anyhow::Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM opportunities WHERE expected_profit > 0 AND fragile = 0",
            [],
            |r| r.get(0),
        )?)
    }

    /// `None` when the table is empty — `AVG` over zero rows is SQL NULL,
    /// not 0, and the two mean different things (no data vs. break-even).
    pub fn avg_profit(&self) -> anyhow::Result<Option<f64>> {
        Ok(self
            .conn
            .query_row("SELECT AVG(expected_profit) FROM opportunities", [], |r| {
                r.get(0)
            })?)
    }

    pub fn profitable_count(&self) -> anyhow::Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM opportunities WHERE expected_profit > 0",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn total_count(&self) -> anyhow::Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM opportunities", [], |r| r.get(0))?)
    }

    /// Every row, with `route_pools` parsed back into `Pubkey`s — for
    /// offline analysis only (e.g. correlating `fragile` against which DEXs
    /// a route touches). Not paginated: fine for analysis-sized result sets,
    /// not meant for a hot path.
    pub fn all_rows(&self) -> anyhow::Result<Vec<OpportunityRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT slot, route_pools, expected_profit, profit_margin_bps, max_price_impact_bps, fragile
             FROM opportunities",
        )?;
        let rows = stmt.query_map([], |r| {
            let slot: i64 = r.get(0)?;
            let route_pools_json: String = r.get(1)?;
            let expected_profit: i64 = r.get(2)?;
            let profit_margin_bps: i32 = r.get(3)?;
            let max_price_impact_bps: i64 = r.get(4)?;
            let fragile: bool = r.get(5)?;
            Ok((
                slot,
                route_pools_json,
                expected_profit,
                profit_margin_bps,
                max_price_impact_bps,
                fragile,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (
                slot,
                route_pools_json,
                expected_profit,
                profit_margin_bps,
                max_price_impact_bps,
                fragile,
            ) = row?;
            let route_pools: Vec<Pubkey> = serde_json::from_str(&route_pools_json)?;
            out.push(OpportunityRow {
                slot: slot as u64,
                route_pools,
                expected_profit,
                profit_margin_bps,
                max_price_impact_bps: max_price_impact_bps as u16,
                fragile,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arb_core::Route;

    fn opp(slot: u64, expected_profit: i64) -> Opportunity {
        opp_with_impact(slot, expected_profit, 0)
    }

    fn opp_with_impact(slot: u64, expected_profit: i64, max_price_impact_bps: u16) -> Opportunity {
        Opportunity {
            route: Route {
                pools: vec![],
                tokens: vec![],
            },
            expected_profit,
            slot,
            max_price_impact_bps,
            // Test convention: treat `expected_profit` as already expressed
            // in bps (as if amount_in were 10_000), matching detector.rs's tests.
            profit_margin_bps: expected_profit as i32,
        }
    }

    #[test]
    fn empty_db_has_no_average() {
        let db = Db::open(":memory:").unwrap();
        assert_eq!(db.avg_profit().unwrap(), None);
        assert_eq!(db.profitable_count().unwrap(), 0);
        assert_eq!(db.total_count().unwrap(), 0);
    }

    #[test]
    fn computes_average_and_profitable_count() {
        let db = Db::open(":memory:").unwrap();
        db.insert_opportunity(&opp(1, 100)).unwrap();
        db.insert_opportunity(&opp(2, -50)).unwrap();
        db.insert_opportunity(&opp(3, 200)).unwrap();

        assert_eq!(db.total_count().unwrap(), 3);
        assert_eq!(db.profitable_count().unwrap(), 2);
        assert_eq!(db.avg_profit().unwrap(), Some(250.0 / 3.0));
    }

    #[test]
    fn fragile_opportunities_are_excluded_from_the_stricter_count() {
        let db = Db::open(":memory:").unwrap();
        // Profitable but the AMM's own price impact already ate the whole edge.
        db.insert_opportunity(&opp_with_impact(1, 100, 150))
            .unwrap();
        // Profitable with real headroom over its price impact.
        db.insert_opportunity(&opp_with_impact(2, 100, 10)).unwrap();

        assert_eq!(db.profitable_count().unwrap(), 2);
        assert_eq!(db.profitable_and_not_fragile_count().unwrap(), 1);
    }
}
