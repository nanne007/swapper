use crate::error::ErrorKind;
use crate::{
    chains::configured_chains,
    config::Config,
    domain::{
        Address, Chain, Context, Input, PREVIEW_TAKER, Quote, Simulation, Tx, is_reserved_address,
        now_ms, parse_positive, parse_uint, rank,
    },
    execution::validate_route,
    http::ReqwestClient,
    providers::{Provider, ProviderRegistry, create_providers},
    rpc::{ContextProvider, ContextSource, RpcClients},
    simulation::{SimulationFailure, SimulationProvider, SimulationRequest, Simulator},
};
use anyhow::Context as _;
use futures::future::join_all;
use serde::Serialize;
use std::{collections::HashMap, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, Semaphore};
use tracing::Instrument;
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
}

#[derive(Clone)]
pub struct Services {
    pub registry: ProviderRegistry,
    chains: Vec<Chain>,
    pub context: Arc<dyn ContextProvider>,
    pub simulator: Arc<dyn SimulationProvider>,
}

impl Services {
    pub fn new(
        providers: Vec<Arc<dyn Provider>>,
        chains: &[Chain],
        context: Arc<dyn ContextProvider>,
        simulator: Arc<dyn SimulationProvider>,
    ) -> Self {
        let registry = ProviderRegistry::new(providers);
        let chains = chains
            .iter()
            .filter(|chain| registry.by_chain().contains_key(&chain.id))
            .cloned()
            .collect();
        Self {
            registry,
            chains,
            context,
            simulator,
        }
    }

    pub fn production(config: &Config) -> Self {
        let client = Arc::new(ReqwestClient::default());
        let providers = create_providers(config, client.clone());
        let registry = ProviderRegistry::new(providers);
        let chains = configured_chains(config, registry.chain_ids());
        let clients = Arc::new(RpcClients::new(Duration::from_millis(config.timeout_ms)));
        Self {
            registry,
            chains,
            context: Arc::new(ContextSource::new(clients.clone())),
            simulator: Arc::new(Simulator::new(clients)),
        }
    }
}

#[derive(Clone)]
pub struct Competitions {
    config: Arc<Config>,
    services: Services,
    items: Arc<Mutex<HashMap<Uuid, Arc<CompetitionHandle>>>>,
    active: Arc<Semaphore>,
    builds: Arc<Semaphore>,
}

impl Competitions {
    pub fn new(config: Config, services: Option<Services>) -> Self {
        let services = services.unwrap_or_else(|| Services::production(&config));
        let active = Arc::new(Semaphore::new(config.max_active));
        let builds = Arc::new(Semaphore::new(config.max_active));
        Self {
            config: Arc::new(config),
            services,
            items: Arc::new(Mutex::new(HashMap::new())),
            active,
            builds,
        }
    }

