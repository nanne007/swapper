use crate::{
    config::Config,
    domain::{
        Address, Chain, Context, Fault, Input, PREVIEW_TAKER, ProviderId, Quote, Simulation, Tx,
        is_reserved_address, net_output, now_ms, parse_positive, rank,
    },
    execution::validate_route,
    http::ReqwestClient,
    providers::{Provider, create_providers},
    rpc::{ContextProvider, ContextSource},
    simulation::{SimulationProvider, SimulationRequest, Simulator},
};
use futures::future::join_all;
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use uuid::Uuid;

struct Competition {
    id: Uuid,
    token: String,
    input: Input,
    chain: Chain,
    expires_at: u64,
    status: String,
    quotes: Vec<Quote>,
    context: Option<Context>,
}

struct CompetitionHandle {
    state: Mutex<Competition>,
    cancel: AtomicBool,
}

#[derive(Clone)]
pub struct Services {
    pub providers: Vec<Arc<dyn Provider>>,
    pub context: Arc<dyn ContextProvider>,
    pub simulator: Arc<dyn SimulationProvider>,
}

impl Services {
    pub fn production(config: &Config) -> Self {
        let client = Arc::new(ReqwestClient::default());
        Self {
            providers: create_providers(config, client.clone()),
            context: Arc::new(ContextSource::new(
                client.clone(),
                Duration::from_millis(config.timeout_ms),
            )),
            simulator: Arc::new(Simulator::new(Duration::from_millis(config.timeout_ms))),
        }
    }
}

#[derive(Clone)]
pub struct Competitions {
    config: Arc<Config>,
    services: Services,
    items: Arc<Mutex<HashMap<Uuid, Arc<CompetitionHandle>>>>,
    active: Arc<AtomicUsize>,
    builds: Arc<AtomicUsize>,
}

