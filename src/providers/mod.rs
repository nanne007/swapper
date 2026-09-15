use crate::error::ErrorKind;
use crate::{
    config::Config,
    domain::{
        Address, Chain, Input, NATIVE, Route, Rule, Tx, minimum, parse_address, parse_hex,
        parse_positive, parse_uint,
    },
    http::{HttpClient, HttpRequest, auth_headers, json_request_as, url_with_params},
};
use anyhow::Context as _;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};

mod barter;
mod bebop;
mod enso;
mod hyperbloom;
mod kyber;
mod liquid_swap;
mod odos;
mod okx;
mod one_inch;
mod ooga_booga;
mod open_ocean;
mod velora;
mod zero_ex;

use self::{
    barter::BarterProvider, bebop::BebopProvider, enso::EnsoProvider,
    hyperbloom::HyperBloomProvider, kyber::KyberProvider, liquid_swap::LiquidSwapProvider,
    odos::OdosProvider, okx::OkxProvider, one_inch::OneInchProvider, ooga_booga::OogaBoogaProvider,
    open_ocean::OpenOceanProvider, velora::VeloraProvider, zero_ex::ZeroExProvider,
};

#[async_trait]
pub trait Provider: Send + Sync {
    /// Stable wire identifier owned by this adapter.
    fn id(&self) -> &'static str;

    /// Whether this adapter needs its configured credential to access any chain.
    fn requires_access_key(&self) -> bool;

    /// Chains this configured adapter can actually enter in the competition.
    fn supported_chains(&self) -> Vec<u64>;

    /// Provider-specific route tuples used by the off-chain preflight check.
    fn rules(&self, _chain_id: u64) -> Vec<Rule> {
        Vec::new()
    }
    async fn quote(&self, input: &Input, chain: &Chain, sender: Address) -> anyhow::Result<Route>;
}

pub fn create_providers(config: &Config, client: Arc<dyn HttpClient>) -> Vec<Arc<dyn Provider>> {
    let providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(ZeroExProvider {
            key: key(config, "0x"),
            client: client.clone(),
            timeout: timeout(config),
        }),
        Arc::new(OneInchProvider {
            key: key(config, "1inch"),
            client: client.clone(),
            timeout: timeout(config),
        }),
        Arc::new(KyberProvider {
            client: client.clone(),
            client_id: key(config, "kyber"),
            timeout: timeout(config),
        }),
        Arc::new(BarterProvider {
            key: key(config, "barter"),
            client: client.clone(),
            timeout: timeout(config),
        }),
        Arc::new(BebopProvider {
            key: key(config, "bebop"),
            client: client.clone(),
            timeout: timeout(config),
        }),
        Arc::new(EnsoProvider {
            key: key(config, "enso"),
            client: client.clone(),
            timeout: timeout(config),
        }),
        Arc::new(HyperBloomProvider {
            key: key(config, "hyperBloom"),
            client: client.clone(),
            timeout: timeout(config),
        }),
        Arc::new(LiquidSwapProvider {
            client: client.clone(),
            timeout: timeout(config),
        }),
        Arc::new(OdosProvider {
            key: key(config, "odos"),
            client: client.clone(),
            timeout: timeout(config),
        }),
        Arc::new(OogaBoogaProvider {
            key: key(config, "oogaBooga"),
            client: client.clone(),
            timeout: timeout(config),
        }),
        Arc::new(OkxProvider {
            api_key: key(config, "okx"),
            secret_key: config.okx_secret_key.clone(),
            passphrase: config.okx_passphrase.clone(),
            project_id: config.okx_project_id.clone(),
            client: client.clone(),
            timeout: timeout(config),
        }),
        Arc::new(OpenOceanProvider {
            client: client.clone(),
            timeout: timeout(config),
        }),
        Arc::new(VeloraProvider {
            client,
            timeout: timeout(config),
        }),
    ];
    providers
}

fn key(config: &Config, provider: &str) -> Option<String> {
    config.provider_keys.get(provider).cloned()
}

fn chains_if_configured(configured: bool, supported: &[u64]) -> Vec<u64> {
    if configured {
        supported.to_vec()
    } else {
        Vec::new()
    }
}

fn timeout(config: &Config) -> Duration {
    Duration::from_millis(config.timeout_ms)
}

