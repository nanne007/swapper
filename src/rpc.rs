use crate::domain::{Chain, Context, Input, parse_hex_quantity};
use crate::error::ErrorKind;
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use alloy_rpc_types_eth::{BlockId, BlockNumberOrTag};
use anyhow::Context as _;
use async_trait::async_trait;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Reuse one Alloy provider per configured RPC URL across context and simulations.
pub struct RpcClients {
    client: reqwest::Client,
    providers: Mutex<HashMap<String, DynProvider>>,
}

impl RpcClients {
    pub fn new(timeout: Duration) -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(timeout)
                .build()
                .expect("static RPC client configuration must be valid"),
            providers: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, url: &str) -> anyhow::Result<DynProvider> {
        let mut providers = self.providers.lock().expect("RPC clients lock poisoned");
        if let Some(provider) = providers.get(url) {
            return Ok(provider.clone());
        }
        let endpoint = url
            .parse::<reqwest::Url>()
            .context(ErrorKind::RpcNotConfigured)?;
        let provider = ProviderBuilder::default()
            .connect_reqwest(self.client.clone(), endpoint)
            .erased();
        providers.insert(url.to_owned(), provider.clone());
        Ok(provider)
    }
}

pub fn map_rpc_error(
    method: &'static str,
    error: alloy_provider::transport::TransportError,
) -> anyhow::Error {
    let code = if error
        .as_error_resp()
        .is_some_and(|response| response.code == -32601)
    {
        ErrorKind::RpcMethodUnsupported
    } else if error.is_null_resp() || error.is_deser_error() {
        ErrorKind::RpcInvalidResponse
    } else {
        ErrorKind::RpcCallFailed
    };
    anyhow::Error::new(error).context(code).context(method)
}

pub fn block_number(context: &Context) -> anyhow::Result<u64> {
    parse_hex_quantity(&context.block_number)?
        .try_into()
        .context(ErrorKind::RpcInvalidResponse)
}

pub fn block_id(context: &Context) -> anyhow::Result<BlockId> {
    Ok(block_number(context)?.into())
}

#[async_trait]
pub trait ContextProvider: Send + Sync {
    async fn get(&self, input: &Input, chain: &Chain) -> anyhow::Result<Context>;
}

pub struct ContextSource {
    clients: Arc<RpcClients>,
}

impl ContextSource {
    pub fn new(clients: Arc<RpcClients>) -> Self {
        Self { clients }
    }
}

#[async_trait]
impl ContextProvider for ContextSource {
    async fn get(&self, _input: &Input, chain: &Chain) -> anyhow::Result<Context> {
        let url = chain
            .rpc_url
            .as_deref()
            .context(ErrorKind::RpcNotConfigured)?;
        let rpc = self.clients.get(url)?;
        let (chain_id, block, gas_price) = tokio::join!(
            rpc.get_chain_id(),
            rpc.get_block_by_number(BlockNumberOrTag::Latest),
            rpc.get_gas_price()
        );
        if chain_id.map_err(|error| map_rpc_error("eth_chainId", error))? != chain.id {
            anyhow::bail!(ErrorKind::RpcChainMismatch);
        }
        let block = block
            .map_err(|error| map_rpc_error("eth_getBlockByNumber", error))?
            .context(ErrorKind::RpcInvalidResponse)?;
        let gas_price = gas_price.map_err(|error| map_rpc_error("eth_gasPrice", error))?;
        Ok(Context {
            block_number: format!("0x{:x}", block.header.inner.number),
            block_hash: format!("{:#x}", block.header.hash),
            timestamp: block.header.inner.timestamp,
            gas_price: gas_price.to_string(),
        })
    }
}
