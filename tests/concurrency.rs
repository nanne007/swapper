mod support;

use metamatch_backend::{
    competitions::Competitions,
    domain::{NATIVE, PREVIEW_TAKER},
    error::{ErrorKind, kind},
    simulation::{SimResult, SimulationProvider, SimulationRequest},
};
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;

#[derive(Default)]
struct PausedBuild {
    entered: Notify,
    release: Notify,
}

#[async_trait::async_trait]
impl SimulationProvider for PausedBuild {
    async fn run(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
        if request.actual {
            self.entered.notify_one();
            self.release.notified().await;
        }
        support::MockSimulation.run(request).await
    }
}

#[tokio::test]
async fn cancelling_build_releases_capacity() {
    let mut config = support::config();
    config.max_active = 1;
    let router = alloy_primitives::Address::repeat_byte(0x22);
    let mut services = support::competition_services_for(&config, Some(router));
    let simulator = Arc::new(PausedBuild::default());
    services.simulator = simulator.clone();
    let service = Competitions::new(config, Some(services));
    let created = service.create(support::input(1, NATIVE)).await.unwrap();
    let snapshot = support::wait_complete(&service, &created).await;
    let quote_id = snapshot.quotes[0].id.parse().unwrap();
    let task_service = service.clone();
    let token = created.access_token.clone();
    let id = created.id;
    let task = tokio::spawn(async move {
        task_service
            .build(id, quote_id, Some(&token), PREVIEW_TAKER, "199")
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), simulator.entered.notified())
        .await
        .unwrap();
    let error = service
        .build(
            id,
            quote_id,
            Some(&created.access_token),
            PREVIEW_TAKER,
            "199",
        )
        .await
        .unwrap_err();
    assert_eq!(kind(&error), ErrorKind::BuildCapacityExceeded);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    simulator.release.notify_one();
    let result = service
        .build(
            id,
            quote_id,
            Some(&created.access_token),
            PREVIEW_TAKER,
            "199",
        )
        .await;
    service.close().await;
    assert!(
        result.is_ok(),
        "cancelled build leaked capacity: {result:?}"
    );
}