impl Competitions {
    pub fn new(config: Config, services: Option<Services>) -> Self {
        let services = services.unwrap_or_else(|| Services::production(&config));
        Self {
            config: Arc::new(config),
            services,
            items: Arc::new(Mutex::new(HashMap::new())),
            active: Arc::new(AtomicUsize::new(0)),
            builds: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub async fn create(&self, input: Input) -> Result<CreateResponse, Fault> {
        self.sweep().await;
        let mut items = self.items.lock().await;
        if self.active.load(Ordering::Acquire) >= self.config.max_active
            || items.len() >= self.config.max_competitions
        {
            return Err(Fault::with_status("CAPACITY_EXCEEDED", 429));
        }
        let chain = self
            .config
            .chains
            .iter()
            .find(|chain| chain.id == input.chain_id)
            .cloned()
            .ok_or_else(|| Fault::with_status("INVALID_INPUT", 400))?;
        if ![input.sell_token, input.buy_token]
            .iter()
            .all(|token| chain.tokens.iter().any(|known| known.address == *token))
        {
            return Err(Fault::with_status("TOKEN_NOT_SUPPORTED", 422));
        }
        if input
            .taker
            .is_some_and(|taker| chain.router == Some(taker) || is_reserved_address(taker))
        {
            return Err(Fault::new("INVALID_TAKER"));
        }
        let id = Uuid::new_v4();
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let expires_at = now_ms() + self.config.ttl_ms;
        let handle = Arc::new(CompetitionHandle {
            state: Mutex::new(Competition {
                id,
                token: token.clone(),
                input: input.clone(),
                chain,
                expires_at,
                status: "running".into(),
                quotes: Vec::new(),
                context: None,
            }),
            cancel: AtomicBool::new(false),
        });
        items.insert(id, handle.clone());
        self.active.fetch_add(1, Ordering::AcqRel);
        let service = self.clone();
        tokio::spawn(async move {
            service.run(handle).await;
            service.active.fetch_sub(1, Ordering::AcqRel);
        });
        Ok(CreateResponse {
            id,
            access_token: token,
            expires_at,
        })
    }

    pub async fn get(&self, id: Uuid, token: Option<&str>) -> Result<Snapshot, Fault> {
        let handle = self.access(id, token).await?;
        let state = handle.state.lock().await;
        let quotes = rank(&state.quotes);
        let recommended = quotes
            .iter()
            .find(|quote| is_verified(quote))
            .map(|quote| quote.id.clone());
        Ok(Snapshot {
            id: state.id,
            input: state.input.clone(),
            status: state.status.clone(),
            expires_at: state.expires_at,
            context: state.context.clone(),
            quotes,
            recommended_quote_id: recommended,
        })
    }

    pub async fn build(
        &self,
        id: Uuid,
        quote_id: Uuid,
        token: Option<&str>,
        taker: Address,
        accepted: &str,
    ) -> Result<BuildResponse, Fault> {
        let handle = self.access(id, token).await?;
        let (input, chain, quote, competition_expires) = {
            let state = handle.state.lock().await;
            let chain = state.chain.clone();
            let quote = state
                .quotes
                .iter()
                .find(|quote| quote.id == quote_id.to_string())
                .cloned()
                .ok_or_else(|| Fault::with_status("QUOTE_NOT_FOUND", 404))?;
            (state.input.clone(), chain, quote, state.expires_at)
        };
        if chain.router.is_none() {
            return Err(Fault::with_status("ROUTER_NOT_CONFIGURED", 503));
        }
        if chain.router == Some(taker) || is_reserved_address(taker) {
            return Err(Fault::new("INVALID_TAKER"));
        }
        if input.taker.is_some_and(|expected| expected != taker) {
            return Err(Fault::with_status("TAKER_MISMATCH", 403));
        }
        if quote.status != "ready" || quote.min_buy_amount.is_none() {
            return Err(Fault::with_status("QUOTE_NOT_FOUND", 404));
        }
        if quote.expires_at <= now_ms() {
            return Err(Fault::with_status("QUOTE_EXPIRED", 409));
        }
        let accepted = parse_positive(accepted)?.to_string();
        if parse_uint_checked(&accepted)?
            < parse_uint_checked(
                quote
                    .min_buy_amount
                    .as_ref()
                    .expect("ready quote has minimum"),
            )?
        {
            return Err(Fault::with_status("MINIMUM_DOWNGRADE_REJECTED", 409));
        }
        if self
            .builds
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                (value < self.config.max_active).then_some(value + 1)
            })
            .is_err()
        {
            return Err(Fault::with_status("BUILD_CAPACITY_EXCEEDED", 429));
        }
        let result = self
            .build_inner(
                &input,
                &chain,
                &quote,
                taker,
                &accepted,
                competition_expires,
            )
            .await;
        self.builds.fetch_sub(1, Ordering::AcqRel);
        result
    }

    async fn build_inner(
        &self,
        input: &Input,
        chain: &Chain,
        quote: &Quote,
        taker: Address,
        accepted: &str,
        competition_expires: u64,
    ) -> Result<BuildResponse, Fault> {
        let provider = self
            .services
            .providers
            .iter()
            .find(|provider| provider.id() == quote.provider)
            .ok_or_else(|| Fault::with_status("QUOTE_NOT_FOUND", 404))?
            .clone();
        let sender = chain.router.expect("build checked router");
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let context_future = self.services.context.get(input, chain);
        let route_future = provider.quote(input, chain, sender);
        let (context, route) = tokio::time::timeout(timeout, async {
            tokio::try_join!(context_future, route_future)
        })
        .await
        .map_err(|_| Fault::with_status("UPSTREAM_TIMEOUT", 504))??;
        validate_route(input, chain, &route, true)?;
        if parse_uint_checked(&route.buy_amount)? < parse_uint_checked(accepted)? {
            return Err(Fault::with_status(
                "PRICE_MOVED_BELOW_ACCEPTED_MINIMUM",
                409,
            ));
        }
        let min = if parse_uint_checked(accepted)? > parse_uint_checked(&route.min_buy_amount)? {
            accepted.to_owned()
        } else {
            route.min_buy_amount.clone()
        };
        let simulation = tokio::time::timeout(
            timeout,
            self.services.simulator.run(SimulationRequest {
                input,
                chain,
                route: &route,
                context: &context,
                taker,
                actual: true,
                min: Some(&min),
            }),
        )
        .await
        .map_err(|_| Fault::with_status("UPSTREAM_TIMEOUT", 504))??;
        if !simulation.simulation.is_actual_success() {
            return Err(Fault::with_status(
                format!("BUILD_{}", simulation_status(&simulation.simulation)),
                422,
            ));
        }
        if route.expires_at <= now_ms() + 2_000 || competition_expires <= now_ms() {
            return Err(Fault::with_status("REQUOTE_REQUIRED", 409));
        }
        Ok(BuildResponse { chain_id: input.chain_id, taker, recipient: taker, provider: route.provider, expires_at: competition_expires.min(route.expires_at), min_buy_amount: min, approvals: simulation.approvals, transaction: simulation.transaction, simulation: simulation.simulation, context, warning: "Sign approvals sequentially, then request a fresh build before signing the swap. Simulation is not an execution guarantee.".into() })
    }

