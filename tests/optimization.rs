use alloy_primitives::{U256, U512};
use metamatch_backend::domain::{minimum, parse_uint};
mod support;

use alloy_primitives::Address;
use async_trait::async_trait;
use metamatch_backend::{
    balance_slots::BalanceSlots,
    competitions::{Competitions, Services},
    domain::{Chain, Input, NATIVE, Route},
    providers::Provider,
    rpc::{ContextSource, RpcClients},
    simulation::Simulator,
};
use std::{collections::HashMap, sync::Arc, time::Duration};

struct Competitor(&'static str, u8);

#[async_trait]
impl Provider for Competitor {
    fn id(&self) -> &'static str {
        self.0
    }
    fn requires_access_key(&self) -> bool {
        false
    }
    fn supported_chains(&self) -> Vec<u64> {
        vec![1]
    }
    async fn quote(&self, input: &Input, _: &Chain, _: Address) -> anyhow::Result<Route> {
        let mut route = support::fixture_route(input);
        route.provider = self.0;
        route.tx.to = Address::repeat_byte(self.1);
        route.spender = Address::repeat_byte(self.1 + 1);
        route.tx.data = vec![self.1; 4].into();
        route.tx.value = if input.sell_token == NATIVE {
            input.sell_amount.clone()
        } else {
            "0".into()
        };
        Ok(route)
    }
}

fn competition(server: &support::RpcServer, configured_slot: bool) -> (Competitions, Input) {
    let config = support::config();
    let mut chain = support::chain(&config, 1);
    chain.rpc_url = Some(server.url.clone());
    chain.router = Some(support::deployment(Address::repeat_byte(0x22)));
    let input = support::input(1, Address::repeat_byte(0x55));
    let slots = if configured_slot {
        HashMap::from([(1, HashMap::from([(input.sell_token, U256::ZERO)]))])
    } else {
        HashMap::new()
    };
    let clients = Arc::new(RpcClients::new(Duration::from_secs(2)));
    let services = Services::new(
        ["a", "b", "c", "d", "e"]
            .into_iter()
            .enumerate()
            .map(|(i, id)| Arc::new(Competitor(id, 0x60 + i as u8)) as Arc<dyn Provider>)
            .collect(),
        &[chain],
        Arc::new(ContextSource::new(clients.clone())),
        Arc::new(Simulator::new(clients, Arc::new(BalanceSlots::new(slots)))),
    );
    (Competitions::new(config, services), input)
}

#[tokio::test]
async fn five_permissionless_routes_share_preparation_but_not_simulation() {
    use alloy_sol_types::SolCall;
    use metamatch_backend::execution::{execCall, executeCall};
    let server = support::FixtureRpc::default().start().await;
    let (service, input) = competition(&server, true);
    let result = service.create(input.clone()).await.unwrap();
    assert!(result.failures.is_empty());
    assert_eq!(result.quotes.len(), 5);
    let requests = server.requests.lock().unwrap();
    let count = |method| requests.iter().filter(|r| r["method"] == method).count();
    assert_eq!(
        requests.len(),
        16,
        "warm ERC20 competition: 6 shared + 2 per route"
    );
    assert_eq!(count("eth_chainId"), 1);
    assert_eq!(count("eth_gasPrice"), 1);
    assert_eq!(count("eth_getCode"), 1);
    assert_eq!(count("eth_call"), 1, "Holder allowance is shared");
    assert_eq!(
        count("eth_getBlockByNumber"),
        7,
        "context + pre-check + five independent post-checks"
    );
    assert_eq!(count("eth_simulateV1"), 5);
    let simulations: Vec<_> = requests
        .iter()
        .filter(|r| r["method"] == "eth_simulateV1")
        .collect();
    for request in &simulations {
        assert_eq!(request["params"][1], "0x10");
        let block = &request["params"][0]["blockStateCalls"][0];
        assert_eq!(
            block["stateOverrides"],
            simulations[0]["params"][0]["blockStateCalls"][0]["stateOverrides"]
        );
        let calls = block["calls"].as_array().unwrap();
        assert_eq!(calls.len(), 4);
        for call in calls {
            assert!(call["input"].is_string());
            assert!(
                call.get("data").is_none(),
                "calldata must not be sent twice"
            );
        }
    }
    for quote in &result.quotes {
        let outer = execCall::abi_decode(&quote.transaction.data).unwrap();
        let inner = executeCall::abi_decode(&outer.data).unwrap();
        assert_eq!(inner.data, quote.route.tx.data);
        assert_eq!(inner.target, quote.route.tx.to);
        assert_eq!(inner.spender, quote.route.spender);
        assert_eq!(inner.receiver, input.taker);
    }
    // Exact serialized-byte saving, not a claimed latency benchmark.
    let current = serde_json::to_vec(simulations[0]).unwrap().len();
    let mut old = simulations[0].clone();
    for call in old["params"][0]["blockStateCalls"][0]["calls"]
        .as_array_mut()
        .unwrap()
    {
        call["data"] = call["input"].clone();
    }
    let previous = serde_json::to_vec(&old).unwrap().len();
    assert!(previous > current);
    println!(
        "5 routes: 28 -> 16 logical RPC calls; fixture simulation body: {previous} -> {current} bytes"
    );
}

