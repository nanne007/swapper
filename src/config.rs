use crate::{chains::configured_chains, domain::Fault};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub host: String,
    pub ttl_ms: u64,
    pub timeout_ms: u64,
    pub max_competitions: usize,
    pub max_active: usize,
    pub chains: Vec<crate::domain::Chain>,
    pub provider_keys: HashMap<String, String>,
    pub okx_secret_key: Option<String>,
    pub okx_passphrase: Option<String>,
    pub okx_project_id: Option<String>,
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

    let chains = configured_chains(env)?;
    let provider_keys = [
        ("0x", "ZERO_EX_API_KEY"),
        ("1inch", "ONE_INCH_API_KEY"),
        ("barter", "BARTER_API_KEY"),
        ("bebop", "BEBOP_API_KEY"),
        ("enso", "ENSO_API_KEY"),
        ("hyperBloom", "HYPERBLOOM_API_KEY"),
        ("kyber", "KYBER_CLIENT_ID"),
        ("odos", "ODOS_API_KEY"),
        ("oogaBooga", "OOGABOOGA_API_KEY"),
        ("okx", "OKX_API_KEY"),
    ]
    .into_iter()
    .filter_map(|(provider, name)| {
        env.get(name)
            .filter(|value| !value.is_empty())
            .map(|value| (provider.to_owned(), value.clone()))
    })
    .collect();
    let okx_secret_key = env_value(env, "OKX_SECRET_KEY");
    let okx_passphrase = env_value(env, "OKX_API_PASSPHRASE");
    let okx_project_id = env_value(env, "OKX_PROJECT_ID");

    Ok(Config {
        port,
        host,
        ttl_ms,
        timeout_ms,
        max_competitions: 500,
        max_active: 20,
        chains,
        provider_keys,
        okx_secret_key,
        okx_passphrase,
        okx_project_id,
    })
}

fn env_value(env: &HashMap<String, String>, name: &str) -> Option<String> {
    env.get(name).cloned().filter(|value| !value.is_empty())
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
