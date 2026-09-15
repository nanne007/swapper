use crate::{balance_slots::BalanceSlotConfig, domain::is_reserved_address, error::ErrorKind};
use alloy_primitives::Address;
use anyhow::Context as _;
use serde::Deserialize;
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    path::Path,
};
use url::Url;

#[derive(Clone, Deserialize)]
#[serde(try_from = "ConfigDocument")]
pub struct Config {
    pub port: u16,
    pub host: IpAddr,
    pub timeout_ms: u64,
    pub max_active: usize,
    pub balance_slots: BalanceSlotConfig,
    pub provider_keys: HashMap<String, String>,
    pub okx_secret_key: Option<String>,
    pub okx_passphrase: Option<String>,
    pub okx_project_id: Option<String>,
    pub chains: HashMap<u64, ChainConfig>,
    alchemy_api_key: Option<String>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChainConfig {
    pub rpc_url: Option<String>,
    pub router: Option<Address>,
}

#[derive(Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct ConfigDocument {
    port: u16,
    host: IpAddr,
    competition_timeout_ms: u64,
    max_active: usize,
    balance_slots: BalanceSlotConfig,
    provider_keys: ProviderKeys,
    okx_secret_key: Option<String>,
    okx_passphrase: Option<String>,
    okx_project_id: Option<String>,
    chains: HashMap<u64, ChainConfig>,
    alchemy_api_key: Option<String>,
}

impl Default for ConfigDocument {
    fn default() -> Self {
        Self {
            port: 3000,
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            competition_timeout_ms: 6000,
            max_active: 20,
            balance_slots: HashMap::new(),
            provider_keys: ProviderKeys::default(),
            okx_secret_key: None,
            okx_passphrase: None,
            okx_project_id: None,
            chains: HashMap::new(),
            alchemy_api_key: None,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct ProviderKeys {
    #[serde(rename = "0x")]
    zero_ex: Option<String>,
    #[serde(rename = "1inch")]
    one_inch: Option<String>,
    barter: Option<String>,
    bebop: Option<String>,
    enso: Option<String>,
    hyper_bloom: Option<String>,
    kyber: Option<String>,
    odos: Option<String>,
    ooga_booga: Option<String>,
    okx: Option<String>,
}

impl TryFrom<ConfigDocument> for Config {
    type Error = anyhow::Error;

    fn try_from(document: ConfigDocument) -> anyhow::Result<Self> {
        if document.port == 0 {
            anyhow::bail!("port must be between 1 and 65535");
        }
        if !(100..=30_000).contains(&document.competition_timeout_ms) {
            anyhow::bail!("competitionTimeoutMs must be between 100 and 30000");
        }
        if document.max_active == 0 || document.max_active > tokio::sync::Semaphore::MAX_PERMITS {
            anyhow::bail!("maxActive is outside the supported semaphore capacity");
        }
        for (chain_id, chain) in &document.chains {
            if *chain_id == 0 {
                anyhow::bail!("chains must use nonzero chain IDs");
            }
            if let Some(value) = &chain.rpc_url {
                let url = Url::parse(value).context("invalid chains.rpcUrl")?;
                if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                    anyhow::bail!("chains.rpcUrl must use HTTP(S) with a host");
                }
            }
            if chain.router.is_some_and(is_reserved_address) {
                anyhow::bail!("chains.router must not be a reserved address");
            }
        }
        if document.balance_slots.iter().any(|(chain_id, slots)| {
            *chain_id == 0 || slots.keys().any(|token| is_reserved_address(*token))
        }) {
            anyhow::bail!("balanceSlots contains a reserved token or zero chain ID");
        }
        let keys = document.provider_keys;
        let mut provider_keys = HashMap::new();
        for (provider, value) in [
            ("0x", keys.zero_ex),
            ("1inch", keys.one_inch),
            ("barter", keys.barter),
            ("bebop", keys.bebop),
            ("enso", keys.enso),
            ("hyperBloom", keys.hyper_bloom),
            ("kyber", keys.kyber),
            ("odos", keys.odos),
            ("oogaBooga", keys.ooga_booga),
            ("okx", keys.okx),
        ] {
            if let Some(value) = value {
                if value.trim().is_empty() {
                    anyhow::bail!("providerKeys.{provider} must not be empty");
                }
                provider_keys.insert(provider.to_owned(), value);
            }
        }
        for (name, value) in [
            ("alchemyApiKey", &document.alchemy_api_key),
            ("okxSecretKey", &document.okx_secret_key),
            ("okxPassphrase", &document.okx_passphrase),
            ("okxProjectId", &document.okx_project_id),
        ] {
            if value.as_ref().is_some_and(|value| value.trim().is_empty()) {
                anyhow::bail!("{name} must not be empty");
            }
        }
        Ok(Self {
            port: document.port,
            host: document.host,
            timeout_ms: document.competition_timeout_ms,
            max_active: document.max_active,
            balance_slots: document.balance_slots,
            provider_keys,
            okx_secret_key: document.okx_secret_key,
            okx_passphrase: document.okx_passphrase,
            okx_project_id: document.okx_project_id,
            chains: document.chains,
            alchemy_api_key: document.alchemy_api_key,
        })
    }
}

impl Config {
    pub(crate) fn explicit_rpc_url(&self, chain_id: u64) -> Option<&str> {
        self.chains.get(&chain_id)?.rpc_url.as_deref()
    }

    pub(crate) fn alchemy_api_key(&self) -> Option<&str> {
        self.alchemy_api_key.as_deref()
    }
}

/// Parse one complete JSON document; do not accept trailing documents or env overrides.
pub fn load_config(json: &str) -> anyhow::Result<Config> {
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let config =
        serde_path_to_error::deserialize(&mut deserializer).context(ErrorKind::InvalidConfig)?;
    deserializer.end().context(ErrorKind::InvalidConfig)?;
    Ok(config)
}

pub fn load_config_file(path: impl AsRef<Path>) -> anyhow::Result<Config> {
    let path = path.as_ref();
    let data = std::fs::read_to_string(path)
        .with_context(|| format!("reading configuration {}", path.display()))
        .context(ErrorKind::InvalidConfig)?;
    load_config(&data).with_context(|| format!("loading configuration {}", path.display()))
}
