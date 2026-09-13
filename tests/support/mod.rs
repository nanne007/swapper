#![allow(dead_code)]

use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_rpc_types_eth::{
    BlockId, TransactionRequest,
    simulate::{SimCallResult, SimulatePayload, SimulatedBlock},
    state::StateOverride,
};
use async_trait::async_trait;
use metamatch_backend::{
    app::App,
    competitions::{Competitions, Services},
    config::{Config, load_config},
    domain::{
        Address as DomainAddress, Chain, Context, Fault, Input, NATIVE, PREVIEW_TAKER, Route, Rule,
        Simulation, Tx, now_ms, parse_address,
    },
    execution::swap_transaction,
    http::{HttpClient, HttpRequest, HttpResponse},
    providers::{Provider, ProviderRegistry, create_providers},
    rpc::{BlockInfo, ContextProvider, EvmRpc, RpcFactory},
    simulation::{SimResult, SimulationProvider, SimulationRequest},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
pub struct MockHttp {
    pub requests: Mutex<Vec<HttpRequest>>,
    pub responses: Mutex<Vec<HttpResponse>>,
}

#[async_trait]
impl HttpClient for MockHttp {
    async fn execute(
        &self,
        request: HttpRequest,
        _timeout: Duration,
    ) -> Result<HttpResponse, Fault> {
        self.requests.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| Fault::new("NO_FIXTURE"))
    }
}

pub fn response(value: Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        body: value.to_string().into_bytes(),
    }
}

pub fn client(responses: impl IntoIterator<Item = Value>) -> Arc<MockHttp> {
    let client = Arc::new(MockHttp::default());
    client
        .responses
        .lock()
        .unwrap()
        .extend(responses.into_iter().map(response));
    client
}

pub fn config() -> Config {
    load_config(&HashMap::new()).unwrap()
}

pub fn config_with(entries: &[(&str, &str)]) -> Config {
    let env = entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    load_config(&env).unwrap()
}

pub fn chain(config: &Config, chain_id: u64) -> Chain {
    config
        .chains
        .iter()
        .find(|chain| chain.id == chain_id)
        .cloned()
        .expect("test chain exists")
}

pub fn input(chain_id: u64, sell_token: Address) -> Input {
    Input {
        chain_id,
        sell_token,
        buy_token: PREVIEW_TAKER,
        sell_amount: "1000000000000000000".into(),
        slippage_bps: 30,
        taker: None,
    }
}

pub fn provider(config: &Config, id: &str, client: Arc<dyn HttpClient>) -> Arc<dyn Provider> {
    create_providers(config, client)
        .into_iter()
        .find(|provider| provider.id() == id)
        .expect("provider is registered")
}

#[derive(Clone, Copy, Default)]
pub struct FixtureRpc {
    pub reorg: bool,
    pub false_approval: bool,
    pub unsupported_simulation: bool,
}

#[async_trait]
impl EvmRpc for FixtureRpc {
    async fn chain_id(&self) -> Result<u64, Fault> {
        Ok(1)
    }

    async fn latest_block(&self) -> Result<BlockInfo, Fault> {
        self.block_by_number(16).await
    }

    async fn block_by_number(&self, _number: u64) -> Result<BlockInfo, Fault> {
        Ok(BlockInfo {
            number: 16,
            hash: B256::from([if self.reorg { 0x22 } else { 0x11 }; 32]),
            timestamp: 100,
        })
    }

    async fn gas_price(&self) -> Result<u128, Fault> {
        Ok(1_000_000_000)
    }

    async fn code_at(&self, _address: Address, _block: BlockId) -> Result<Bytes, Fault> {
        Ok(Bytes::from_static(b"\x60\x00"))
    }

    async fn balance_at(&self, _address: Address, _block: BlockId) -> Result<U256, Fault> {
        Ok(U256::from(10_u64).pow(U256::from(20)))
    }

    async fn call(
        &self,
        request: TransactionRequest,
        _block: BlockId,
        _overrides: Option<StateOverride>,
    ) -> Result<Bytes, Fault> {
        let is_balance = request
            .input
            .input()
            .is_some_and(|data| data.starts_with(&[0x70, 0xa0, 0x82, 0x31]));
        Ok(word_bytes(if is_balance {
            U256::from(100)
        } else {
            U256::ZERO
        }))
    }

    async fn simulate(
        &self,
        payload: SimulatePayload,
        _block: BlockId,
    ) -> Result<Vec<SimulatedBlock>, Fault> {
        if self.unsupported_simulation {
            return Err(Fault::new("RPC_METHOD_UNSUPPORTED"));
        }
        let call_count = payload
            .block_state_calls
            .first()
            .map_or(0, |block| block.calls.len());
        let calls = (0..call_count)
            .map(|index| SimCallResult {
                return_data: word_bytes(if index == 0 {
                    U256::from(10)
                } else if index == call_count - 1 {
                    U256::from(110)
                } else if self.false_approval {
                    U256::ZERO
                } else {
                    U256::ONE
                }),
                logs: Vec::new(),
                gas_used: 0x5208,
                max_used_gas: None,
                status: true,
                error: None,
            })
            .collect();
        Ok(vec![SimulatedBlock {
            inner: Default::default(),
            calls,
        }])
    }
}

pub struct FixtureFactory {
    pub rpc: FixtureRpc,
}

impl RpcFactory for FixtureFactory {
    fn connect(&self, _url: &str, _timeout: Duration) -> Result<Arc<dyn EvmRpc>, Fault> {
        Ok(Arc::new(self.rpc))
    }
}

pub fn fixture_context() -> Context {
    Context {
        block_number: "0x10".into(),
        block_hash: format!("0x{}", "11".repeat(32)),
        timestamp: 100,
        gas_price: "1000000000".into(),
    }
}