#[tokio::test]
async fn shared_slot_failure_is_not_retried_per_route_or_cached_across_competitions() {
    let server = support::FixtureRpc::default().start().await;
    let (service, input) = competition(&server, false);
    let mut previous = 0;
    for _ in 0..2 {
        let result = service.create(input.clone()).await.unwrap();
        assert!(result.quotes.is_empty());
        assert_eq!(result.failures.len(), 5);
        assert!(
            result
                .failures
                .iter()
                .all(|failure| failure.error == "RPC_METHOD_UNSUPPORTED")
        );
        let requests = server.requests.lock().unwrap();
        let probes = requests
            .iter()
            .filter(|r| r["method"] == "eth_createAccessList")
            .count();
        assert!(
            (1..=2).contains(&(probes - previous)),
            "one shared probe attempt per competition"
        );
        assert!(!requests.iter().any(|r| r["method"] == "eth_simulateV1"));
        previous = probes;
    }
}

#[test]
fn calldata_round_trips_as_hex_and_rejects_invalid_provider_strings() {
    use metamatch_backend::domain::{Tx, parse_hex};
    let tx = Tx {
        to: Address::repeat_byte(0x55),
        data: parse_hex("0xABCDef00").unwrap(),
        value: "0".into(),
    };
    let wire = serde_json::to_value(&tx).unwrap();
    assert_eq!(wire["data"], "0xabcdef00");
    let decoded: Tx = serde_json::from_value(wire).unwrap();
    assert_eq!(decoded.data, tx.data);
    for invalid in ["", "12345678", "0X12345678", "0x123", "0xgg"] {
        assert!(parse_hex(invalid).is_err());
    }
}

#[tokio::test]
async fn shared_preparation_preserves_native_and_all_allowance_sequences() {
    use alloy_sol_types::SolCall;
    use metamatch_backend::execution::approveCall;
    for (native, allowance, expected_approvals) in [
        (true, U256::ZERO, 0),
        (false, U256::ZERO, 1),
        (false, U256::ONE, 2),
        (false, U256::MAX, 0),
    ] {
        let server = support::FixtureRpc {
            allowance,
            ..Default::default()
        }
        .start()
        .await;
        let (service, mut input) = competition(&server, true);
        if native {
            input.sell_token = NATIVE;
        }
        let result = service.create(input.clone()).await.unwrap();
        assert!(result.failures.is_empty());
        assert_eq!(result.quotes.len(), 5);
        for quote in &result.quotes {
            assert_eq!(quote.approvals.len(), expected_approvals);
            for (i, tx) in quote.approvals.iter().enumerate() {
                let approval = approveCall::abi_decode(&tx.data).unwrap();
                assert_eq!(approval.spender, support::FIXTURE_HOLDER);
                assert_eq!(
                    approval.amount,
                    if expected_approvals == 2 && i == 0 {
                        U256::ZERO
                    } else {
                        parse_uint(&input.sell_amount).unwrap()
                    }
                );
            }
        }
        let requests = server.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "eth_call")
                .count(),
            usize::from(!native)
        );
        assert!(!requests.iter().any(|r| r["method"] == "eth_getBalance"));
        for request in requests.iter().filter(|r| r["method"] == "eth_simulateV1") {
            assert_eq!(
                request["params"][0]["blockStateCalls"][0]["calls"]
                    .as_array()
                    .unwrap()
                    .len(),
                expected_approvals + 3
            );
        }
    }
}

#[tokio::test]
async fn reorg_after_shared_preparation_still_rejects_each_simulated_route() {
    let server = support::FixtureRpc {
        reorg_after_simulation: true,
        ..Default::default()
    }
    .start()
    .await;
    let (service, input) = competition(&server, true);
    let result = service.create(input).await.unwrap();
    assert!(result.quotes.is_empty());
    assert_eq!(result.failures.len(), 5);
    assert!(
        result
            .failures
            .iter()
            .all(|failure| failure.error == "CHAIN_REORG_REQUOTE")
    );
    let requests = server.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"] == "eth_simulateV1")
            .count(),
        5
    );
}

#[test]
fn minimum_preserves_full_uint256_precision_and_floor_rounding() {
    for amount in [U256::ONE, U256::from(10001), U256::ONE << 255, U256::MAX] {
        for bps in [0, 1, 30, 500, 9999, 10000] {
            let expected = U512::from(amount) * U512::from(10000 - bps) / U512::from(10000);
            assert_eq!(
                minimum(&amount.to_string(), bps).unwrap(),
                expected.to_string()
            );
            assert!(parse_uint(&minimum(&amount.to_string(), bps).unwrap()).unwrap() <= amount);
        }
    }
}

#[test]
fn minimum_rejects_out_of_range_basis_points() {
    assert!(minimum("100", 10001).is_err());
    assert!(minimum("100", u64::MAX).is_err());
}
