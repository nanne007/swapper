mod support;

use alloy_primitives::Address;
use alloy_provider::Provider;
use metamatch_backend::{
    api_error::ApiError,
    competitions::Competitions,
    domain::{NATIVE, PREVIEW_TAKER, Simulation},
    rpc::RpcClients,
    simulation::{SimResult, SimulationFailure, SimulationProvider, SimulationRequest, Simulator},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[test]
fn rpc_clients_reuse_the_alloy_provider_for_each_endpoint() {
    let clients = RpcClients::new(Duration::from_secs(1));
    let first = clients.get("http://127.0.0.1:1").unwrap();
    let same = clients.get("http://127.0.0.1:1").unwrap();
    let other = clients.get("http://127.0.0.1:2").unwrap();
    assert!(std::ptr::eq(first.root(), same.root()));
    assert!(!std::ptr::eq(first.root(), other.root()));
    assert!(clients.get("not a URL").is_err());
}

#[tokio::test]
async fn alloy_simulation_keeps_fixed_block_probes_and_funding_boundaries() {
    let server = support::FixtureRpc::default().start().await;
    let mut chain = support::chain(&support::config(), 1);
    chain.rpc_url = Some(server.url.clone());
    let input = support::input(1, NATIVE);
    let route = support::fixture_route(&input);
    let context = support::fixture_context();
    let simulator = Simulator::new(Arc::new(RpcClients::new(Duration::from_secs(1))));
    for actual in [false, true] {
        let result = simulator
            .run(SimulationRequest {
                input: &input,
                chain: &chain,
                route: &route,
                context: &context,
                rules: &[],
                taker: PREVIEW_TAKER,
                actual,
                min: None,
            })
            .await;
        if actual {
            let failure = result.unwrap_err();
            assert_eq!(
                serde_json::to_value(Simulation::from(
                    *failure.downcast_ref::<SimulationFailure>().unwrap()
                ))
                .unwrap(),
                json!({"status": "reverted", "reason": "INSUFFICIENT_NATIVE_BALANCE_FOR_GAS"})
            );
        } else {
            assert!(result.unwrap().simulation.is_success());
        }
    }
    let requests = server.requests.lock().unwrap();
    for request in requests.iter() {
        let params = &request["params"];
        match request["method"].as_str().unwrap() {
            "eth_getBlockByNumber" => assert_eq!(params[0], "0x10"),
            "eth_getBalance" => assert_eq!(params[1], "0x10"),
            "eth_simulateV1" => {
                assert_eq!(params[1], "0x10");
                let block = &params[0]["blockStateCalls"][0];
                let calls = block["calls"].as_array().unwrap();
                assert_eq!(calls.first().unwrap()["gasPrice"], "0x0");
                assert_eq!(calls.last().unwrap()["gasPrice"], "0x0");
                assert!(
                    block["stateOverrides"][format!("{PREVIEW_TAKER:#x}")]["balance"].is_string()
                );
            }
            method => panic!("unexpected RPC method: {method}"),
        }
    }
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"] == "eth_simulateV1")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"] == "eth_getBlockByNumber")
            .count(),
        3
    );
}

struct FailedBuild(SimulationFailure);

#[tokio::test]
async fn erc20_funding_and_approval_failures_still_reject_simulation() {
    let server = support::FixtureRpc {
        false_approval: true,
        ..Default::default()
    }
    .start()
    .await;
    let simulator = Simulator::new(Arc::new(RpcClients::new(Duration::from_secs(1))));
    for (amount, actual, slot, expected) in [
        (
            "1000",
            false,
            None,
            json!({"status": "unsupported", "reason": "BALANCE_OVERRIDE_SLOT_UNCONFIGURED"}),
        ),
        (
            "1000",
            false,
            Some(0),
            json!({"status": "unsupported", "reason": "BALANCE_OVERRIDE_VALIDATION_FAILED"}),
        ),
        (
            "1000",
            true,
            None,
            json!({"status": "reverted", "reason": "INSUFFICIENT_SELL_BALANCE"}),
        ),
        (
            "100",
            false,
            None,
            json!({"status": "reverted", "reason": "APPROVAL_RETURNED_FALSE"}),
        ),
    ] {
        let mut chain = support::chain(&support::config(), 1);
        chain.rpc_url = Some(server.url.clone());
        let mut input = support::input(1, Address::repeat_byte(0x55));
        input.sell_amount = amount.into();
        if let Some(slot) = slot {
            chain
                .balance_slots
                .insert(format!("{:#x}", input.sell_token), slot);
        }
        let mut route = support::fixture_route(&input);
        route.tx.value = "0".into();
        let error = simulator
            .run(SimulationRequest {
                input: &input,
                chain: &chain,
                route: &route,
                context: &support::fixture_context(),
                rules: &[],
                taker: PREVIEW_TAKER,
                actual,
                min: None,
            })
            .await
            .unwrap_err();
        assert_eq!(
            serde_json::to_value(Simulation::from(
                *error.downcast_ref::<SimulationFailure>().unwrap()
            ))
            .unwrap(),
            expected
        );
    }
}

