use crate::{
    domain::{Chain, Context, Fault, Input, parse_hex_quantity},
    http::{HttpClient, HttpRequest, json_request},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::{DynProvider, Provider as AlloyProvider, ProviderBuilder};
use alloy_rpc_types_eth::{
    BlockId, BlockNumberOrTag, TransactionRequest,
    simulate::{SimulatePayload, SimulatedBlock},
    state::StateOverride,
};
use async_trait::async_trait;
use serde_json::Value;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;

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
    client: Arc<dyn HttpClient>,
    timeout: Duration,
    rpc_factory: Arc<dyn RpcFactory>,
    prices: Mutex<Option<PriceCache>>,
}

struct PriceCache {
    at: u64,
    values: HashMap<String, String>,
}

impl ContextSource {
    pub fn new(client: Arc<dyn HttpClient>, timeout: Duration) -> Self {
        Self::with_factory(client, timeout, Arc::new(AlloyRpcFactory))
    }

    pub fn with_factory(
        client: Arc<dyn HttpClient>,
        timeout: Duration,
        rpc_factory: Arc<dyn RpcFactory>,
    ) -> Self {
        Self {
            client,
            timeout,
            rpc_factory,
            prices: Mutex::new(None),
        }
    }

    async fn get_prices(&self) -> HashMap<String, String> {
        {
            let cache = self.prices.lock().await;
            if cache
                .as_ref()
                .is_some_and(|cache| crate::domain::now_ms().saturating_sub(cache.at) < 30_000)
            {
                return cache.as_ref().expect("cache was checked").values.clone();
            }
        }
        let request = HttpRequest {
            method: "GET".into(),
            url: "https://api.coingecko.com/api/v3/simple/price?ids=ethereum,usd-coin&vs_currencies=usd".into(),
            headers: HashMap::new(),
            body: None,
        };
        let Ok(raw) = json_request(self.client.as_ref(), request, Duration::from_secs(2)).await
        else {
            return HashMap::new();
        };
        let mut values = HashMap::new();
        let Some(objects) = raw.as_object() else {
            return values;
        };
        for (key, value) in objects {
            let Some(price) = value.get("usd").and_then(Value::as_number) else {
                continue;
            };
            let price = price.to_string();
            if let Ok(units) = crate::domain::price_units(&price) {
                values.insert(
                    key.clone(),
                    format!(
                        "{}.{:08}",
                        units / alloy_primitives::U256::from(100_000_000_u64),
                        units % alloy_primitives::U256::from(100_000_000_u64)
                    ),
                );
            }
        }
        *self.prices.lock().await = Some(PriceCache {
            at: crate::domain::now_ms(),
            values: values.clone(),
        });
        values
    }
}

#[async_trait]
impl ContextProvider for ContextSource {
    async fn get(&self, input: &Input, chain: &Chain) -> Result<Context, Fault> {
        let Some(url) = &chain.rpc_url else {
            return Err(Fault::with_status("RPC_NOT_CONFIGURED", 503));
        };
        let rpc = self.rpc_factory.connect(url, self.timeout)?;
        let (chain_id, block, gas_price, prices) = tokio::join!(
            rpc.chain_id(),
            rpc.latest_block(),
            rpc.gas_price(),
            self.get_prices(),
        );
        if chain_id? != chain.id {
            return Err(Fault::with_status("RPC_CHAIN_MISMATCH", 503));
        }
        let block = block?;
        let gas_price = gas_price?;
        let buy_token = chain
            .tokens
            .iter()
            .find(|token| token.address == input.buy_token)
            .ok_or_else(|| Fault::with_status("TOKEN_NOT_SUPPORTED", 422))?;
        Ok(Context {
            block_number: format!("0x{:x}", block.number),
            block_hash: format!("{:#x}", block.hash),
            timestamp: block.timestamp,
            gas_price: U256::from(gas_price).to_string(),
            native_usd: prices.get("ethereum").cloned(),
            buy_usd: prices.get(&buy_token.price_id).cloned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{NATIVE, PREVIEW_TAKER, Token},
        http::{HttpRequest, HttpResponse},
    };
    use async_trait::async_trait;
    use std::sync::Mutex as StdMutex;

    struct MockHttp {
        requests: StdMutex<Vec<HttpRequest>>,
    }

    #[async_trait]
    impl HttpClient for MockHttp {
        async fn execute(
            &self,
            request: HttpRequest,
            _timeout: Duration,
        ) -> Result<HttpResponse, Fault> {
            if request.body.is_none() {
                return Ok(HttpResponse {
                    status: 200,
                    body: br#"{"ethereum":{"usd":3000},"usd-coin":{"usd":1}}"#.to_vec(),
                });
            }
            self.requests.lock().unwrap().push(request);
            unreachable!("RPC traffic is handled by the typed fixture")
        }
    }

    struct FixtureRpc;

    #[async_trait]
    impl EvmRpc for FixtureRpc {
        async fn chain_id(&self) -> Result<u64, Fault> {
            Ok(1)
        }

        async fn latest_block(&self) -> Result<BlockInfo, Fault> {
            Ok(BlockInfo {
                number: 16,
                hash: B256::from([0x11; 32]),
                timestamp: 100,
            })
        }

        async fn block_by_number(&self, _number: u64) -> Result<BlockInfo, Fault> {
            self.latest_block().await
        }

        async fn gas_price(&self) -> Result<u128, Fault> {
            Ok(1_000_000_000)
        }

        async fn code_at(&self, _address: Address, _block: BlockId) -> Result<Bytes, Fault> {
            Ok(Bytes::new())
        }

        async fn balance_at(&self, _address: Address, _block: BlockId) -> Result<U256, Fault> {
            Ok(U256::ZERO)
        }

        async fn call(
            &self,
            _request: TransactionRequest,
            _block: BlockId,
            _overrides: Option<StateOverride>,
        ) -> Result<Bytes, Fault> {
            Ok(Bytes::new())
        }

        async fn simulate(
            &self,
            _payload: SimulatePayload,
            _block: BlockId,
        ) -> Result<Vec<SimulatedBlock>, Fault> {
            Ok(Vec::new())
        }
    }

    struct FixtureFactory;

    impl RpcFactory for FixtureFactory {
        fn connect(&self, _url: &str, _timeout: Duration) -> Result<Arc<dyn EvmRpc>, Fault> {
            Ok(Arc::new(FixtureRpc))
        }
    }

    #[tokio::test]
    async fn typed_rpc_provider_supplies_context_and_prices() {
        let client = Arc::new(MockHttp {
            requests: StdMutex::new(Vec::new()),
        });
        let source = ContextSource::with_factory(
            client.clone(),
            Duration::from_secs(1),
            Arc::new(FixtureFactory),
        );
        let chain = Chain {
            id: 1,
            name: "Ethereum".into(),
            rpc_url: Some("http://fixture".into()),
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
                    address: PREVIEW_TAKER,
                    symbol: "BUY".into(),
                    decimals: 18,
                    price_id: "ethereum".into(),
                },
            ],
        };
        let input = Input {
            chain_id: 1,
            sell_token: NATIVE,
            buy_token: PREVIEW_TAKER,
            sell_amount: "1".into(),
            slippage_bps: 30,
            taker: None,
        };
        let context = source.get(&input, &chain).await.unwrap();
        assert_eq!(context.block_number, "0x10");
        assert_eq!(context.block_hash, format!("0x{}", "11".repeat(32)));
        assert_eq!(context.gas_price, "1000000000");
        assert_eq!(context.native_usd.as_deref(), Some("3000.00000000"));
        assert_eq!(client.requests.lock().unwrap().len(), 0);
    }
}