    pub fn provider_ids_for_chain(&self, chain_id: u64) -> Vec<&'static str> {
        self.services
            .registry
            .by_chain()
            .get(&chain_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn chains(&self) -> &[Chain] {
        &self.services.chains
    }

    pub async fn create(&self, input: Input) -> anyhow::Result<CreateResponse> {
        self.sweep().await;
        let mut items = self.items.lock().await;
        if items.len() >= self.config.max_competitions {
            anyhow::bail!(ErrorKind::CapacityExceeded);
        }
        let permit = self
            .active
            .clone()
            .try_acquire_owned()
            .context(ErrorKind::CapacityExceeded)?;
        let chain = self
            .services
            .chains
            .iter()
            .find(|chain| chain.id == input.chain_id)
            .cloned()
            .context(ErrorKind::InvalidInput)?;
        if input
            .taker
            .is_some_and(|taker| chain.router == Some(taker) || is_reserved_address(taker))
        {
            anyhow::bail!(ErrorKind::InvalidTaker);
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
        });
        items.insert(id, handle.clone());
        let service = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            service.run(handle).await;
        });
        Ok(CreateResponse {
            id,
            access_token: token,
            expires_at,
        })
    }

    pub async fn get(&self, id: Uuid, token: Option<&str>) -> anyhow::Result<Snapshot> {
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
    ) -> anyhow::Result<BuildResponse> {
        let handle = self.access(id, token).await?;
        let (input, chain, quote, competition_expires) = {
            let state = handle.state.lock().await;
            let chain = state.chain.clone();
            let quote = state
                .quotes
                .iter()
                .find(|quote| quote.id == quote_id.to_string())
                .cloned()
                .context(ErrorKind::QuoteNotFound)?;
            (state.input.clone(), chain, quote, state.expires_at)
        };
        if chain.router.is_none() {
            anyhow::bail!(ErrorKind::RouterNotConfigured);
        }
        if chain.router == Some(taker) || is_reserved_address(taker) {
            anyhow::bail!(ErrorKind::InvalidTaker);
        }
        if input.taker.is_some_and(|expected| expected != taker) {
            anyhow::bail!(ErrorKind::TakerMismatch);
        }
        if quote.status != "ready" || quote.min_buy_amount.is_none() {
            anyhow::bail!(ErrorKind::QuoteNotFound);
        }
        if quote.expires_at <= now_ms() {
            anyhow::bail!(ErrorKind::QuoteExpired);
        }
        let accepted = parse_positive(accepted)?.to_string();
        if parse_uint(&accepted)?
            < parse_uint(
                quote
                    .min_buy_amount
                    .as_ref()
                    .expect("ready quote has minimum"),
            )?
        {
            anyhow::bail!(ErrorKind::MinimumDowngradeRejected);
        }
        let _permit = self
            .builds
            .clone()
            .try_acquire_owned()
            .context(ErrorKind::BuildCapacityExceeded)?;
        self.build_inner(
            &input,
            &chain,
            &quote,
            taker,
            &accepted,
            competition_expires,
        )
        .await
    }

    async fn build_inner(
        &self,
        input: &Input,
        chain: &Chain,
        quote: &Quote,
        taker: Address,
        accepted: &str,
        competition_expires: u64,
    ) -> anyhow::Result<BuildResponse> {
        let provider = self
            .services
            .registry
            .get(quote.provider)
            .context(ErrorKind::QuoteNotFound)?;
        let sender = chain.router.expect("build checked router");
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let context_future = self.services.context.get(input, chain);
        let route_future = provider.quote(input, chain, sender);
        let rules = provider.rules(chain.id);
        let (context, route) = tokio::time::timeout(timeout, async {
            tokio::try_join!(context_future, route_future)
        })
        .await
        .context(ErrorKind::UpstreamTimeout)??;
        validate_route(input, chain, &route, &rules, true)?;
        if parse_uint(&route.buy_amount)? < parse_uint(accepted)? {
            anyhow::bail!(ErrorKind::PriceMovedBelowAcceptedMinimum);
        }
        let min = if parse_uint(accepted)? > parse_uint(&route.min_buy_amount)? {
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
                rules: &rules,
                taker,
                actual: true,
                min: Some(&min),
            }),
        )
        .await
        .context(ErrorKind::UpstreamTimeout)?
        .map_err(|error| {
            let Some(failure) = error.downcast_ref::<SimulationFailure>() else {
                return error;
            };
            let kind = match failure {
                SimulationFailure::Reverted(_) => ErrorKind::BuildReverted,
                SimulationFailure::Unsupported(_) => ErrorKind::BuildUnsupported,
                SimulationFailure::Error(_) => ErrorKind::BuildError,
            };
            error.context(kind).context("build simulation")
        })?;
        if !simulation.simulation.is_actual_success() {
            anyhow::bail!(ErrorKind::InternalError);
        }
        if route.expires_at <= now_ms() + 2_000 || competition_expires <= now_ms() {
            anyhow::bail!(ErrorKind::RequoteRequired);
        }
        Ok(BuildResponse { chain_id: input.chain_id, taker, recipient: taker, provider: route.provider, expires_at: competition_expires.min(route.expires_at), min_buy_amount: min, approvals: simulation.approvals, transaction: simulation.transaction, simulation: simulation.simulation, context, warning: "Sign approvals sequentially, then request a fresh build before signing the swap. Simulation is not an execution guarantee.".into() })
    }

    async fn access(
        &self,
        id: Uuid,
        token: Option<&str>,
    ) -> anyhow::Result<Arc<CompetitionHandle>> {
        self.sweep().await;
        let handle = self
            .items
            .lock()
            .await
            .get(&id)
            .cloned()
            .context(ErrorKind::CompetitionNotFoundOrExpired)?;
        let state = handle.state.lock().await;
        let valid = constant_time_token_eq(token, &state.token);
        drop(state);
        if !valid {
            anyhow::bail!(ErrorKind::InvalidAccessToken);
        }
        Ok(handle)
    }

    async fn sweep(&self) {
        let now = now_ms();
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
        for id in expired_ids {
            items.remove(&id);
        }
    }

    pub async fn close(&self) {
        // In-flight work remains bounded by its deadlines; discarded snapshots are inaccessible.
        self.items.lock().await.clear();
    }

    async fn run(&self, handle: Arc<CompetitionHandle>) {
        let competition_id = handle.state.lock().await.id;
        self.run_live(&handle)
            .instrument(tracing::info_span!("competition", %competition_id))
            .await;
        handle.state.lock().await.status = "complete".into();
    }

    async fn run_live(&self, handle: &Arc<CompetitionHandle>) {
        let (input, chain, expires_at, competition_id) = {
            let state = handle.state.lock().await;
            (
                state.input.clone(),
                state.chain.clone(),
                state.expires_at,
                state.id,
            )
        };
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let context =
            match tokio::time::timeout(timeout, self.services.context.get(&input, &chain)).await {
                Ok(Ok(context)) => {
                    handle.state.lock().await.context = Some(context.clone());
                    Some(context)
                }
                Ok(Err(error)) => {
                    tracing::warn!(error = ?error, "competition context failed");
                    None
                }
                Err(error) => {
                    tracing::warn!(error = ?error, "competition context timed out");
                    None
                }
            };
        let providers = self.services.registry.for_chain(chain.id);
        let futures = providers.iter().map(|provider| {
            let input = input.clone();
            let chain = chain.clone();
            let context = context.clone();
            let handle = handle.clone();
            let span = tracing::info_span!("provider", %competition_id, chain_id = chain.id, provider = provider.id());
            async move {
                let started = now_ms();
                let mut quote = Quote {
                    id: Uuid::new_v4().to_string(),
                    provider: provider.id(),
                    status: "unavailable".into(),
                    quoted_amount: None,
                    min_buy_amount: None,
                    simulation: None,
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
                let route = tokio::time::timeout(timeout, provider.quote(&input, &chain, sender))
                    .await
                    .context(ErrorKind::UpstreamTimeout).context("provider quote deadline")
                    .and_then(|result| result);
                let rules = provider.rules(chain.id);
                match route.and_then(|route| {
                    validate_route(&input, &chain, &route, &rules, false).map(|_| route)
                }) {
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
                                    rules: &rules,
                                    taker: input.taker.unwrap_or(PREVIEW_TAKER),
                                    actual: input.taker.is_some(),
                                    min: None,
                                }),
                            )
                            .await
                            {
                                Ok(Ok(result)) => {
                                    quote.simulation = Some(result.simulation);
                                }
                                Ok(Err(error)) => {
                                    tracing::warn!(error = ?error, "quote simulation failed");
                                    quote.simulation = Some(match error.downcast_ref::<SimulationFailure>() {
                                        Some(failure) => (*failure).into(),
                                        None => Simulation::Error { reason: crate::api_error::ApiError::from(&error).code.into() },
                                    });
                                }
                                Err(error) => {
                                    quote.simulation = Some(Simulation::Error {
                                        reason: quote_error_code(&anyhow::Error::new(error).context(ErrorKind::UpstreamTimeout).context("quote simulation deadline")),
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
                        quote.error = Some(quote_error_code(&error));
                        quote.status = if quote.error.as_deref() == Some("PROVIDER_UNCONFIGURED") {
                            "unavailable".into()
                        } else {
                            "error".into()
                        };
                    }
                }
                quote.latency_ms = now_ms().saturating_sub(started);
                handle.state.lock().await.quotes.push(quote);
            }.instrument(span)
        });
        join_all(futures).await;
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateResponse {
    pub id: Uuid,
    pub access_token: String,
    pub expires_at: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: Uuid,
    pub input: Input,
    pub status: String,
    pub expires_at: u64,
    pub context: Option<Context>,
    pub quotes: Vec<Quote>,
    pub recommended_quote_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResponse {
    pub chain_id: u64,
    pub taker: Address,
    pub recipient: Address,
    pub provider: &'static str,
    pub expires_at: u64,
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
        && quote.quoted_amount.is_some()
}

fn quote_error_code(error: &anyhow::Error) -> String {
    tracing::warn!(error = ?error, "quote failed");
    crate::api_error::ApiError::from(error).code.to_owned()
}