#[tokio::test]
async fn shared_transaction_validation_keeps_provider_and_execution_guards() {
    use metamatch_backend::{
        error::{ErrorKind, kind},
        execution::validate_route,
    };
    let config = support::config_with(&[("ZERO_EX_API_KEY", "fixture-key")]);
    let input = support::input(1, NATIVE);
    let chain = support::chain(&config, 1);
    for (value, data, expected) in [
        ("0", "0x12345678", ErrorKind::UnexpectedTransactionValue),
        (
            input.sell_amount.as_str(),
            "0x1234",
            ErrorKind::InvalidCalldata,
        ),
    ] {
        let mut route = support::fixture_route(&input);
        route.tx.value = value.into();
        route.tx.data = data.into();
        assert_eq!(
            kind(&validate_route(&input, &chain, &route, &[], false).unwrap_err()),
            expected
        );
        let client = support::client([json!({
            "liquidityAvailable": true, "sellAmount": input.sell_amount,
            "buyAmount": "100", "minBuyAmount": "99",
            "transaction": {"to": format!("{PREVIEW_TAKER:#x}"), "data": data, "value": value}
        })]);
        let error = support::provider(&config, "0x", client)
            .quote(&input, &chain, PREVIEW_TAKER)
            .await
            .unwrap_err();
        assert_eq!(kind(&error), expected);
    }
    let mut route = support::fixture_route(&input);
    route.expires_at = 0;
    assert_eq!(
        kind(&validate_route(&input, &chain, &route, &[], false).unwrap_err()),
        ErrorKind::QuoteExpired
    );
}

#[async_trait::async_trait]
impl SimulationProvider for FailedBuild {
    async fn run(&self, request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
        if request.actual {
            Err(anyhow::anyhow!("original simulation cause").context(self.0))
        } else {
            support::MockSimulation.run(request).await
        }
    }
}

#[tokio::test]
async fn build_failures_keep_public_categories_and_sources_and_release_capacity() {
    for (failure, expected) in [
        (
            SimulationFailure::Reverted("SIMULATION_REVERTED"),
            "BUILD_REVERTED",
        ),
        (
            SimulationFailure::Unsupported("RPC_METHOD_UNSUPPORTED"),
            "BUILD_UNSUPPORTED",
        ),
        (SimulationFailure::Error("RPC_CALL_FAILED"), "BUILD_ERROR"),
    ] {
        let mut config = support::config();
        config.max_active = 1;
        let mut services =
            support::competition_services_for(&config, Some(Address::repeat_byte(0x22)));
        services.simulator = Arc::new(FailedBuild(failure));
        let service = Competitions::new(config, Some(services));
        let created = service.create(support::input(1, NATIVE)).await.unwrap();
        let snapshot = support::wait_complete(&service, &created).await;
        for _ in 0..2 {
            let error = service
                .build(
                    created.id,
                    snapshot.quotes[0].id.parse().unwrap(),
                    Some(&created.access_token),
                    PREVIEW_TAKER,
                    "199",
                )
                .await
                .unwrap_err();
            assert_eq!(ApiError::from(&error).code, expected);
            assert_eq!(error.root_cause().to_string(), "original simulation cause");
            assert!(error.downcast_ref::<SimulationFailure>().is_some());
        }
        service.close().await;
    }
}

#[tokio::test]
async fn response_wire_names_and_optional_fields_are_unchanged() {
    let config = support::config();
    let services = support::competition_services_for(&config, Some(Address::repeat_byte(0x22)));
    let service = Competitions::new(config, Some(services));
    let input = support::input(1, NATIVE);
    let created = service.create(input.clone()).await.unwrap();
    let snapshot = support::wait_complete(&service, &created).await;
    let build = service
        .build(
            created.id,
            snapshot.quotes[0].id.parse().unwrap(),
            Some(&created.access_token),
            PREVIEW_TAKER,
            "199",
        )
        .await
        .unwrap();
    fn assert_keys(value: serde_json::Value, expected: &[&str]) {
        let actual: std::collections::BTreeSet<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(actual, expected.iter().copied().collect());
    }
    assert_keys(
        serde_json::to_value(&created).unwrap(),
        &["id", "accessToken", "expiresAt"],
    );
    assert_keys(
        serde_json::to_value(&snapshot).unwrap(),
        &[
            "id",
            "input",
            "status",
            "expiresAt",
            "context",
            "quotes",
            "recommendedQuoteId",
        ],
    );
    assert_keys(
        serde_json::to_value(&snapshot.quotes[0]).unwrap(),
        &[
            "id",
            "provider",
            "status",
            "quotedAmount",
            "minBuyAmount",
            "simulation",
            "latencyMs",
            "expiresAt",
            "execution",
        ],
    );
    assert_keys(
        serde_json::to_value(&build).unwrap(),
        &[
            "chainId",
            "taker",
            "recipient",
            "provider",
            "expiresAt",
            "minBuyAmount",
            "approvals",
            "transaction",
            "simulation",
            "context",
            "warning",
        ],
    );
    assert_keys(
        serde_json::to_value(&build.simulation).unwrap(),
        &[
            "status",
            "boughtAmount",
            "gasUsed",
            "gasFeeWei",
            "funding",
            "blockHash",
        ],
    );
    assert_eq!(
        serde_json::to_value(input).unwrap()["taker"],
        serde_json::Value::Null
    );
    assert_eq!(
        serde_json::to_value(build.simulation).unwrap()["status"],
        "success"
    );
    service.close().await;
}