pub fn fixture_route(input: &Input) -> Route {
    Route {
        provider: "kyber",
        buy_amount: "100".into(),
        min_buy_amount: "99".into(),
        sell_amount: input.sell_amount.clone(),
        spender: PREVIEW_TAKER,
        tx: Tx {
            to: PREVIEW_TAKER,
            data: "0x12345678".into(),
            value: input.sell_amount.clone(),
        },
        expires_at: now_ms() + 20_000,
    }
}

pub async fn run_simulation(
    reorg: bool,
    false_approval: bool,
    unsupported_simulation: bool,
) -> SimResult {
    let config = config();
    let mut chain = config.chains[0].clone();
    chain.rpc_url = Some("http://fixture".into());
    let input = input(1, NATIVE);
    let route = fixture_route(&input);
    SimulatorForTest::new(FixtureRpc {
        reorg,
        false_approval,
        unsupported_simulation,
    })
    .run(SimulationRequest {
        input: &input,
        chain: &chain,
        route: &route,
        context: &fixture_context(),
        rules: &[],
        taker: PREVIEW_TAKER,
        actual: false,
        min: None,
    })
    .await
    .unwrap()
}

struct SimulatorForTest {
    simulator: metamatch_backend::simulation::Simulator,
}

impl SimulatorForTest {
    fn new(rpc: FixtureRpc) -> Self {
        Self {
            simulator: metamatch_backend::simulation::Simulator::with_factory(
                Duration::from_secs(1),
                Arc::new(FixtureFactory { rpc }),
            ),
        }
    }

    async fn run(&self, request: SimulationRequest<'_>) -> Result<SimResult, Fault> {
        self.simulator.run(request).await
    }
}

pub struct MockContext;

#[async_trait]
impl ContextProvider for MockContext {
    async fn get(&self, _input: &Input, _chain: &Chain) -> Result<Context, Fault> {
        Ok(fixture_context())
    }
}

pub struct MockSimulation;

#[async_trait]
impl SimulationProvider for MockSimulation {
    async fn run(&self, request: SimulationRequest<'_>) -> Result<SimResult, Fault> {
        Ok(SimResult {
            simulation: Simulation::Success {
                bought_amount: request.route.buy_amount.clone(),
                gas_used: "1".into(),
                gas_fee_wei: Some("1".into()),
                funding: if request.actual {
                    "actual".into()
                } else {
                    "overridden".into()
                },
                block_hash: request.context.block_hash.clone(),
            },
            approvals: Vec::new(),
            transaction: swap_transaction(
                request.input,
                request.chain,
                request.route,
                request.rules,
                request.min,
            )?,
        })
    }
}

pub struct MockProvider {
    pub rule: Rule,
}

#[async_trait]
impl Provider for MockProvider {
    fn id(&self) -> &'static str {
        "kyber"
    }

    fn requires_access_key(&self) -> bool {
        false
    }

    fn supported_chains(&self) -> Vec<u64> {
        vec![1]
    }

    fn rules(&self, chain_id: u64) -> Vec<Rule> {
        if chain_id == 1 {
            vec![self.rule.clone()]
        } else {
            Vec::new()
        }
    }

    async fn quote(&self, input: &Input, _chain: &Chain, sender: Address) -> Result<Route, Fault> {
        Ok(Route {
            provider: self.id(),
            buy_amount: "200".into(),
            min_buy_amount: "199".into(),
            sell_amount: input.sell_amount.clone(),
            spender: sender,
            tx: Tx {
                to: sender,
                data: "0x12345678".into(),
                value: if input.sell_token == NATIVE {
                    input.sell_amount.clone()
                } else {
                    "0".into()
                },
            },
            expires_at: now_ms() + 20_000,
        })
    }
}

pub fn competition_services() -> Services {
    let config = config();
    competition_services_for(&config)
}

pub fn competition_services_for(config: &Config) -> Services {
    let router = config
        .chains
        .iter()
        .find(|chain| chain.id == 1)
        .and_then(|chain| chain.router)
        .unwrap_or(PREVIEW_TAKER);
    Services::new(
        vec![Arc::new(MockProvider {
            rule: fixture_rule(router),
        })],
        &config.chains,
        Arc::new(MockContext),
        Arc::new(MockSimulation),
    )
}

pub async fn wait_complete(
    service: &Competitions,
    id: &metamatch_backend::competitions::CreateResponse,
) -> metamatch_backend::competitions::Snapshot {
    for _ in 0..100 {
        let state = service.get(id.id, Some(&id.access_token)).await.unwrap();
        if state.status == "complete" {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("competition did not complete")
}

pub async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    authorization: Option<&str>,
) -> (axum::http::StatusCode, axum::http::HeaderMap, String) {
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    let request = builder
        .body(axum::body::Body::from(
            body.map_or_else(Vec::new, |body| body.to_string().into_bytes()),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8(body.to_vec()).unwrap())
}

fn word_bytes(value: U256) -> Bytes {
    Bytes::copy_from_slice(&value.to_be_bytes::<32>())
}

pub fn parse_fixture_address(value: &str) -> DomainAddress {
    parse_address(value).unwrap()
}

pub fn fixture_rule(target: Address) -> Rule {
    Rule {
        target,
        spender: target,
        selector: "0x12345678".into(),
    }
}

pub fn fixture_registry(config: &Config, providers: Vec<Arc<dyn Provider>>) -> ProviderRegistry {
    ProviderRegistry::new(providers, &config.chains)
}

pub fn fixture_app(config: Config, services: Services) -> App {
    metamatch_backend::app::create_app_with_services(config, Some(services))
}
