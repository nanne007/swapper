use crate::error::ErrorKind;
use crate::{
    balance_slots::BalanceSlots,
    chains::configured_chains,
    config::Config,
    domain::{Chain, Context, Input, Quote, is_reserved_address, parse_positive, parse_uint, rank},
    execution::validate_route,
    http::ReqwestClient,
    providers::{Provider, ProviderRegistry, create_providers},
    rpc::{ContextProvider, ContextSource, RpcClients},
    simulation::{SimulationFailure, SimulationProvider, SimulationRequest, Simulator},
};
use anyhow::Context as _;
use futures::future::join_all;
use serde::Serialize;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::Semaphore,
    time::{Instant, timeout_at},
};
use tracing::Instrument;
use uuid::Uuid;

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
            simulator: Arc::new(Simulator::new(
                clients,
                Arc::new(BalanceSlots::new(config.balance_slots.clone())),
            )),
        }
    }
}

#[derive(Clone)]
pub struct Competitions {
    config: Arc<Config>,
    services: Services,
    active: Arc<Semaphore>,
}

impl Competitions {
    pub fn new(config: Config, services: Option<Services>) -> Self {
        let services = services.unwrap_or_else(|| Services::production(&config));
        let active = Arc::new(Semaphore::new(config.max_active));
        Self {
            config: Arc::new(config),
            services,
            active,
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

    pub async fn create(&self, input: Input) -> anyhow::Result<CompetitionResponse> {
        let started = Instant::now();
        let deadline = started + Duration::from_millis(self.config.timeout_ms);
        let chain = self
            .services
            .chains
            .iter()
            .find(|chain| chain.id == input.chain_id)
            .context(ErrorKind::InvalidInput)?;
        if chain.router == Some(input.taker) || is_reserved_address(input.taker) {
            anyhow::bail!(ErrorKind::InvalidTaker);
        }
        let _permit = self
            .active
            .clone()
            .try_acquire_owned()
            .context(ErrorKind::CapacityExceeded)?;
        let id = Uuid::new_v4();
        async {
            let providers = self.services.registry.for_chain(chain.id);
            let context = timeout_at(deadline, async {
                if chain.router.is_none() {
                    anyhow::bail!(ErrorKind::RouterNotConfigured);
                }
                self.services.context.get(&input, chain).await
            })
            .await
            .context(ErrorKind::UpstreamTimeout)
            .and_then(|result| result);
            let outcomes = match context {
                Ok(context) => {
                    join_all(providers.iter().map(|provider| {
                        self.compete(provider.as_ref(), &input, chain, &context, deadline)
                            .instrument(tracing::info_span!("provider", provider = provider.id()))
                    }))
                    .await
                }
                Err(error) => {
                    let code = quote_error_code(&error);
                    providers
                        .iter()
                        .map(|provider| {
                            Err(provider_failure(
                                provider.id(),
                                &error,
                                code.clone(),
                                started.elapsed().as_millis() as u64,
                            ))
                        })
                        .collect()
                }
            };
            let mut quotes = Vec::new();
            let mut failures = Vec::new();
            for outcome in outcomes {
                match outcome {
                    Ok(quote) => quotes.push(quote),
                    Err(failure) => failures.push(failure),
                }
            }
            failures.sort_by_key(|failure| failure.provider);
            Ok(CompetitionResponse {
                id,
                input: input.clone(),
                quotes: rank(quotes)?,
                failures,
            })
        }
        .instrument(tracing::info_span!("competition", %id, chain_id = input.chain_id))
        .await
    }

    async fn compete(
        &self,
        provider: &dyn Provider,
        input: &Input,
        chain: &Chain,
        context: &Context,
        deadline: Instant,
    ) -> Result<Quote, ProviderFailure> {
        let started = Instant::now();
        // Every provider's entire quote -> transaction -> simulation pipeline shares
        // the request deadline. Dropping a timed-out future cancels its remaining work.
        let result = timeout_at(deadline, async {
            let sender = chain.router.context(ErrorKind::RouterNotConfigured)?;
            let route = provider.quote(input, chain, sender).await?;
            if route.provider != provider.id() {
                anyhow::bail!(ErrorKind::InvalidRoute);
            }
            let rules = provider.rules(chain.id);
            validate_route(input, chain, &route, &rules, true)?;
            let result = self
                .services
                .simulator
                .run(SimulationRequest {
                    input,
                    chain,
                    route: &route,
                    context,
                    rules: &rules,
                    taker: input.taker,
                    min: None,
                })
                .await?;
            if parse_positive(&result.simulation.bought_amount)?
                < parse_uint(&route.min_buy_amount)?
            {
                anyhow::bail!(SimulationFailure::Reverted(
                    "SIMULATED_OUTPUT_BELOW_MINIMUM"
                ));
            }
            Ok(Quote {
                route,
                simulation: result.simulation,
                approvals: result.approvals,
                transaction: result.transaction,
                latency_ms: 0,
            })
        })
        .await
        .context(ErrorKind::UpstreamTimeout)
        .and_then(|result| result);
        let latency_ms = started.elapsed().as_millis() as u64;
        result
            .map(|mut quote| {
                quote.latency_ms = latency_ms;
                quote
            })
            .map_err(|error| {
                provider_failure(provider.id(), &error, quote_error_code(&error), latency_ms)
            })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompetitionResponse {
    pub id: Uuid,
    pub input: Input,
    pub quotes: Vec<Quote>,
    pub failures: Vec<ProviderFailure>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFailure {
    pub provider: &'static str,
    pub status: FailureStatus,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simulation: Option<SimulationFailure>,
    pub latency_ms: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureStatus {
    Unavailable,
    Error,
}

fn provider_failure(
    provider: &'static str,
    error: &anyhow::Error,
    code: String,
    latency_ms: u64,
) -> ProviderFailure {
    let simulation = error.downcast_ref::<SimulationFailure>().copied();
    let unavailable = matches!(
        crate::error::kind(error),
        ErrorKind::ProviderUnconfigured
            | ErrorKind::RouterNotConfigured
            | ErrorKind::RpcNotConfigured
    ) || matches!(simulation, Some(SimulationFailure::Unsupported(_)));
    ProviderFailure {
        provider,
        status: if unavailable {
            FailureStatus::Unavailable
        } else {
            FailureStatus::Error
        },
        error: code,
        simulation,
        latency_ms,
    }
}

fn quote_error_code(error: &anyhow::Error) -> String {
    tracing::warn!(error = ?error, "quote failed");
    if let Some(failure) = error.downcast_ref::<SimulationFailure>() {
        return failure.to_string();
    }
    crate::api_error::ApiError::from(error).code.to_owned()
}
