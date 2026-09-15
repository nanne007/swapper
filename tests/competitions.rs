mod support;

use alloy_primitives::Address;
use async_trait::async_trait;
use metamatch_backend::{
    competitions::{Competitions, FailureStatus, Services},
    domain::{Chain, Input, NATIVE, Route, SimulationSuccess},
    error::{ErrorKind, kind},
    providers::Provider,
    simulation::{SimResult, SimulationFailure, SimulationProvider, SimulationRequest},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::time::{Instant, sleep};

struct Competitor {
    id: &'static str,
    delay_ms: u64,
    error: bool,
    completed: Arc<AtomicBool>,
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

    async fn quote(&self, input: &Input, chain: &Chain, sender: Address) -> anyhow::Result<Route> {
        assert_eq!(sender, chain.router.unwrap().address);
        sleep(Duration::from_millis(self.delay_ms)).await;
        self.completed.store(true, Ordering::SeqCst);
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
                data: "0x12345678".parse().unwrap(),
                value: input.sell_amount.clone(),
            },
            deadline: None,
        })
    }
}

struct SimulationFixture {
    calls: Arc<AtomicUsize>,
    below_minimum: bool,
    slow_route_completed: Option<Arc<AtomicBool>>,
    fast_started_before_slow: Option<Arc<AtomicBool>>,
}

#[async_trait]
impl SimulationProvider for SimulationFixture {
    async fn simulate(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if request.route.provider == "fast"
            && self
                .slow_route_completed
                .as_ref()
                .is_some_and(|completed| !completed.load(Ordering::SeqCst))
            && let Some(observed) = &self.fast_started_before_slow
        {
            observed.store(true, Ordering::SeqCst);
        }
        sleep(Duration::from_millis(
            if request.route.provider == "slow-simulation" {
                80
            } else {
                10
            },
        ))
        .await;
        let mut result = support::MockSimulation.simulate(request).await?;
        let SimulationSuccess {
            bought_amount,
            gas_fee_wei,
            ..
        } = &mut result.simulation;
        *bought_amount = if self.below_minimum {
            "0"
        } else if bought_amount == "9007199254740993999" {
            "9007199254740993001"
        } else {
            "9007199254740993002"
        }
        .into();
        *gas_fee_wei = None;
        Ok(result)
    }
}

