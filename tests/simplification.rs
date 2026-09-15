mod support;

use alloy_primitives::Address;
use alloy_provider::Provider;
use metamatch_backend::{
    competitions::Competitions,
    domain::NATIVE,
    rpc::RpcClients,
    simulation::{SimResult, SimulationFailure, SimulationProvider, SimulationRequest, Simulator},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use support::FIXTURE_TAKER;

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
async fn alloy_simulation_uses_latest_without_context_or_gas_price() {
    let server = support::FixtureRpc::default().start().await;
    let mut chain = support::chain(&support::config(), 1);
    chain.rpc_url = Some(server.url.clone());
    chain.router = Some(support::deployment(alloy_primitives::Address::repeat_byte(
        0x22,
    )));
    let input = support::input(1, NATIVE);
    let route = support::fixture_route(&input);
    let simulator = Simulator::new(
        Arc::new(RpcClients::new(Duration::from_secs(1))),
        std::sync::Arc::new(metamatch_backend::balance_slots::BalanceSlots::new(
            Default::default(),
        )),
    );
    let result = support::simulate(&simulator, &input, &chain, &route)
        .await
        .unwrap();
    assert_eq!(result.simulation.bought_amount, "100");
    assert_eq!(result.simulation.funding, "overridden");
    let requests = server.requests.lock().unwrap();
    for request in requests.iter() {
        let params = &request["params"];
        match request["method"].as_str().unwrap() {
            "eth_simulateV1" => {
                assert_eq!(params[1], "latest");
                let block = &params[0]["blockStateCalls"][0];
                assert!(block.get("blockOverrides").is_none());
                let calls = block["calls"].as_array().unwrap();
                assert!(calls.iter().all(|call| call.get("gasPrice").is_none()));
                assert_eq!(
                    block["stateOverrides"][format!("{FIXTURE_TAKER:#x}")]["balance"],
                    format!("0x{}", "ff".repeat(32))
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
    assert_eq!(requests.len(), 1);
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
        chain.router = Some(support::deployment(alloy_primitives::Address::repeat_byte(
            0x22,
        )));
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
        let error = support::simulate(&simulator, &input, &chain, &route)
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
    let config = support::config_with(serde_json::json!({"providerKeys":{"0x":"fixture-key"}}));
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
        route.tx.data = data.parse().unwrap();
        assert_eq!(kind(&validate_route(&input, route).unwrap_err()), expected);
        let client = support::client([json!({
            "liquidityAvailable": true, "sellAmount": input.sell_amount,
            "buyAmount": "100", "minBuyAmount": "99",
            "transaction": {"to": format!("{FIXTURE_TAKER:#x}"), "data": data, "value": value}
        })]);
        let error = support::provider(&config, "0x", client)
            .quote(&input, &chain, FIXTURE_TAKER)
            .await
            .unwrap_err();
        assert_eq!(kind(&error), expected);
    }
    let mut route = support::fixture_route(&input);
    route.deadline = Some(0);
    assert_eq!(
        kind(&validate_route(&input, route).unwrap_err()),
        ErrorKind::QuoteExpired
    );
}

#[async_trait::async_trait]
impl SimulationProvider for FailedSimulation {
    async fn simulate(&self, _request: SimulationRequest<'_>) -> anyhow::Result<SimResult> {
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
        let service = Competitions::new(config, services);
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