    async fn access(&self, id: Uuid, token: Option<&str>) -> Result<Arc<CompetitionHandle>, Fault> {
        self.sweep().await;
        let handle = self
            .items
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or_else(|| Fault::with_status("COMPETITION_NOT_FOUND_OR_EXPIRED", 404))?;
        let state = handle.state.lock().await;
        let valid = constant_time_token_eq(token, &state.token);
        drop(state);
        if !valid {
            return Err(Fault::with_status("INVALID_ACCESS_TOKEN", 401));
        }
        Ok(handle)
    }

    async fn sweep(&self) {
        let now = now_ms();
        let expired = {
            let handles = self
                .items
                .lock()
                .await
                .values()
                .cloned()
                .collect::<Vec<_>>();
            let mut expired_ids = Vec::new();
            for handle in handles {
                let state = handle.state.lock().await;
                if state.expires_at <= now {
                    expired_ids.push(state.id);
                }
            }
            let mut items = self.items.lock().await;
            expired_ids
                .into_iter()
                .filter_map(|id| items.remove(&id))
                .collect::<Vec<_>>()
        };
        for handle in expired {
            handle.cancel.store(true, Ordering::Release);
        }
    }

    pub async fn close(&self) {
        let handles = self
            .items
            .lock()
            .await
            .drain()
            .map(|(_, handle)| handle)
            .collect::<Vec<_>>();
        for handle in handles {
            handle.cancel.store(true, Ordering::Release);
        }
    }

    async fn run(&self, handle: Arc<CompetitionHandle>) {
        let _ = self.run_live(&handle).await;
        if !handle.cancel.load(Ordering::Acquire) {
            let mut state = handle.state.lock().await;
            state.status = "complete".into();
        }
    }

