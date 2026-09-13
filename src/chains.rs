use crate::domain::{Chain, Fault};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainSpec {
    pub id: u64,
    pub name: &'static str,
    pub slug: &'static str,
}

// This is the union of the EVM networks listed by Matcha Meta's DEX
// aggregation page. It is capability data, not an enable/disable config.
pub const CHAIN_CATALOG: &[ChainSpec] = &[
    ChainSpec {
        id: 1,
        name: "Ethereum",
        slug: "ethereum",
    },
    ChainSpec {
        id: 10,
        name: "Optimism",
        slug: "optimism",
    },
    ChainSpec {
        id: 56,
        name: "BNB Smart Chain",
        slug: "bsc",
    },
    ChainSpec {
        id: 130,
        name: "Unichain",
        slug: "unichain",
    },
    ChainSpec {
        id: 137,
        name: "Polygon",
        slug: "polygon",
    },
    ChainSpec {
        id: 146,
        name: "Sonic",
        slug: "sonic",
    },
    ChainSpec {
        id: 999,
        name: "HyperEVM",
        slug: "hyperevm",
    },
    ChainSpec {
        id: 5000,
        name: "Mantle",
        slug: "mantle",
    },
    ChainSpec {
        id: 8453,
        name: "Base",
        slug: "base",
    },
    ChainSpec {
        id: 9745,
        name: "Plasma",
        slug: "plasma",
    },
    ChainSpec {
        id: 143,
        name: "Monad",
        slug: "monad",
    },
    ChainSpec {
        id: 42161,
        name: "Arbitrum One",
        slug: "arbitrum",
    },
    ChainSpec {
        id: 43114,
        name: "Avalanche",
        slug: "avalanche",
    },
    ChainSpec {
        id: 59144,
        name: "Linea",
        slug: "linea",
    },
    ChainSpec {
        id: 80094,
        name: "Berachain",
        slug: "berachain",
    },
    ChainSpec {
        id: 81457,
        name: "Blast",
        slug: "blast",
    },
    ChainSpec {
        id: 534352,
        name: "Scroll",
        slug: "scroll",
    },
];

pub fn spec(chain_id: u64) -> Option<&'static ChainSpec> {
    CHAIN_CATALOG.iter().find(|item| item.id == chain_id)
}

pub fn configured_chains(env: &HashMap<String, String>) -> Result<Vec<Chain>, Fault> {
    CHAIN_CATALOG
        .iter()
        .map(|item| {
            let rpc_url = rpc_url(env, item.id);
            if let Some(url) = &rpc_url
                && !(url.starts_with("http://") || url.starts_with("https://"))
            {
                return Err(Fault::new("INVALID_CONFIG"));
            }
            Ok(Chain {
                id: item.id,
                name: item.name.into(),
                slug: item.slug.into(),
                rpc_url,
                router: None,
                balance_slots: HashMap::new(),
            })
        })
        .collect()
}

fn rpc_url(env: &HashMap<String, String>, chain_id: u64) -> Option<String> {
    env.get(&format!("RPC_URL_{chain_id}"))
        .cloned()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            (chain_id == 1)
                .then(|| env.get("ETHEREUM_RPC_URL").cloned())
                .flatten()
                .filter(|value| !value.is_empty())
        })
}
