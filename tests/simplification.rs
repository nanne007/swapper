mod support;

use alloy_primitives::Address;
use alloy_provider::Provider;
use metamatch_backend::{
    competitions::Competitions,
    domain::{NATIVE, PREVIEW_TAKER},
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
async fn alloy_simulation_keeps_fixed_block_probes_and_always_overrides_funding() {
    let server = support::FixtureRpc::default().start().await;
    let mut chain = support::chain(&support::config(), 1);
    chain.rpc_url = Some(server.url.clone());
    let input = support::input(1, NATIVE);
    let route = support::fixture_route(&input);
    let context = support::fixture_context();
    let simulator = Simulator::new(
        Arc::new(RpcClients::new(Duration::from_secs(1))),
        std::sync::Arc::new(metamatch_backend::balance_slots::BalanceSlots::new(
            Default::default(),
        )),
    );
    let result = simulator
        .run(SimulationRequest {
            input: &input,
            chain: &chain,
            route: &route,
            context: &context,
            rules: &[],
            taker: PREVIEW_TAKER,
            min: None,
        })
        .await
        .unwrap();
    assert_eq!(result.simulation.bought_amount, "100");
    assert_eq!(result.simulation.funding, "overridden");
    let requests = server.requests.lock().unwrap();
    for request in requests.iter() {
        let params = &request["params"];
        match request["method"].as_str().unwrap() {
            "eth_getBlockByNumber" => assert_eq!(params[0], "0x10"),
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
        2
    );
    assert!(!requests.iter().any(|r| r["method"] == "eth_getBalance"));
}

struct FailedSimulation(SimulationFailure);

#[tokio::test]
async fn erc20_slot_resolution_and_approval_failures_still_reject_simulation() {
    let server = support::FixtureRpc {
        false_approval: true,
        ..Default::default()
    }
    .start()
    .await;
    for (slot, expected) in [
        (
            None,
            json!({"status": "unsupported", "reason": "RPC_METHOD_UNSUPPORTED"}),
        ),
        (
            Some(0_u64),
            json!({"status": "reverted", "reason": "APPROVAL_RETURNED_FALSE"}),
        ),
    ] {
        let mut chain = support::chain(&support::config(), 1);
        chain.rpc_url = Some(server.url.clone());
        let mut input = support::input(1, Address::repeat_byte(0x55));
        input.sell_amount = "1000".into();
        let configured = slot
            .map(|slot| {
                std::collections::HashMap::from([(
                    1,
                    std::collections::HashMap::from([(
                        input.sell_token,
                        alloy_primitives::U256::from(slot),
                    )]),
                )])
            })
            .unwrap_or_default();
        let simulator = Simulator::new(
            Arc::new(RpcClients::new(Duration::from_secs(1))),
            Arc::new(metamatch_backend::balance_slots::BalanceSlots::new(
                configured,
            )),
        );
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
                min: None,
            })
            .await
            .unwrap_err();
        assert_eq!(
            serde_json::to_value(error.downcast_ref::<SimulationFailure>().unwrap()).unwrap(),
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
    route.deadline = Some(0);
    assert_eq!(
        kind(&validate_route(&input, &chain, &route, &[], false).unwrap_err()),
        ErrorKind::QuoteExpired
    );
}

#[async_trait::async_trait]
impl SimulationProvider for FailedSimulation {
    async fn run(&self, _request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
        Err(anyhow::anyhow!("original simulation cause").context(self.0))
    }
}

#[tokio::test]
async fn simulation_failures_keep_categories_and_release_capacity() {
    for (failure, status) in [
        (
            SimulationFailure::Reverted("SIMULATION_REVERTED"),
            "reverted",
        ),
        (
            SimulationFailure::Unsupported("RPC_METHOD_UNSUPPORTED"),
            "unsupported",
        ),
        (SimulationFailure::Error("RPC_CALL_FAILED"), "error"),
    ] {
        let mut config = support::config();
        config.max_active = 1;
        let mut services =
            support::competition_services_for(&config, Some(Address::repeat_byte(0x22)));
        services.simulator = Arc::new(FailedSimulation(failure));
        let service = Competitions::new(config, Some(services));
        for _ in 0..2 {
            let response = service.create(support::input(1, NATIVE)).await.unwrap();
            assert!(response.quotes.is_empty());
            let quote = &response.failures[0];
            assert_eq!(quote.error, failure.to_string());
            assert_eq!(
                serde_json::to_value(quote.simulation).unwrap()["status"],
                status
            );
            assert!(
                !serde_json::to_string(quote)
                    .unwrap()
                    .contains("original simulation cause")
            );
        }
    }
}
