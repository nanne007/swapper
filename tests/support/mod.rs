#![allow(dead_code)]
use anyhow::Context as _;

use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_rpc_types_eth::{
    TransactionRequest,
    simulate::{SimCallResult, SimulatePayload, SimulatedBlock},
};
use async_trait::async_trait;
use metamatch_backend::{
    app::App,
    chains::configured_chain,
    competitions::{Competitions, Services},
    config::{Config, load_config},
    domain::{
        Address as DomainAddress, Chain, Context, Input, NATIVE, PREVIEW_TAKER, Route, Rule,
        Simulation, Tx, now_ms, parse_address,
    },
    execution::swap_transaction,
    http::{HttpClient, HttpRequest, HttpResponse},
    providers::{Provider, ProviderRegistry, create_providers},
    rpc::{ContextProvider, RpcClients},
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
    ) -> anyhow::Result<HttpResponse> {
        self.requests.lock().unwrap().push(request);
        self.responses.lock().unwrap().pop().context("NO_FIXTURE")
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
    configured_chain(config, chain_id)
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
    pub revert: bool,
}

pub struct RpcServer {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RpcServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FixtureRpc {
    pub async fn start(self) -> RpcServer {
        use axum::{Json, Router, routing::post};
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let router = Router::new().route(
            "/",
            post(move |Json(request): Json<Value>| {
                recorded.lock().unwrap().push(request.clone());
                async move { Json(self.response(&request)) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        RpcServer {
            url,
            requests,
            task,
        }
    }

    fn response(&self, request: &Value) -> Value {
        use serde_json::json;
        let result = match request["method"].as_str().unwrap() {
            "eth_chainId" => json!("0x1"),
            "eth_getBlockByNumber" => {
                let mut block: alloy_rpc_types_eth::Block = Default::default();
                block.header.inner.number = 16;
                block.header.inner.timestamp = 100;
                block.header.hash = B256::from([if self.reorg { 0x22 } else { 0x11 }; 32]);
                json!(block)
            }
            "eth_gasPrice" => json!("0x3b9aca00"),
            "eth_getCode" => json!("0x6000"),
            "eth_getBalance" => json!("0x0"),
            "eth_call" => {
                let call: TransactionRequest =
                    serde_json::from_value(request["params"][0].clone()).unwrap();
                let is_balance = call
                    .input
                    .input()
                    .is_some_and(|data| data.starts_with(&[0x70, 0xa0, 0x82, 0x31]));
                json!(word_bytes(if is_balance {
                    U256::from(100)
                } else {
                    U256::ZERO
                }))
            }
            "eth_simulateV1" if self.unsupported_simulation => {
                return json!({
                    "jsonrpc": "2.0", "id": request["id"],
                    "error": {"code": -32601, "message": "simulation unsupported"}
                });
            }
            "eth_simulateV1" => {
                self.simulate(serde_json::from_value(request["params"][0].clone()).unwrap())
            }
            method => panic!("unexpected RPC method: {method}"),
        };
        json!({"jsonrpc": "2.0", "id": request["id"], "result": result})
    }

    fn simulate(&self, payload: SimulatePayload) -> Value {
        // Model the node's upfront balance check, followed by charging actual gas used.
        // A rich fixture account used to hide the real preview funding defect.
        for block in &payload.block_state_calls {
            let mut balance = block
                .state_overrides
                .as_ref()
                .and_then(|overrides| overrides.get(&PREVIEW_TAKER))
                .and_then(|account| account.balance)
                .unwrap_or_default();
            for call in &block.calls {
                if call.from != Some(PREVIEW_TAKER) {
                    continue;
                }
                let value = call.value.unwrap_or_default();
                let gas_price = U256::from(call.gas_price.unwrap_or_default());
                let upfront = value + U256::from(call.gas.unwrap_or_default()) * gas_price;
                assert!(
                    balance >= upfront,
                    "preview must cover the declared transaction gas limit"
                );
                balance -= value + U256::from(21_000) * gas_price;
            }
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
                status: !self.revert || index != 1,
                error: (self.revert && index == 1).then(|| {
                    alloy_rpc_types_eth::simulate::SimulateError {
                        code: 3,
                        message: "execution reverted fixture-secret".into(),
                        data: Some(Bytes::from_static(&[0xde, 0xad])),
                    }
                }),
            })
            .collect();
        let block: SimulatedBlock = SimulatedBlock {
            inner: Default::default(),
            calls,
        };
        serde_json::json!([block])
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
) -> anyhow::Result<SimResult> {
    let config = config();
    let mut chain = chain(&config, 1);
    let server = FixtureRpc {
        reorg,
        false_approval,
        unsupported_simulation,
        ..Default::default()
    }
    .start()
    .await;
    chain.rpc_url = Some(server.url.clone());
    let input = input(1, NATIVE);
    let route = fixture_route(&input);
    metamatch_backend::simulation::Simulator::new(Arc::new(RpcClients::new(Duration::from_secs(1))))
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
}

pub struct MockContext;

#[async_trait]
impl ContextProvider for MockContext {
    async fn get(&self, _input: &Input, _chain: &Chain) -> anyhow::Result<Context> {
        Ok(fixture_context())
    }
}

pub struct MockSimulation;

#[async_trait]
impl SimulationProvider for MockSimulation {
    async fn run(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
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

    async fn quote(&self, input: &Input, _chain: &Chain, sender: Address) -> anyhow::Result<Route> {
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
    competition_services_for(&config, None)
}

pub fn competition_services_for(config: &Config, configured_router: Option<Address>) -> Services {
    let router = configured_router.unwrap_or(PREVIEW_TAKER);
    let mut chain = chain(config, 1);
    chain.router = configured_router;
    Services::new(
        vec![Arc::new(MockProvider {
            rule: fixture_rule(router),
        })],
        &[chain],
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

pub fn fixture_registry(_config: &Config, providers: Vec<Arc<dyn Provider>>) -> ProviderRegistry {
    ProviderRegistry::new(providers)
}

pub fn fixture_app(config: Config, services: Services) -> App {
    metamatch_backend::app::create_app_with_services(config, Some(services))
}