#[derive(Clone)]
pub struct ProviderRegistry {
    providers: HashMap<&'static str, Arc<dyn Provider>>,
    by_chain: HashMap<u64, Vec<&'static str>>,
}

impl ProviderRegistry {
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        let mut registered = HashMap::with_capacity(providers.len());
        let mut by_chain = HashMap::new();
        for provider in providers {
            let id = provider.id();
            assert!(
                registered.insert(id, provider.clone()).is_none(),
                "duplicate provider id: {id}"
            );
            for chain_id in provider.supported_chains() {
                by_chain.entry(chain_id).or_insert_with(Vec::new).push(id);
            }
        }
        Self {
            providers: registered,
            by_chain,
        }
    }

    pub fn for_chain(&self, chain_id: u64) -> Vec<Arc<dyn Provider>> {
        let Some(ids) = self.by_chain.get(&chain_id) else {
            return Vec::new();
        };
        ids.iter()
            .filter_map(|id| self.providers.get(id).cloned())
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(id).cloned()
    }

    pub fn by_chain(&self) -> &HashMap<u64, Vec<&'static str>> {
        &self.by_chain
    }

    pub fn chain_ids(&self) -> Vec<u64> {
        let mut chain_ids = self.by_chain.keys().copied().collect::<Vec<_>>();
        chain_ids.sort_unstable();
        chain_ids
    }
}

fn address_string(address: Address) -> String {
    format!("{address:#x}")
}

#[derive(Debug, Deserialize)]
struct RawTransaction {
    to: String,
    data: String,
    value: Value,
    from: Option<String>,
}

impl RawTransaction {
    fn into_tx(self, expected_from: Address) -> anyhow::Result<Tx> {
        if self
            .from
            .as_deref()
            .map(parse_address)
            .transpose()?
            .is_some_and(|from| from != expected_from)
        {
            anyhow::bail!(ErrorKind::UpstreamTakerMismatch);
        }
        Ok(Tx {
            to: parse_address(&self.to)?,
            data: parse_hex(&self.data)?,
            value: quantity_value(&self.value)?,
        })
    }
}

struct RouteCandidate {
    sell_amount: String,
    buy_amount: String,
    min_buy_amount: String,
    spender: Address,
    tx: RawTransaction,
    deadline: Option<u64>,
}

fn normalize_route(
    provider: &'static str,
    input: &Input,
    sender: Address,
    candidate: RouteCandidate,
) -> anyhow::Result<Route> {
    let sell_amount = positive_string(&candidate.sell_amount)?;
    if sell_amount != input.sell_amount {
        anyhow::bail!(ErrorKind::UpstreamAmountMismatch);
    }
    let buy_amount = positive_string(&candidate.buy_amount)?;
    let min_buy_amount = positive_string(&candidate.min_buy_amount)?;
    if parse_uint(&min_buy_amount)? > parse_uint(&buy_amount)? {
        anyhow::bail!(ErrorKind::UpstreamMinimumExceedsQuote);
    }
    let transaction = candidate.tx.into_tx(sender)?;
    crate::execution::validate_transaction(input, &transaction, candidate.deadline)?;
    if transaction.to == Address::ZERO
        || (input.sell_token != NATIVE && candidate.spender == Address::ZERO)
    {
        anyhow::bail!(ErrorKind::UpstreamInvalidResponse);
    }
    Ok(Route {
        provider,
        buy_amount,
        min_buy_amount,
        sell_amount,
        spender: candidate.spender,
        tx: transaction,
        deadline: candidate.deadline,
    })
}

fn positive_string(value: &str) -> anyhow::Result<String> {
    Ok(parse_positive(value)?.to_string())
}

fn quantity_value(value: &Value) -> anyhow::Result<String> {
    let value = match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value
            .as_u64()
            .map(|value| value.to_string())
            .context(ErrorKind::UpstreamInvalidResponse)?,
        _ => return Err(anyhow::Error::new(ErrorKind::UpstreamInvalidResponse)),
    };
    if value.starts_with("0x") {
        return Ok(crate::domain::parse_hex_quantity(&value)?.to_string());
    }
    Ok(parse_uint(&value)?.to_string())
}

fn percentage_string(bps: u64) -> String {
    format!("{}.{:02}", bps / 100, bps % 100)
}

fn fraction_string(bps: u64) -> String {
    let mut value = format!("0.{bps:04}");
    while value.ends_with('0') {
        value.pop();
    }
    value
}
