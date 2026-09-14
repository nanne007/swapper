use crate::error::ErrorKind;
use anyhow::Context as _;
use std::collections::HashMap;
use url::Url;

#[derive(Clone)]
pub struct Config {
    pub port: u16,
    pub host: String,
    pub ttl_ms: u64,
    pub timeout_ms: u64,
    pub max_competitions: usize,
    pub max_active: usize,
    pub provider_keys: HashMap<String, String>,
    pub okx_secret_key: Option<String>,
    pub okx_passphrase: Option<String>,
    pub okx_project_id: Option<String>,
    rpc_urls: HashMap<u64, String>,
    alchemy_api_key: Option<String>,
}

impl Config {
    pub(crate) fn explicit_rpc_url(&self, chain_id: u64) -> Option<&str> {
        self.rpc_urls.get(&chain_id).map(String::as_str)
    }

    pub(crate) fn alchemy_api_key(&self) -> Option<&str> {
        self.alchemy_api_key.as_deref()
    }
}

pub fn load_config_from_env() -> anyhow::Result<Config> {
    let env = std::env::vars().collect::<HashMap<_, _>>();
    load_config(&env)
}

pub fn load_config(env: &HashMap<String, String>) -> anyhow::Result<Config> {
    let port = bounded_integer(env.get("PORT"), 3000, 1, 65_535)? as u16;
    let ttl_ms = bounded_integer(env.get("QUOTE_TTL_MS"), 60_000, 10_000, 120_000)?;
    let timeout_ms = bounded_integer(env.get("PROVIDER_TIMEOUT_MS"), 6_000, 100, 30_000)?;
    let host = env
        .get("HOST")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".into());

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
    let rpc_urls = rpc_urls(env)?;
    let alchemy_api_key = env_value(env, "ALCHEMY_API_KEY");

    Ok(Config {
        port,
        host,
        ttl_ms,
        timeout_ms,
        max_competitions: 500,
        max_active: 20,
        provider_keys,
        okx_secret_key,
        okx_passphrase,
        okx_project_id,
        rpc_urls,
        alchemy_api_key,
    })
}

fn rpc_urls(env: &HashMap<String, String>) -> anyhow::Result<HashMap<u64, String>> {
    let mut rpc_urls = HashMap::new();
    for (name, value) in env {
        let Some(chain_id) = name
            .strip_prefix("RPC_URL_")
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        if !value.is_empty() {
            validate_rpc_url(value)?;
            rpc_urls.insert(chain_id, value.clone());
        }
    }
    if let Some(value) = env_value(env, "ETHEREUM_RPC_URL") {
        validate_rpc_url(&value)?;
        rpc_urls.entry(1).or_insert(value);
    }
    Ok(rpc_urls)
}

fn validate_rpc_url(value: &str) -> anyhow::Result<()> {
    let url = Url::parse(value).context(ErrorKind::InvalidConfig)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        anyhow::bail!(ErrorKind::InvalidConfig);
    }
    Ok(())
}

fn env_value(env: &HashMap<String, String>, name: &str) -> Option<String> {
    env.get(name).cloned().filter(|value| !value.is_empty())
}

fn bounded_integer(
    value: Option<&String>,
    default: u64,
    min: u64,
    max: u64,
) -> anyhow::Result<u64> {
    let value = value.map(String::as_str).unwrap_or_default();
    let parsed = if value.is_empty() {
        default
    } else {
        value.parse::<u64>().context(ErrorKind::InvalidConfig)?
    };
    if !(min..=max).contains(&parsed) {
        anyhow::bail!(ErrorKind::InvalidConfig);
    }
    Ok(parsed)
}