    async fn run_live(&self, handle: &Arc<CompetitionHandle>) -> Result<(), Fault> {
        let (input, chain, expires_at) = {
            let state = handle.state.lock().await;
            (state.input.clone(), state.chain.clone(), state.expires_at)
        };
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let context =
            match tokio::time::timeout(timeout, self.services.context.get(&input, &chain)).await {
                Ok(Ok(context)) => {
                    handle.state.lock().await.context = Some(context.clone());
                    Some(context)
                }
                Ok(Err(_)) | Err(_) => None,
            };
        let futures = self.services.providers.iter().map(|provider| {
            let input = input.clone();
            let chain = chain.clone();
            let context = context.clone();
            let handle = handle.clone();
            async move {
                let started = now_ms();
                let mut quote = Quote {
                    id: Uuid::new_v4().to_string(),
                    provider: provider.id(),
                    status: "unavailable".into(),
                    quoted_amount: None,
                    min_buy_amount: None,
                    simulation: None,
                    net_output: None,
                    latency_ms: 0,
                    expires_at,
                    execution: if chain.router.is_some() {
                        "unified".into()
                    } else {
                        "direct-preview".into()
                    },
                    error: None,
                };
                let sender = chain.router.unwrap_or(input.taker.unwrap_or(PREVIEW_TAKER));
                let route = if !provider.enabled() {
                    Err(Fault::with_status("PROVIDER_UNCONFIGURED", 503))
                } else {
                    tokio::time::timeout(timeout, provider.quote(&input, &chain, sender))
                        .await
                        .map_err(|_| Fault::with_status("UPSTREAM_TIMEOUT", 504))?
                };
                match route
                    .and_then(|route| validate_route(&input, &chain, &route, false).map(|_| route))
                {
                    Ok(route) => {
                        quote.status = "ready".into();
                        quote.quoted_amount = Some(route.buy_amount.clone());
                        quote.min_buy_amount = Some(route.min_buy_amount.clone());
                        quote.expires_at = expires_at.min(route.expires_at);
                        if let Some(context) = context {
                            match tokio::time::timeout(
                                timeout,
                                self.services.simulator.run(SimulationRequest {
                                    input: &input,
                                    chain: &chain,
                                    route: &route,
                                    context: &context,
                                    taker: input.taker.unwrap_or(PREVIEW_TAKER),
                                    actual: input.taker.is_some(),
                                    min: None,
                                }),
                            )
                            .await
                            {
                                Ok(Ok(result)) => {
                                    quote.simulation = Some(result.simulation.clone());
                                    if let Simulation::Success {
                                        bought_amount,
                                        gas_fee_wei,
                                        ..
                                    } = &result.simulation
                                    {
                                        quote.net_output = Some(net_output(
                                            bought_amount,
                                            gas_fee_wei.as_deref(),
                                            &context,
                                            chain
                                                .tokens
                                                .iter()
                                                .find(|token| token.address == input.buy_token)
                                                .map(|token| token.decimals)
                                                .unwrap_or_default(),
                                        )?);
                                    }
                                }
                                Ok(Err(error)) => {
                                    quote.simulation = Some(Simulation::Error {
                                        reason: safe_error(&error),
                                    })
                                }
                                Err(_) => {
                                    quote.simulation = Some(Simulation::Error {
                                        reason: "UPSTREAM_TIMEOUT".into(),
                                    })
                                }
                            }
                        } else {
                            quote.simulation = Some(Simulation::Unsupported {
                                reason: "SIMULATION_CONTEXT_UNAVAILABLE".into(),
                            });
                        }
                    }
                    Err(error) => {
                        quote.error = Some(safe_error(&error));
                        quote.status = if quote.error.as_deref() == Some("PROVIDER_UNCONFIGURED") {
                            "unavailable".into()
                        } else {
                            "error".into()
                        };
                    }
                }
                quote.latency_ms = now_ms().saturating_sub(started);
                handle.state.lock().await.quotes.push(quote);
                Ok::<(), Fault>(())
            }
        });
        let _ = join_all(futures).await;
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct CreateResponse {
    pub id: Uuid,
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub id: Uuid,
    pub input: Input,
    pub status: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
    pub context: Option<Context>,
    pub quotes: Vec<Quote>,
    #[serde(rename = "recommendedQuoteId")]
    pub recommended_quote_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BuildResponse {
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    pub taker: Address,
    pub recipient: Address,
    pub provider: ProviderId,
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
    #[serde(rename = "minBuyAmount")]
    pub min_buy_amount: String,
    pub approvals: Vec<Tx>,
    pub transaction: Tx,
    pub simulation: Simulation,
    pub context: Context,
    pub warning: String,
}

fn constant_time_token_eq(token: Option<&str>, expected: &str) -> bool {
    let Some(token) = token else {
        return false;
    };
    if token.len() != expected.len()
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return false;
    }
    token.as_bytes().ct_eq(expected.as_bytes()).into()
}

fn is_verified(quote: &Quote) -> bool {
    quote.expires_at > now_ms()
        && quote
            .simulation
            .as_ref()
            .is_some_and(Simulation::is_success)
        && quote.net_output.as_ref().is_some_and(Option::is_some)
}

fn parse_uint_checked(value: &str) -> Result<alloy_primitives::U256, Fault> {
    crate::domain::parse_uint(value)
}

fn simulation_status(simulation: &Simulation) -> &'static str {
    match simulation {
        Simulation::Success { .. } => "SUCCESS",
        Simulation::Reverted { .. } => "REVERTED",
        Simulation::Unsupported { .. } => "UNSUPPORTED",
        Simulation::Error { .. } => "ERROR",
    }
}

fn safe_error(error: &Fault) -> String {
    error.code.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{NATIVE, Route, Rule},
        execution::swap_transaction,
        simulation::SimResult,
    };
    use async_trait::async_trait;
    use std::collections::HashMap;

