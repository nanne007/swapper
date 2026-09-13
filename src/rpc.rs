use crate::domain::{Chain, Context, Fault, Input, parse_hex_quantity};
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::{DynProvider, Provider as AlloyProvider, ProviderBuilder};
use alloy_rpc_types_eth::{
    BlockId, BlockNumberOrTag, TransactionRequest,
    simulate::{SimulatePayload, SimulatedBlock},
    state::StateOverride,
};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};

#[derive(Clone, Debug)]
pub struct BlockInfo {
    pub number: u64,
    pub hash: B256,
    pub timestamp: u64,
}

#[async_trait]
pub trait EvmRpc: Send + Sync {
    async fn chain_id(&self) -> Result<u64, Fault>;
    async fn latest_block(&self) -> Result<BlockInfo, Fault>;
    async fn block_by_number(&self, number: u64) -> Result<BlockInfo, Fault>;
    async fn gas_price(&self) -> Result<u128, Fault>;
    async fn code_at(&self, address: Address, block: BlockId) -> Result<Bytes, Fault>;
    async fn balance_at(&self, address: Address, block: BlockId) -> Result<U256, Fault>;
    async fn call(
        &self,
        request: TransactionRequest,
        block: BlockId,
        overrides: Option<StateOverride>,
    ) -> Result<Bytes, Fault>;
    async fn simulate(
        &self,
        payload: SimulatePayload,
        block: BlockId,
    ) -> Result<Vec<SimulatedBlock>, Fault>;
}

pub trait RpcFactory: Send + Sync {
    fn connect(&self, url: &str, timeout: Duration) -> Result<Arc<dyn EvmRpc>, Fault>;
}

#[derive(Clone, Default)]
pub struct AlloyRpcFactory;

struct AlloyRpc {
    provider: DynProvider,
}

impl RpcFactory for AlloyRpcFactory {
    fn connect(&self, url: &str, timeout: Duration) -> Result<Arc<dyn EvmRpc>, Fault> {
        let url = url
            .parse::<reqwest::Url>()
            .map_err(|_| Fault::with_status("RPC_NOT_CONFIGURED", 503))?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()
            .map_err(|_| Fault::with_status("RPC_NOT_CONFIGURED", 503))?;
        let provider = ProviderBuilder::default()
            .connect_reqwest(client, url)
            .erased();
        Ok(Arc::new(AlloyRpc { provider }))
    }
}

#[async_trait]
impl EvmRpc for AlloyRpc {
    async fn chain_id(&self) -> Result<u64, Fault> {
        self.provider.get_chain_id().await.map_err(map_rpc_error)
    }

    async fn latest_block(&self) -> Result<BlockInfo, Fault> {
        self.block(BlockNumberOrTag::Latest).await
    }

    async fn block_by_number(&self, number: u64) -> Result<BlockInfo, Fault> {
        self.block(BlockNumberOrTag::Number(number)).await
    }

    async fn gas_price(&self) -> Result<u128, Fault> {
        self.provider.get_gas_price().await.map_err(map_rpc_error)
    }

    async fn code_at(&self, address: Address, block: BlockId) -> Result<Bytes, Fault> {
        self.provider
            .get_code_at(address)
            .block_id(block)
            .await
            .map_err(map_rpc_error)
    }

    async fn balance_at(&self, address: Address, block: BlockId) -> Result<U256, Fault> {
        self.provider
            .get_balance(address)
            .block_id(block)
            .await
            .map_err(map_rpc_error)
    }

    async fn call(
        &self,
        request: TransactionRequest,
        block: BlockId,
        overrides: Option<StateOverride>,
    ) -> Result<Bytes, Fault> {
        self.provider
            .call(request)
            .block(block)
            .overrides_opt(overrides)
            .await
            .map_err(map_rpc_error)
    }

    async fn simulate(
        &self,
        payload: SimulatePayload,
        block: BlockId,
    ) -> Result<Vec<SimulatedBlock>, Fault> {
        self.provider
            .simulate(&payload)
            .block_id(block)
            .await
            .map_err(map_rpc_error)
    }
}

impl AlloyRpc {
    async fn block(&self, number: BlockNumberOrTag) -> Result<BlockInfo, Fault> {
        let block = self
            .provider
            .get_block_by_number(number)
            .await
            .map_err(map_rpc_error)?
            .ok_or_else(|| Fault::with_status("RPC_INVALID_RESPONSE", 502))?;
        Ok(BlockInfo {
            number: block.header.inner.number,
            hash: block.header.hash,
            timestamp: block.header.inner.timestamp,
        })
    }
}

fn map_rpc_error(error: alloy_provider::transport::TransportError) -> Fault {
    let code = if error
        .as_error_resp()
        .is_some_and(|response| response.code == -32601)
    {
        "RPC_METHOD_UNSUPPORTED"
    } else if error.is_null_resp() || error.is_deser_error() {
        "RPC_INVALID_RESPONSE"
    } else {
        "RPC_CALL_FAILED"
    };
    Fault::with_status(code, 502)
}

pub fn block_number(context: &Context) -> Result<u64, Fault> {
    parse_hex_quantity(&context.block_number)?
        .try_into()
        .map_err(|_| Fault::with_status("RPC_INVALID_RESPONSE", 502))
}

pub fn block_id(context: &Context) -> Result<BlockId, Fault> {
    Ok(block_number(context)?.into())
}

#[async_trait]
pub trait ContextProvider: Send + Sync {
    async fn get(&self, input: &Input, chain: &Chain) -> Result<Context, Fault>;
}

pub struct ContextSource {
    timeout: Duration,
    rpc_factory: Arc<dyn RpcFactory>,
}

impl ContextSource {
    pub fn new(timeout: Duration) -> Self {
        Self::with_factory(timeout, Arc::new(AlloyRpcFactory))
    }

    pub fn with_factory(timeout: Duration, rpc_factory: Arc<dyn RpcFactory>) -> Self {
        Self {
            timeout,
            rpc_factory,
        }
    }
}

#[async_trait]
impl ContextProvider for ContextSource {
    async fn get(&self, _input: &Input, chain: &Chain) -> Result<Context, Fault> {
        let Some(url) = &chain.rpc_url else {
            return Err(Fault::with_status("RPC_NOT_CONFIGURED", 503));
        };
        let rpc = self.rpc_factory.connect(url, self.timeout)?;
        let (chain_id, block, gas_price) =
            tokio::join!(rpc.chain_id(), rpc.latest_block(), rpc.gas_price());
        if chain_id? != chain.id {
            return Err(Fault::with_status("RPC_CHAIN_MISMATCH", 503));
        }
        let block = block?;
        let gas_price = gas_price?;
        Ok(Context {
            block_number: format!("0x{:x}", block.number),
            block_hash: format!("{:#x}", block.hash),
            timestamp: block.timestamp,
            gas_price: U256::from(gas_price).to_string(),
        })
    }
}
