mod support;

use alloy_primitives::Address;
use async_trait::async_trait;
use metamatch_backend::{
    competitions::{Competitions, FailureStatus, Services},
    domain::{Chain, Context, Input, NATIVE, Route, Rule, SimulationSuccess},
    error::{ErrorKind, kind},
    providers::Provider,
    rpc::ContextProvider,
    simulation::{SimResult, SimulationProvider, SimulationRequest},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::time::{Instant, sleep};

struct Competitor {
    id: &'static str,
    delay_ms: u64,
    error: bool,
    dropped: Arc<AtomicUsize>,
}

struct InFlight(Arc<AtomicUsize>);
impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl Provider for Competitor {
    fn id(&self) -> &'static str {
        self.id
    }
    fn requires_access_key(&self) -> bool {
        false
    }
    fn supported_chains(&self) -> Vec<u64> {
        vec![1]
    }
    fn rules(&self, _: u64) -> Vec<Rule> {
        vec![support::fixture_rule(Address::repeat_byte(0x22))]
    }
    async fn quote(&self, input: &Input, chain: &Chain, sender: Address) -> anyhow::Result<Route> {
        let _in_flight = InFlight(self.dropped.clone());
        assert_eq!(sender, chain.router.unwrap());
        assert_eq!(input.taker, support::input(1, NATIVE).taker);
        sleep(Duration::from_millis(self.delay_ms)).await;
        if self.error {
            anyhow::bail!(ErrorKind::UpstreamHttpError);
        }
        Ok(Route {
            provider: self.id,
            buy_amount: if self.id == "a" {
                "9007199254740993999"
            } else {
                "9007199254740993002"
            }
            .into(),
            min_buy_amount: "1".into(),
            sell_amount: input.sell_amount.clone(),
            spender: sender,
            tx: metamatch_backend::domain::Tx {
                to: sender,
                data: "0x12345678".into(),
                value: input.sell_amount.clone(),
            },
            deadline: None,
        })
    }
}

struct DelayedContext(u64);
#[async_trait]
impl ContextProvider for DelayedContext {
    async fn get(&self, _: &Input, _: &Chain) -> anyhow::Result<Context> {
        sleep(Duration::from_millis(self.0)).await;
        Ok(support::fixture_context())
    }
}

struct SimulationFixture {
    dropped: Arc<AtomicUsize>,
    below_minimum: bool,
}
#[async_trait]
impl SimulationProvider for SimulationFixture {
    async fn run(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
        let _in_flight = InFlight(self.dropped.clone());
        assert_eq!(request.taker, request.input.taker);
        assert_eq!(
            request.context.block_context(),
            support::fixture_context().block_context()
        );
        sleep(Duration::from_millis(
            if request.route.provider == "slow-simulation" {
                80
            } else {
                10
            },
        ))
        .await;
        let mut result = support::MockSimulation.run(request).await?;
        let SimulationSuccess {
            bought_amount,
            gas_fee_wei,
            ..
        } = &mut result.simulation;
        {
            *bought_amount = if self.below_minimum {
                "100"
            } else if bought_amount == "9007199254740993999" {
                "9007199254740993001"
            } else {
                "9007199254740993002"
            }
            .into();
            *gas_fee_wei = None;
        }
        Ok(result)
    }
}

