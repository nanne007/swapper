use crate::{config::Config, domain::Chain};
use alloy_chains::{Chain as AlloyChain, NamedChain};
use url::Url;

/// Builds runtime chain state from the current providers' supported chain IDs.
pub fn configured_chains(config: &Config, chain_ids: impl IntoIterator<Item = u64>) -> Vec<Chain> {
    chain_ids
        .into_iter()
        .map(|chain_id| configured_chain(config, chain_id))
        .collect()
}

/// Builds runtime state for one provider-supported chain ID.
pub fn configured_chain(config: &Config, chain_id: u64) -> Chain {
    Chain {
        id: chain_id,
        name: AlloyChain::from_id(chain_id).to_string(),
        rpc_url: rpc_url(config, chain_id),
        router: None,
    }
}

/// Returns the Alchemy endpoint prefix for a chain, without an API key.
pub fn alchemy_rpc_url(chain_id: u64) -> Option<&'static str> {
    match AlloyChain::from_id(chain_id).named()? {
        NamedChain::Mainnet => Some("https://eth-mainnet.g.alchemy.com/v2/"),
        NamedChain::Optimism => Some("https://opt-mainnet.g.alchemy.com/v2/"),
        NamedChain::BinanceSmartChain => Some("https://bnb-mainnet.g.alchemy.com/v2/"),
        NamedChain::Unichain => Some("https://unichain-mainnet.g.alchemy.com/v2/"),
        NamedChain::Polygon => Some("https://polygon-mainnet.g.alchemy.com/v2/"),
        NamedChain::Monad => Some("https://monad-mainnet.g.alchemy.com/v2/"),
        NamedChain::Sonic => Some("https://sonic-mainnet.g.alchemy.com/v2/"),
        NamedChain::Hyperliquid => Some("https://hyperliquid-mainnet.g.alchemy.com/v2/"),
        NamedChain::Mantle => Some("https://mantle-mainnet.g.alchemy.com/v2/"),
        NamedChain::Base => Some("https://base-mainnet.g.alchemy.com/v2/"),
        NamedChain::Plasma => Some("https://plasma-mainnet.g.alchemy.com/v2/"),
        NamedChain::Arbitrum => Some("https://arb-mainnet.g.alchemy.com/v2/"),
        NamedChain::Avalanche => Some("https://avax-mainnet.g.alchemy.com/v2/"),
        NamedChain::Linea => Some("https://linea-mainnet.g.alchemy.com/v2/"),
        NamedChain::Berachain => Some("https://berachain-mainnet.g.alchemy.com/v2/"),
        NamedChain::Blast => Some("https://blast-mainnet.g.alchemy.com/v2/"),
        NamedChain::Scroll => Some("https://scroll-mainnet.g.alchemy.com/v2/"),
        _ => None,
    }
}

/// Resolves an explicit chain RPC first, then the Alchemy fallback.
pub fn rpc_url(config: &Config, chain_id: u64) -> Option<String> {
    config
        .explicit_rpc_url(chain_id)
        .map(str::to_owned)
        .or_else(|| {
            let base_url = alchemy_rpc_url(chain_id)?;
            Some(append_api_key(base_url, config.alchemy_api_key()?))
        })
}

fn append_api_key(base_url: &str, api_key: &str) -> String {
    let mut url = Url::parse(base_url).expect("static Alchemy base URL must be valid");
    url.path_segments_mut()
        .expect("Alchemy endpoint must be a base URL")
        .pop_if_empty()
        .push(api_key);
    url.into()
}
