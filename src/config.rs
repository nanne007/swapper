use crate::domain::{Chain, Fault, HOLDER, NATIVE, ProviderId, Rule, Token, parse_address};
use serde::Deserialize;
use std::{collections::HashMap, fs};

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub host: String,
    pub ttl_ms: u64,
    pub timeout_ms: u64,
    pub max_competitions: usize,
    pub max_active: usize,
    pub chains: Vec<Chain>,
    pub zero_ex_key: Option<String>,
    pub one_inch_key: Option<String>,
    pub kyber_client_id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(rename = "1")]
    ethereum: Option<ChainConfig>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ChainConfig {
    router: Option<String>,
    #[serde(default)]
    rules: HashMap<String, Vec<RuleConfig>>,
    #[serde(rename = "balanceSlots", default)]
    balance_slots: HashMap<String, u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleConfig {
    target: String,
    spender: String,
    selector: String,
}

pub fn load_config_from_env() -> Result<Config, Fault> {
    let env = std::env::vars().collect::<HashMap<_, _>>();
    load_config(&env)
}

pub fn load_config(env: &HashMap<String, String>) -> Result<Config, Fault> {
    let port = bounded_integer(env.get("PORT"), 3000, 1, 65_535)? as u16;
    let ttl_ms = bounded_integer(env.get("QUOTE_TTL_MS"), 60_000, 10_000, 120_000)?;
    let timeout_ms = bounded_integer(env.get("PROVIDER_TIMEOUT_MS"), 6_000, 100, 30_000)?;
    let host = env
        .get("HOST")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".into());
    let custom = match env.get("CONFIG_PATH") {
        Some(path) => serde_json::from_str::<FileConfig>(
            &fs::read_to_string(path).map_err(|_| Fault::new("INVALID_CONFIG"))?,
        )
        .map_err(|_| Fault::new("INVALID_CONFIG"))?,
        None => FileConfig::default(),
    };

    let mut chains = vec![ethereum_chain(env.get("ETHEREUM_RPC_URL").cloned())];
    apply_chain_config(&mut chains[0], custom.ethereum)?;
    for chain in &chains {
        if let Some(url) = &chain.rpc_url
            && !(url.starts_with("http://") || url.starts_with("https://"))
        {
            return Err(Fault::new("INVALID_CONFIG"));
        }
    }

    Ok(Config {
        port,
        host,
        ttl_ms,
        timeout_ms,
        max_competitions: 500,
        max_active: 20,
        chains,
        zero_ex_key: env
            .get("ZERO_EX_API_KEY")
            .cloned()
            .filter(|value| !value.is_empty()),
        one_inch_key: env
            .get("ONE_INCH_API_KEY")
            .cloned()
            .filter(|value| !value.is_empty()),
        kyber_client_id: env
            .get("KYBER_CLIENT_ID")
            .filter(|value| !value.is_empty())
            .cloned(),
    })
}

fn bounded_integer(value: Option<&String>, default: u64, min: u64, max: u64) -> Result<u64, Fault> {
    let value = value.map(String::as_str).unwrap_or_default();
    let parsed = if value.is_empty() {
        default
    } else {
        value
            .parse::<u64>()
            .map_err(|_| Fault::new("INVALID_CONFIG"))?
    };
    if !(min..=max).contains(&parsed) {
        return Err(Fault::new("INVALID_CONFIG"));
    }
    Ok(parsed)
}

fn ethereum_chain(rpc_url: Option<String>) -> Chain {
    Chain {
        id: 1,
        name: "Ethereum".into(),
        rpc_url,
        router: None,
        rules: HashMap::new(),
        balance_slots: HashMap::new(),
        tokens: vec![
            Token {
                address: NATIVE,
                symbol: "ETH".into(),
                decimals: 18,
                price_id: "ethereum".into(),
            },
            Token {
                address: parse_address("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2")
                    .expect("static WETH address"),
                symbol: "WETH".into(),
                decimals: 18,
                price_id: "ethereum".into(),
            },
            Token {
                address: parse_address("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")
                    .expect("static USDC address"),
                symbol: "USDC".into(),
                decimals: 6,
                price_id: "usd-coin".into(),
            },
        ],
    }
}

fn apply_chain_config(chain: &mut Chain, custom: Option<ChainConfig>) -> Result<(), Fault> {
    let Some(custom) = custom else {
        return Ok(());
    };
    if let Some(router) = custom.router {
        let router = parse_address(&router)?;
        if router == HOLDER {
            return Err(Fault::new("INVALID_CONFIG"));
        }
        chain.router = Some(router);
    }
    chain.balance_slots = custom.balance_slots;
    let mut rules = HashMap::new();
    for (provider, entries) in custom.rules {
        let provider = match provider.as_str() {
            "0x" => ProviderId::ZeroEx,
            "1inch" => ProviderId::OneInch,
            "kyber" => ProviderId::Kyber,
            _ => return Err(Fault::new("INVALID_CONFIG")),
        };
        let parsed = entries
            .into_iter()
            .map(|entry| {
                if entry.selector.len() != 10
                    || !entry.selector.starts_with("0x")
                    || hex::decode(&entry.selector[2..]).is_err()
                {
                    return Err(Fault::new("INVALID_CONFIG"));
                }
                Ok(Rule {
                    target: parse_address(&entry.target)?,
                    spender: parse_address(&entry.spender)?,
                    selector: entry.selector.to_ascii_lowercase(),
                })
            })
            .collect::<Result<Vec<_>, Fault>>()?;
        rules.insert(provider, parsed);
    }
    chain.rules = rules;
    if let Some(url) = &chain.rpc_url
        && !(url.starts_with("http://") || url.starts_with("https://"))
    {
        return Err(Fault::new("INVALID_CONFIG"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_one_configurable_chain() {
        let config = load_config(&HashMap::new()).unwrap();
        assert_eq!(config.port, 3000);
        assert_eq!(config.chains.len(), 1);
        assert_eq!(config.chains[0].id, 1);
    }

    #[test]
    fn rejects_bad_rpc_and_reserved_router() {
        let mut env = HashMap::from([(
            String::from("ETHEREUM_RPC_URL"),
            String::from("file:///tmp/rpc"),
        )]);
        assert!(load_config(&env).is_err());
        env.insert(
            String::from("CONFIG_PATH"),
            String::from("/path/does/not/exist"),
        );
        assert!(load_config(&env).is_err());
    }
}