fn service(
    specs: &[(&'static str, u64, bool)],
    context_delay: u64,
) -> (Competitions, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let config = support::config_with(&[("COMPETITION_TIMEOUT_MS", "100")]);
    let mut chain = support::chain(&config, 1);
    chain.router = Some(Address::repeat_byte(0x22));
    let quote_drops = Arc::new(AtomicUsize::new(0));
    let simulation_drops = Arc::new(AtomicUsize::new(0));
    let providers = specs
        .iter()
        .map(|&(id, delay_ms, error)| {
            Arc::new(Competitor {
                id,
                delay_ms,
                error,
                dropped: quote_drops.clone(),
            }) as Arc<dyn Provider>
        })
        .collect();
    let services = Services::new(
        providers,
        &[chain],
        Arc::new(DelayedContext(context_delay)),
        Arc::new(SimulationFixture {
            dropped: simulation_drops.clone(),
            below_minimum: false,
        }),
    );
    (
        Competitions::new(config, Some(services)),
        quote_drops,
        simulation_drops,
    )
}

#[tokio::test(start_paused = true)]
async fn ranks_simulated_balance_delta_as_integers_and_finishes_early() {
    let (service, _, _) = service(&[("a", 0, false), ("c", 5, false), ("b", 10, false)], 0);
    let started = Instant::now();
    let result = service.create(support::input(1, NATIVE)).await.unwrap();
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(
        result
            .quotes
            .iter()
            .map(|q| q.route.provider)
            .collect::<Vec<_>>(),
        ["b", "c", "a"]
    );
    assert!(
        result
            .quotes
            .iter()
            .all(|q| q.simulation.funding == "overridden" && !q.transaction.data.is_empty())
    );
    // Fixed old block timestamps do not expire results; fees are not needed to rank output.
    assert_eq!(
        serde_json::to_value(&result.quotes[0].simulation).unwrap()["blockContext"]["timestamp"],
        100
    );
}

#[tokio::test(start_paused = true)]
async fn one_deadline_covers_context_quote_and_simulation_and_keeps_completed_results() {
    let (service, quotes_dropped, simulations_dropped) = service(
        &[
            ("fast", 10, false),
            ("slow-quote", 200, false),
            ("slow-simulation", 30, false),
            ("failed", 1, true),
        ],
        40,
    );
    let started = Instant::now();
    let result = service.create(support::input(1, NATIVE)).await.unwrap();
    assert_eq!(started.elapsed(), Duration::from_millis(100));
    assert_eq!(result.quotes.len(), 1);
    assert_eq!(result.failures.len(), 3);
    assert_eq!(result.quotes[0].route.provider, "fast");
    for id in ["slow-quote", "slow-simulation"] {
        let failure = result.failures.iter().find(|q| q.provider == id).unwrap();
        assert_eq!(failure.error, "UPSTREAM_TIMEOUT");
    }
    assert_eq!(
        result
            .failures
            .iter()
            .find(|q| q.provider == "failed")
            .unwrap()
            .error,
        "UPSTREAM_HTTP_ERROR"
    );
    let wire = serde_json::to_value(&result).unwrap();
    assert!(wire["quotes"][0].get("error").is_none());
    assert!(wire["quotes"][0].get("status").is_none());
    assert!(wire["quotes"][0]["route"].is_object());
    for failure in wire["failures"].as_array().unwrap() {
        assert!(failure.get("route").is_none());
        assert!(failure.get("transaction").is_none());
        assert!(failure.get("approvals").is_none());
    }
    assert_eq!(quotes_dropped.load(Ordering::SeqCst), 4);
    assert_eq!(simulations_dropped.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn context_timeout_reports_all_providers_without_starting_more_work() {
    let (service, quote_drops, _) = service(&[("a", 0, false), ("b", 0, false)], 200);
    let started = Instant::now();
    let result = service.create(support::input(1, NATIVE)).await.unwrap();
    assert_eq!(started.elapsed(), Duration::from_millis(100));
    assert!(result.quotes.is_empty());
    assert_eq!(result.failures.len(), 2);
    assert!(
        result
            .failures
            .iter()
            .all(|q| q.error == "UPSTREAM_TIMEOUT")
    );
    assert_eq!(quote_drops.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn missing_router_and_context_failure_never_return_executable_transactions() {
    struct FailedContext;
    #[async_trait]
    impl ContextProvider for FailedContext {
        async fn get(&self, _: &Input, _: &Chain) -> anyhow::Result<Context> {
            anyhow::bail!(ErrorKind::RpcNotConfigured)
        }
    }
    for (router, code) in [
        (None, "ROUTER_NOT_CONFIGURED"),
        (Some(Address::repeat_byte(0x22)), "RPC_NOT_CONFIGURED"),
    ] {
        let config = support::config();
        let mut services = support::competition_services_for(&config, router);
        services.context = Arc::new(FailedContext);
        let result = Competitions::new(config, Some(services))
            .create(support::input(1, NATIVE))
            .await
            .unwrap();
        assert!(result.quotes.is_empty());
        assert_eq!(result.failures[0].status, FailureStatus::Unavailable);
        assert_eq!(result.failures[0].error, code);
    }
}

#[tokio::test(start_paused = true)]
async fn invalid_output_cannot_become_executable() {
    let config = support::config();
    let mut services = support::competition_services_for(&config, Some(Address::repeat_byte(0x22)));
    services.simulator = Arc::new(SimulationFixture {
        dropped: Arc::new(AtomicUsize::new(0)),
        below_minimum: true,
    });
    let result = Competitions::new(config, Some(services))
        .create(support::input(1, NATIVE))
        .await
        .unwrap();
    assert!(result.quotes.is_empty());
    assert_eq!(result.failures[0].status, FailureStatus::Error);
}

#[tokio::test]
async fn taker_cannot_be_the_router_and_route_must_be_allowlisted() {
    let config = support::config();
    let router = Address::repeat_byte(0x22);
    let service = Competitions::new(
        config.clone(),
        Some(support::competition_services_for(&config, Some(router))),
    );
    let mut input = support::input(1, NATIVE);
    input.taker = router;
    assert_eq!(
        kind(&service.create(input).await.unwrap_err()),
        ErrorKind::InvalidTaker
    );
    struct Unallowlisted(support::MockProvider);
    #[async_trait]
    impl Provider for Unallowlisted {
        fn id(&self) -> &'static str {
            "kyber"
        }
        fn requires_access_key(&self) -> bool {
            false
        }
        fn supported_chains(&self) -> Vec<u64> {
            vec![1]
        }
        async fn quote(
            &self,
            input: &Input,
            chain: &Chain,
            sender: Address,
        ) -> anyhow::Result<Route> {
            self.0.quote(input, chain, sender).await
        }
    }
    let mut chain = support::chain(&config, 1);
    chain.router = Some(router);
    let services = Services::new(
        vec![Arc::new(Unallowlisted(support::MockProvider {
            rule: support::fixture_rule(router),
        }))],
        &[chain],
        Arc::new(support::MockContext),
        Arc::new(support::MockSimulation),
    );
    let result = Competitions::new(config, Some(services))
        .create(support::input(1, NATIVE))
        .await
        .unwrap();
    assert_eq!(result.failures[0].error, "ROUTE_NOT_ALLOWLISTED");
    assert!(result.quotes.is_empty());
}