fn service(
    specs: &[(&'static str, u64, bool)],
    simulator: Arc<dyn SimulationProvider>,
) -> Competitions {
    let config = support::config_with(serde_json::json!({"competitionTimeoutMs":100}));
    let mut chain = support::chain(&config, 1);
    chain.router = Some(support::deployment(Address::repeat_byte(0x22)));
    let providers = specs
        .iter()
        .map(|&(id, delay_ms, error)| {
            Arc::new(Competitor {
                id,
                delay_ms,
                error,
                completed: Arc::new(AtomicBool::new(false)),
            }) as Arc<dyn Provider>
        })
        .collect();
    Competitions::new(config, Services::new(providers, &[chain], simulator))
}

fn fixture_simulation() -> Arc<SimulationFixture> {
    Arc::new(SimulationFixture {
        calls: Arc::new(AtomicUsize::new(0)),
        below_minimum: false,
        slow_route_completed: None,
        fast_started_before_slow: None,
    })
}

#[tokio::test(start_paused = true)]
async fn ranks_simulated_balance_delta_as_integers() {
    let simulator = fixture_simulation();
    let service = service(
        &[("a", 0, false), ("c", 5, false), ("b", 10, false)],
        simulator.clone(),
    );
    let result = service.create(support::input(1, NATIVE)).await.unwrap();
    assert_eq!(
        result
            .quotes
            .iter()
            .map(|quote| quote.route.provider)
            .collect::<Vec<_>>(),
        ["b", "c", "a"]
    );
    assert_eq!(simulator.calls.load(Ordering::SeqCst), 3);
    assert!(result.quotes.iter().all(|quote| {
        quote.simulation.funding == "overridden"
            && quote.simulation.block_context.timestamp == 101
            && !quote.transaction.data.is_empty()
    }));
    assert_eq!(result.quotes[0].latency_ms, 20);
    assert_eq!(result.quotes[1].latency_ms, 15);
    assert_eq!(result.quotes[2].latency_ms, 10);
}

#[tokio::test(start_paused = true)]
async fn each_provider_routes_then_simulates_without_waiting_for_other_routes() {
    let slow_completed = Arc::new(AtomicBool::new(false));
    let observed = Arc::new(AtomicBool::new(false));
    let simulator = Arc::new(SimulationFixture {
        calls: Arc::new(AtomicUsize::new(0)),
        below_minimum: false,
        slow_route_completed: Some(slow_completed.clone()),
        fast_started_before_slow: Some(observed.clone()),
    });
    let config = support::config_with(serde_json::json!({"competitionTimeoutMs":100}));
    let mut chain = support::chain(&config, 1);
    chain.router = Some(support::deployment(Address::repeat_byte(0x22)));
    let providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(Competitor {
            id: "fast",
            delay_ms: 1,
            error: false,
            completed: Arc::new(AtomicBool::new(false)),
        }),
        Arc::new(Competitor {
            id: "slow-route",
            delay_ms: 50,
            error: false,
            completed: slow_completed,
        }),
    ];
    let result = Competitions::new(config, Services::new(providers, &[chain], simulator))
        .create(support::input(1, NATIVE))
        .await
        .unwrap();
    assert!(result.failures.is_empty());
    assert!(observed.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn one_deadline_covers_each_provider_route_and_simulation() {
    let service = service(
        &[
            ("fast", 10, false),
            ("slow-simulation", 30, false),
            ("failed", 1, true),
        ],
        fixture_simulation(),
    );
    let started = Instant::now();
    let result = service.create(support::input(1, NATIVE)).await.unwrap();
    assert_eq!(started.elapsed(), Duration::from_millis(100));
    assert_eq!(result.quotes.len(), 1);
    assert_eq!(result.quotes[0].route.provider, "fast");
    assert_eq!(result.failures.len(), 2);
    assert_eq!(
        result
            .failures
            .iter()
            .find(|failure| failure.provider == "slow-simulation")
            .unwrap()
            .error,
        "UPSTREAM_TIMEOUT"
    );
    assert_eq!(
        result
            .failures
            .iter()
            .find(|failure| failure.provider == "failed")
            .unwrap()
            .error,
        "UPSTREAM_HTTP_ERROR"
    );
}

#[tokio::test(start_paused = true)]
async fn slow_route_does_not_consume_fast_provider_simulation_budget() {
    let service = service(
        &[("fast", 10, false), ("slow-route", 200, false)],
        fixture_simulation(),
    );
    let result = service.create(support::input(1, NATIVE)).await.unwrap();
    assert_eq!(result.quotes.len(), 1);
    assert_eq!(result.quotes[0].route.provider, "fast");
    assert_eq!(result.failures.len(), 1);
    assert_eq!(result.failures[0].provider, "slow-route");
    assert_eq!(result.failures[0].error, "UPSTREAM_TIMEOUT");
}

struct FailedSimulation(SimulationFailure);

#[async_trait]
impl SimulationProvider for FailedSimulation {
    async fn simulate(&self, _: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
        Err(anyhow::Error::new(self.0))
    }
}

#[tokio::test]
async fn missing_router_and_rpc_never_return_executable_transactions() {
    for (router, simulator, code) in [
        (
            None,
            Arc::new(support::MockSimulation) as Arc<dyn SimulationProvider>,
            "ROUTER_NOT_CONFIGURED",
        ),
        (
            Some(Address::repeat_byte(0x22)),
            Arc::new(FailedSimulation(SimulationFailure::Unsupported(
                "RPC_NOT_CONFIGURED",
            ))) as Arc<dyn SimulationProvider>,
            "RPC_NOT_CONFIGURED",
        ),
    ] {
        let config = support::config();
        let mut chain = support::chain(&config, 1);
        chain.router = router.map(support::deployment);
        let services = Services::new(vec![Arc::new(support::MockProvider)], &[chain], simulator);
        let result = Competitions::new(config, services)
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
    let service = service(
        &[("a", 0, false)],
        Arc::new(SimulationFixture {
            calls: Arc::new(AtomicUsize::new(0)),
            below_minimum: true,
            slow_route_completed: None,
            fast_started_before_slow: None,
        }),
    );
    let result = service.create(support::input(1, NATIVE)).await.unwrap();
    assert!(result.quotes.is_empty());
    assert_eq!(result.failures[0].status, FailureStatus::Error);
}

#[tokio::test]
async fn taker_cannot_be_router_or_holder_but_routes_need_no_allowlist() {
    let config = support::config();
    let router = Address::repeat_byte(0x22);
    let service = Competitions::new(
        config.clone(),
        support::competition_services_for(&config, Some(router)),
    );
    let mut input = support::input(1, NATIVE);
    for reserved in [router, support::FIXTURE_HOLDER] {
        input.taker = reserved;
        assert_eq!(
            kind(&service.create(input.clone()).await.unwrap_err()),
            ErrorKind::InvalidTaker
        );
    }

    let result = service.create(support::input(1, NATIVE)).await.unwrap();
    assert!(result.failures.is_empty());
    assert_eq!(result.quotes.len(), 1);
}
