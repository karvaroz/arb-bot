use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use crate::{Metrics, Opportunity};

/// The one event type scanner, research, api, dashboard and cli all share.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    PoolUpdated {
        #[serde(with = "crate::pubkey_json")]
        pool_id: Pubkey,
        slot: u64,
    },
    RouteCandidate {
        route_id: u64,
        slot: u64,
    },
    OpportunityFound(Opportunity),
    SimulationFinished {
        opportunity: Opportunity,
        actual_profit: i64,
    },
    MetricsUpdated(Metrics),
}
