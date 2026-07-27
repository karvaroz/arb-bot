//! Graph topology: built once from pool metadata, not rebuilt per update —
//! per IMPLEMENTATION_PLAN.md, topology (which tokens/pools exist) and state
//! (reserves/price) are different lifecycles. Nodes are token mints, edges
//! are pools (undirected: a pool prices both directions via `a_to_b`).

use arb_core::PoolMetadata;
use petgraph::graph::{EdgeIndex, NodeIndex, UnGraph};
use petgraph::visit::EdgeRef;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;

pub struct PoolGraph {
    graph: UnGraph<Pubkey, Pubkey>,
    token_index: HashMap<Pubkey, NodeIndex>,
    /// pool_id -> the one edge that pool is. A pool is always exactly one
    /// edge (one pair of tokens), so this is a plain map, not `Vec<_>`.
    pool_edge: HashMap<Pubkey, EdgeIndex>,
}

impl PoolGraph {
    /// Built once at startup (or on a periodic discovery refresh) from
    /// whatever pools the cache currently knows about.
    pub fn build(pools: &[PoolMetadata]) -> Self {
        let mut graph = UnGraph::new_undirected();
        let mut token_index = HashMap::new();
        let mut pool_edge = HashMap::new();

        let node_for = |graph: &mut UnGraph<Pubkey, Pubkey>,
                        token_index: &mut HashMap<Pubkey, NodeIndex>,
                        mint: Pubkey| {
            *token_index
                .entry(mint)
                .or_insert_with(|| graph.add_node(mint))
        };

        for meta in pools {
            let a = node_for(&mut graph, &mut token_index, meta.token_a);
            let b = node_for(&mut graph, &mut token_index, meta.token_b);
            let edge = graph.add_edge(a, b, meta.id);
            pool_edge.insert(meta.id, edge);
        }

        Self {
            graph,
            token_index,
            pool_edge,
        }
    }

    pub fn token_node(&self, mint: &Pubkey) -> Option<NodeIndex> {
        self.token_index.get(mint).copied()
    }

    pub fn token_at(&self, node: NodeIndex) -> Pubkey {
        self.graph[node]
    }

    pub fn pool_id_of(&self, edge: EdgeIndex) -> Pubkey {
        self.graph[edge]
    }

    pub fn edge_for_pool(&self, pool_id: &Pubkey) -> Option<EdgeIndex> {
        self.pool_edge.get(pool_id).copied()
    }

    pub fn pool_count(&self) -> usize {
        self.pool_edge.len()
    }

    pub fn token_count(&self) -> usize {
        self.token_index.len()
    }

    /// Every edge (pool_id) touching `node`, with the token on the other side.
    pub fn edges_from(&self, node: NodeIndex) -> Vec<(EdgeIndex, NodeIndex, Pubkey)> {
        self.graph
            .edges(node)
            .map(|e| (e.id(), e.target(), *e.weight()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arb_core::{Curve, Dex};

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
    fn builds_nodes_and_edges_from_pools() {
        let sol = Pubkey::new_unique();
        let usdc = Pubkey::new_unique();
        let pool1 = Pubkey::new_unique();
        let pool2 = Pubkey::new_unique();

        let pools = vec![meta(pool1, sol, usdc), meta(pool2, sol, usdc)];
        let graph = PoolGraph::build(&pools);

        assert_eq!(graph.token_count(), 2);
        assert_eq!(graph.pool_count(), 2);
        assert!(graph.token_node(&sol).is_some());
        assert!(graph.edge_for_pool(&pool1).is_some());
        assert!(graph.edge_for_pool(&pool2).is_some());
        assert_ne!(graph.edge_for_pool(&pool1), graph.edge_for_pool(&pool2));

        let sol_node = graph.token_node(&sol).unwrap();
        assert_eq!(
            graph.edges_from(sol_node).len(),
            2,
            "two parallel pools = two edges"
        );
    }
}
