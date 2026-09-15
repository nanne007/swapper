mod support;

use metamatch_backend::{
    competitions::Competitions,
    domain::NATIVE,
    error::{ErrorKind, kind},
    simulation::{SimResult, SimulationProvider, SimulationRequest},
};
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;

#[derive(Default)]
struct PausedSimulation {
    entered: Notify,
    release: Notify,
}

#[derive(Default)]
struct PausedPreparation {
    entered: Notify,
    release: Notify,
}

#[async_trait::async_trait]
impl SimulationProvider for PausedPreparation {
    async fn prepare(
        &self,
        input: &metamatch_backend::domain::Input,
        chain: &metamatch_backend::domain::Chain,
        context: &metamatch_backend::domain::Context,
    ) -> anyhow::Result<metamatch_backend::simulation::SimulationPreparation> {
        self.entered.notify_one();
        self.release.notified().await;
        support::MockSimulation.prepare(input, chain, context).await
    }
    async fn run(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
        support::MockSimulation.run(request).await
    }
}

#[tokio::test]
async fn cancelling_shared_preparation_releases_capacity_and_allows_retry() {
    let mut config = support::config();
    config.max_active = 1;
    let mut services = support::competition_services_for(
        &config,
        Some(alloy_primitives::Address::repeat_byte(0x22)),
    );
    let preparation = Arc::new(PausedPreparation::default());
    services.simulator = preparation.clone();
    let service = Arc::new(Competitions::new(config, services));
    let task_service = service.clone();
    let task = tokio::spawn(async move { task_service.create(support::input(1, NATIVE)).await });
    tokio::time::timeout(Duration::from_secs(1), preparation.entered.notified())
        .await
        .unwrap();
    assert_eq!(
        kind(&service.create(support::input(1, NATIVE)).await.unwrap_err()),
        ErrorKind::CapacityExceeded
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    preparation.release.notify_one();
    let result = service.create(support::input(1, NATIVE)).await.unwrap();
    assert_eq!(result.quotes.len(), 1);
    assert!(result.failures.is_empty());
}

#[async_trait::async_trait]
impl SimulationProvider for PausedSimulation {
    async fn prepare(
        &self,
        input: &metamatch_backend::domain::Input,
        chain: &metamatch_backend::domain::Chain,
        context: &metamatch_backend::domain::Context,
    ) -> anyhow::Result<metamatch_backend::simulation::SimulationPreparation> {
        support::MockSimulation.prepare(input, chain, context).await
    }
    async fn run(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
        self.entered.notify_one();
        self.release.notified().await;
        support::MockSimulation.run(request).await
    }
}

#[tokio::test]
async fn cancelling_competition_releases_capacity() {
    let mut config = support::config();
    config.max_active = 1;
    let router = alloy_primitives::Address::repeat_byte(0x22);
    let mut services = support::competition_services_for(&config, Some(router));
    let simulator = Arc::new(PausedSimulation::default());
    services.simulator = simulator.clone();
    let service = Competitions::new(config, services);
    let task_service = service.clone();
    let task = tokio::spawn(async move { task_service.create(support::input(1, NATIVE)).await });
    tokio::time::timeout(Duration::from_secs(1), simulator.entered.notified())
        .await
        .unwrap();
    let error = service.create(support::input(1, NATIVE)).await.unwrap_err();
    assert_eq!(kind(&error), ErrorKind::CapacityExceeded);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    simulator.release.notify_one();
    let result = service.create(support::input(1, NATIVE)).await.unwrap();
    assert_eq!(
        result.quotes.len(),
        1,
        "cancelled competition leaked capacity"
    );
    assert!(result.failures.is_empty());
}