    struct MockContext;
    #[async_trait]
    impl ContextProvider for MockContext {
        async fn get(&self, _input: &Input, _chain: &Chain) -> Result<Context, Fault> {
            Ok(Context {
                block_number: "0x10".into(),
                block_hash: format!("0x{}", "11".repeat(32)),
                timestamp: 100,
                gas_price: "1".into(),
                native_usd: Some("1".into()),
                buy_usd: Some("1".into()),
            })
        }
    }
    struct MockSim;
    #[async_trait]
    impl SimulationProvider for MockSim {
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
                    request.min,
                )
                .unwrap(),
            })
        }
    }
    struct MockProvider;
    #[async_trait]
    impl Provider for MockProvider {
        fn id(&self) -> ProviderId {
            ProviderId::Kyber
        }
        fn enabled(&self) -> bool {
            true
        }
        async fn quote(
            &self,
            input: &Input,
            _chain: &Chain,
            sender: Address,
        ) -> Result<Route, Fault> {
            Ok(Route {
                provider: ProviderId::Kyber,
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

    fn config() -> Config {
        crate::config::load_config(&HashMap::new()).unwrap()
    }
    fn services() -> Services {
        Services {
            providers: vec![Arc::new(MockProvider)],
            context: Arc::new(MockContext),
            simulator: Arc::new(MockSim),
        }
    }

    async fn wait_complete(service: &Competitions, created: &CreateResponse) -> Snapshot {
        for _ in 0..100 {
            let state = service
                .get(created.id, Some(&created.access_token))
                .await
                .unwrap();
            if state.status == "complete" {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("competition did not complete")
    }

    #[tokio::test]
    async fn create_completes_with_one_result_per_provider() {
        let service = Competitions::new(config(), None);
        let chain = service.config.chains[0].clone();
        let input = Input {
            chain_id: 1,
            sell_token: NATIVE,
            buy_token: chain.tokens[2].address,
            sell_amount: "1000000000000000000".into(),
            slippage_bps: 30,
            taker: None,
        };
        let created = service.create(input).await.unwrap();
        let state = wait_complete(&service, &created).await;
        assert_eq!(state.status, "complete");
        assert_eq!(state.quotes.len(), 3);
        assert!(state.recommended_quote_id.is_none());
        service.close().await;
    }

    #[tokio::test]
    async fn provider_failure_isolated_from_available_provider() {
        let mut config = config();
        config.chains[0].router = Some(
            crate::domain::parse_address("0x2222222222222222222222222222222222222222").unwrap(),
        );
        config.chains[0].rules.insert(
            ProviderId::Kyber,
            vec![Rule {
                target: crate::domain::parse_address("0x2222222222222222222222222222222222222222")
                    .unwrap(),
                spender: crate::domain::parse_address("0x2222222222222222222222222222222222222222")
                    .unwrap(),
                selector: "0x12345678".into(),
            }],
        );
        let service = Competitions::new(config, Some(services()));
        let chain = service.config.chains[0].clone();
        let input = Input {
            chain_id: 1,
            sell_token: NATIVE,
            buy_token: chain.tokens[2].address,
            sell_amount: "1".into(),
            slippage_bps: 30,
            taker: None,
        };
        let created = service.create(input).await.unwrap();
        let state = wait_complete(&service, &created).await;
        assert_eq!(state.quotes.len(), 1);
        assert!(state.quotes[0].simulation.as_ref().unwrap().is_success());
        service.close().await;
    }
}
