//! Route enumeration: fixed 2-3 hop cycles, bounded to configured base
//! tokens as start/end — not Bellman-Ford. See IMPLEMENTATION_PLAN.md for
//! why direct enumeration is cheaper at this scale, and revisit only if hop
//! count or the base-token set becomes unbounded.

use crate::graph::PoolGraph;
use petgraph::graph::{EdgeIndex, NodeIndex};
use solana_sdk::pubkey::Pubkey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteCandidate {
    /// Pool ids in hop order.
    pub pools: Vec<Pubkey>,
    /// Token path, length `pools.len() + 1`: starts and ends on the same
    /// base token.
    pub tokens: Vec<Pubkey>,
}

/// Every simple cycle of length 2..=`max_hops` that starts and ends on one
/// of `base_tokens`. "Simple" here means no pool is reused within one
/// route — reusing the same pool twice can't be a real arbitrage leg.
pub fn enumerate_cycles(
    graph: &PoolGraph,
    base_tokens: &[Pubkey],
    max_hops: usize,
) -> Vec<RouteCandidate> {
    let mut routes = Vec::new();
    for &start_token in base_tokens {
        let Some(start_node) = graph.token_node(&start_token) else {
            continue;
        };
        let mut edges_used = Vec::new();
        let mut token_path = vec![start_token];
        dfs(
            graph,
            start_node,
            start_node,
            max_hops,
            &mut edges_used,
            &mut token_path,
            &mut routes,
        );
    }
    routes
}

fn dfs(
    graph: &PoolGraph,
    start_node: NodeIndex,
    current_node: NodeIndex,
    max_hops: usize,
    edges_used: &mut Vec<EdgeIndex>,
    token_path: &mut Vec<Pubkey>,
    out: &mut Vec<RouteCandidate>,
) {
    if edges_used.len() == max_hops {
        return;
    }

    for (edge_id, next_node, _pool_id) in graph.edges_from(current_node) {
        if edges_used.contains(&edge_id) {
            continue; // no reusing the same pool within one route
        }

        edges_used.push(edge_id);
        token_path.push(graph.token_at(next_node));

        if next_node == start_node && edges_used.len() >= 2 {
            out.push(RouteCandidate {
                pools: edges_used.iter().map(|&e| graph.pool_id_of(e)).collect(),
                tokens: token_path.clone(),
            });
        }

        dfs(
            graph, start_node, next_node, max_hops, edges_used, token_path, out,
        );

        token_path.pop();
        edges_used.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::PoolGraph;
    use arb_core::{Curve, Dex, PoolMetadata};

    fn meta(id: Pubkey, token_a: Pubkey, token_b: Pubkey) -> PoolMetadata {
        PoolMetadata {
            id,
            dex: Dex::Raydium,
            curve: Curve::ConstantProduct,
            token_a,
            token_b,
            vault_a: Pubkey::new_unique(),
            vault_b: Pubkey::new_unique(),
            fee_bps: 25,
        }
    }

    #[test]
    fn finds_two_hop_cycle_through_parallel_pools() {
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let pool_raydium = Pubkey::new_unique();
        let pool_orca = Pubkey::new_unique();

        let pools = vec![meta(pool_raydium, sol, usdc), meta(pool_orca, sol, usdc)];
        let graph = PoolGraph::build(&pools);

        let routes = enumerate_cycles(&graph, &[sol], 3);

        assert!(
            !routes.is_empty(),
            "two parallel SOL/USDC pools should yield a 2-hop cycle"
        );
        let two_hop = routes
            .iter()
            .find(|r| r.pools.len() == 2)
            .expect("expected a 2-hop route");
        assert_eq!(two_hop.tokens.first(), Some(&sol));
        assert_eq!(two_hop.tokens.last(), Some(&sol));
        assert_ne!(
            two_hop.pools[0], two_hop.pools[1],
            "must use two different pools"
        );
    }

    #[test]
    fn finds_three_hop_triangle() {
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let bonk = Pubkey::new_unique();
        let p1 = Pubkey::new_unique();
        let p2 = Pubkey::new_unique();
        let p3 = Pubkey::new_unique();

        let pools = vec![
            meta(p1, sol, usdc),
            meta(p2, usdc, bonk),
            meta(p3, bonk, sol),
        ];
        let graph = PoolGraph::build(&pools);

        let routes = enumerate_cycles(&graph, &[sol], 3);
        let triangle = routes.iter().find(|r| r.pools.len() == 3);
        assert!(triangle.is_some(), "SOL->USDC->BONK->SOL should be found");
    }

    #[test]
    fn respects_max_hops() {
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let bonk = Pubkey::new_unique();
        let p1 = Pubkey::new_unique();
        let p2 = Pubkey::new_unique();
        let p3 = Pubkey::new_unique();

        let pools = vec![
            meta(p1, sol, usdc),
            meta(p2, usdc, bonk),
            meta(p3, bonk, sol),
        ];
        let graph = PoolGraph::build(&pools);

        let routes = enumerate_cycles(&graph, &[sol], 2);
        assert!(
            routes.iter().all(|r| r.pools.len() <= 2),
            "max_hops=2 must not return 3-hop routes"
        );
    }
}
