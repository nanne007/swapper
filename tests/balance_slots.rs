mod support;

use alloy_primitives::{Address, B256, U256, keccak256};
use metamatch_backend::{
    balance_slots::{BalanceSlotError, BalanceSlots},
    config::load_config,
    error::{ErrorKind, kind},
    rpc::RpcClients,
    simulation::Simulator,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

const TOKEN: Address = Address::repeat_byte(0x55);
const OWNER: Address = Address::repeat_byte(0x66);

fn key(owner: Address, base: u64) -> B256 {
    let mut words = [0u8; 64];
    words[12..32].copy_from_slice(owner.as_slice());
    words[32..].copy_from_slice(&U256::from(base).to_be_bytes::<32>());
    keccak256(words)
}

fn word(value: U256) -> Value {
    json!(B256::from(value.to_be_bytes::<32>()))
}

#[derive(Default)]
struct Fixture {
    base: u64,
    shared: bool,
    ambiguous: bool,
    access_list_error: Option<i64>,
    access_list_result_error: bool,
    call_error: Option<i64>,
    malformed: bool,
    excess: bool,
    pause: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    access_list_barrier: Option<Arc<tokio::sync::Barrier>>,
}

struct Server {
    url: String,
    fixture: Arc<Mutex<Fixture>>,
    requests: Arc<Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    async fn start(fixture: Fixture) -> Self {
        let fixture = Arc::new(Mutex::new(fixture));
        let state = fixture.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let router = axum::Router::new().route(
            "/",
            axum::routing::post(move |axum::Json(req): axum::Json<Value>| {
                recorded.lock().unwrap().push(req.clone());
                let fixture = state.clone();
                async move {
                    let barrier = fixture.lock().unwrap().access_list_barrier.clone();
                    if req["method"] == "eth_createAccessList"
                        && let Some(barrier) = barrier
                    {
                        barrier.wait().await;
                    }
                    let pause = fixture.lock().unwrap().pause.clone();
                    if req["method"] == "eth_createAccessList"
                        && let Some((entered, resume)) = pause
                    {
                        entered.notify_one();
                        resume.notified().await;
                    }
                    let fixture = fixture.lock().unwrap();
                    let mut result = json!({"jsonrpc":"2.0", "id":req["id"]});
                    let error_code = match req["method"].as_str().unwrap() {
                        "eth_createAccessList" => fixture.access_list_error,
                        "eth_call" => fixture.call_error,
                        _ => None,
                    };
                    if let Some(code) = error_code {
                        result["error"] =
                            json!({"code":code,"message":"fixture private upstream cause"});
                    } else {
                        result["result"] = fixture.respond(&req);
                    }
                    axum::Json(result)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            url,
            fixture,
            requests,
            task,
        }
    }

    fn access_lists(&self) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["method"] == "eth_createAccessList")
            .count()
    }

    async fn resolve(
        &self,
        slots: &BalanceSlots,
        chain: u64,
        token: Address,
    ) -> anyhow::Result<U256> {
        let rpc = RpcClients::new(Duration::from_secs(2)).get(&self.url)?;
        slots.resolve(&rpc, chain, token, 16.into()).await
    }
}

impl Fixture {
    fn respond(&self, req: &Value) -> Value {
        match req["method"].as_str().unwrap() {
            "eth_createAccessList" => {
                let input = req["params"][0]["input"]
                    .as_str()
                    .or_else(|| req["params"][0]["data"].as_str())
                    .unwrap();
                let owner: Address = format!("0x{}", &input[input.len() - 40..]).parse().unwrap();
                let token: Address = req["params"][0]["to"].as_str().unwrap().parse().unwrap();
                let slot = key(if self.shared { OWNER } else { owner }, self.base);
                if self.malformed {
                    return json!({"accessList":[{"address":format!("{token:#x}"),"storageKeys":["invalid"]}],"gasUsed":"0x1"});
                }
                if self.access_list_result_error {
                    return json!({"accessList":[],"gasUsed":"0x1","error":"fixture execution error"});
                }
                let mut storage_keys = vec![slot];
                storage_keys.push(if self.ambiguous {
                    key(owner, self.base + 1)
                } else {
                    B256::repeat_byte(0x99)
                });
                if self.excess {
                    storage_keys
                        .extend((0..33).map(|i| B256::from(U256::from(i).to_be_bytes::<32>())));
                }
                json!({"accessList":[{"address":token,"storageKeys":storage_keys}],"gasUsed":"0x1"})
            }
            "eth_call" => word(U256::ZERO),
            _ => support::FixtureRpc::default().response(req)["result"].clone(),
        }
    }
}

#[test]
fn config_parses_full_width_base_slots_and_rejects_invalid_values() {
    let value = json!({"1":{format!("{TOKEN:#x}"):format!("{:#x}",U256::MAX)},"8453":{format!("{TOKEN:#x}"):"0x0"}}).to_string();
    let config = load_config(&format!(r#"{{"balanceSlots":{value}}}"#)).unwrap();
    assert_eq!(config.balance_slots[&1][&TOKEN], U256::MAX);
    assert_eq!(config.balance_slots[&8453][&TOKEN], U256::ZERO);
    for value in [
        "[]".into(),
        "null".into(),
        json!({"0":{format!("{TOKEN:#x}"):"0x0"}}).to_string(),
        json!({"1":{format!("{:#x}", Address::ZERO):"0x0"}}).to_string(),
        json!({"1":{format!("{TOKEN:#x}"):"-1"}}).to_string(),
        json!({"1":{"invalid":"0x0"}}).to_string(),
    ] {
        let error = load_config(&format!(r#"{{"balanceSlots":{value}}}"#))
            .err()
            .unwrap();
        assert_eq!(kind(&error), ErrorKind::InvalidConfig);
    }
}

#[tokio::test]
async fn configured_base_is_returned_without_rpc_or_funding_validation() {
    let server = Server::start(Fixture {
        access_list_error: Some(-32601),
        call_error: Some(-32000),
        ..Default::default()
    })
    .await;
    for base in [U256::from(7), U256::MAX] {
        let slots = BalanceSlots::new(HashMap::from([(1, HashMap::from([(TOKEN, base)]))]));
        assert_eq!(server.resolve(&slots, 1, TOKEN).await.unwrap(), base);
        assert_eq!(server.resolve(&slots, 1, TOKEN).await.unwrap(), base);
    }
    assert!(server.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn standalone_detection_uses_fake_owners_without_overrides_and_caches_base() {
    let server = Server::start(Fixture {
        base: 900,
        ..Default::default()
    })
    .await;
    let slots = BalanceSlots::new(Default::default());
    assert_eq!(
        server.resolve(&slots, 1, TOKEN).await.unwrap(),
        U256::from(900)
    );
    assert_eq!(
        server.resolve(&slots, 1, TOKEN).await.unwrap(),
        U256::from(900)
    );
    assert_eq!(server.access_lists(), 2);
    for (chain, token) in [(8453, TOKEN), (1, Address::repeat_byte(0x77))] {
        assert_eq!(
            server.resolve(&slots, chain, token).await.unwrap(),
            U256::from(900)
        );
    }
    assert_eq!(server.access_lists(), 6);
    let requests = server.requests.lock().unwrap();
    let mut probes = std::collections::HashSet::new();
    for req in requests.iter() {
        assert_eq!(req["method"], "eth_createAccessList");
        assert_eq!(req["params"][1], "0x10");
        assert_eq!(req["params"].as_array().unwrap().len(), 2);
        assert!(req["params"][0].get("stateOverrides").is_none());
        let input = req["params"][0]["input"]
            .as_str()
            .or_else(|| req["params"][0]["data"].as_str())
            .unwrap();
        let owner: Address = format!("0x{}", &input[input.len() - 40..]).parse().unwrap();
        assert_ne!(owner, OWNER);
        assert_ne!(owner, Address::ZERO);
        probes.insert(owner);
    }
    assert_eq!(probes.len(), 2);
}

#[tokio::test]
async fn concurrent_resolvers_share_one_detection() {
    let server = Server::start(Fixture::default()).await;
    let slots = BalanceSlots::new(Default::default());
    let results =
        futures::future::join_all((0..13).map(|_| server.resolve(&slots, 1, TOKEN))).await;
    assert!(
        results
            .into_iter()
            .all(|result| result.unwrap() == U256::ZERO)
    );
    assert_eq!(server.access_lists(), 2);
}

#[tokio::test]
async fn both_access_lists_start_before_either_response_is_available() {
    let server = Server::start(Fixture {
        access_list_barrier: Some(Arc::new(tokio::sync::Barrier::new(2))),
        ..Default::default()
    })
    .await;
    let slots = BalanceSlots::new(Default::default());
    let base = tokio::time::timeout(Duration::from_secs(2), server.resolve(&slots, 1, TOKEN))
        .await
        .expect("both access-list calls must be in flight together")
        .unwrap();
    assert_eq!(base, U256::ZERO);
    assert_eq!(server.access_lists(), 2);
}

#[tokio::test]
async fn detection_range_is_bounded_and_larger_bases_can_be_configured() {
    let server = Server::start(Fixture {
        base: 1023,
        ..Default::default()
    })
    .await;
    let slots = BalanceSlots::new(Default::default());
    assert_eq!(
        server.resolve(&slots, 1, TOKEN).await.unwrap(),
        U256::from(1023)
    );
    server.fixture.lock().unwrap().base = 1024;
    let fresh_slots = BalanceSlots::new(Default::default());
    assert!(matches!(
        server
            .resolve(&fresh_slots, 1, TOKEN)
            .await
            .unwrap_err()
            .downcast_ref(),
        Some(BalanceSlotError::NotFound)
    ));
    let configured = BalanceSlots::new(HashMap::from([(
        1,
        HashMap::from([(TOKEN, U256::from(1024))]),
    )]));
    assert_eq!(
        server.resolve(&configured, 1, TOKEN).await.unwrap(),
        U256::from(1024)
    );
    assert_eq!(server.access_lists(), 4);
}

#[tokio::test]
async fn detection_rejects_fixed_storage_and_ambiguous_mappings_without_caching_failure() {
    for ambiguous in [false, true] {
        let server = Server::start(Fixture {
            shared: !ambiguous,
            ambiguous,
            ..Default::default()
        })
        .await;
        let slots = BalanceSlots::new(Default::default());
        for _ in 0..2 {
            let error = server.resolve(&slots, 1, TOKEN).await.unwrap_err();
            if ambiguous {
                assert!(matches!(
                    error.downcast_ref(),
                    Some(BalanceSlotError::Ambiguous)
                ));
            } else {
                assert!(matches!(
                    error.downcast_ref(),
                    Some(BalanceSlotError::NotFound)
                ));
            }
        }
        assert_eq!(server.access_lists(), 4);
    }
}

#[tokio::test]
async fn access_list_errors_remain_diagnostic_and_retryable() {
    for (fixture, expected) in [
        (
            Fixture {
                access_list_error: Some(-32601),
                ..Default::default()
            },
            ErrorKind::RpcMethodUnsupported,
        ),
        (
            Fixture {
                access_list_error: Some(-32000),
                ..Default::default()
            },
            ErrorKind::RpcCallFailed,
        ),
        (
            Fixture {
                malformed: true,
                ..Default::default()
            },
            ErrorKind::RpcInvalidResponse,
        ),
        (
            Fixture {
                access_list_result_error: true,
                ..Default::default()
            },
            ErrorKind::RpcCallFailed,
        ),
    ] {
        let server = Server::start(fixture).await;
        let slots = BalanceSlots::new(Default::default());
        assert_eq!(
            kind(&server.resolve(&slots, 1, TOKEN).await.unwrap_err()),
            expected
        );
        *server.fixture.lock().unwrap() = Fixture::default();
        server.resolve(&slots, 1, TOKEN).await.unwrap();
        assert!((3..=4).contains(&server.access_lists()));
    }
}

#[tokio::test]
async fn detection_limits_access_list_storage_before_continuing() {
    let server = Server::start(Fixture {
        excess: true,
        ..Default::default()
    })
    .await;
    let error = server
        .resolve(&BalanceSlots::new(Default::default()), 1, TOKEN)
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(BalanceSlotError::ProbeLimit)
    ));
    assert!((1..=2).contains(&server.requests.lock().unwrap().len()));
}

async fn simulate(
    server: &Server,
    slots: Arc<BalanceSlots>,
    owner: Address,
) -> anyhow::Result<metamatch_backend::simulation::SimResult> {
    let simulator = Simulator::new(Arc::new(RpcClients::new(Duration::from_secs(2))), slots);
    let mut chain = support::chain(&support::config(), 1);
    chain.rpc_url = Some(server.url.clone());
    chain.router = Some(support::deployment(alloy_primitives::Address::repeat_byte(
        0x22,
    )));
    let mut input = support::input(1, TOKEN);
    input.sell_amount = "100".into();
    input.taker = owner;
    let mut route = support::fixture_route(&input);
    route.tx.value = "0".into();
    support::simulate(&simulator, &input, &chain, &route).await
}

#[tokio::test]
async fn simulation_uses_pre_resolved_base_for_each_real_owner() {
    let server = Server::start(Fixture {
        base: 7,
        ..Default::default()
    })
    .await;
    let slots = Arc::new(BalanceSlots::new(Default::default()));
    assert_eq!(
        server.resolve(&slots, 1, TOKEN).await.unwrap(),
        U256::from(7)
    );
    for owner in [OWNER, Address::repeat_byte(0x77)] {
        assert_eq!(
            simulate(&server, slots.clone(), owner)
                .await
                .unwrap()
                .simulation
                .bought_amount,
            "100"
        );
    }
    assert_eq!(server.access_lists(), 2);
    let requests = server.requests.lock().unwrap();
    let simulations: Vec<_> = requests
        .iter()
        .filter(|req| req["method"] == "eth_simulateV1")
        .collect();
    for (req, owner) in simulations.iter().zip([OWNER, Address::repeat_byte(0x77)]) {
        let overrides = &req["params"][0]["blockStateCalls"][0]["stateOverrides"];
        assert_eq!(
            overrides[format!("{TOKEN:#x}")]["stateDiff"][format!("{:#x}", key(owner, 7))],
            word(U256::from(100))
        );
        assert!(overrides[format!("{owner:#x}")]["balance"].is_string());
    }
    assert_eq!(simulations.len(), 2);
    assert!(
        !requests
            .iter()
            .any(|request| request["method"] == "eth_getBalance")
    );
}

#[tokio::test]
async fn cancellation_releases_detection_for_the_next_request() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let server = Server::start(Fixture {
        pause: Some((entered.clone(), resume.clone())),
        ..Default::default()
    })
    .await;
    let slots = Arc::new(BalanceSlots::new(Default::default()));
    let shared = slots.clone();
    let rpc = RpcClients::new(Duration::from_secs(2))
        .get(&server.url)
        .unwrap();
    let task = tokio::spawn(async move { shared.resolve(&rpc, 1, TOKEN, 16.into()).await });
    entered.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    server.fixture.lock().unwrap().pause = None;
    resume.notify_one();
    tokio::time::timeout(Duration::from_secs(2), server.resolve(&slots, 1, TOKEN))
        .await
        .unwrap()
        .unwrap();
    assert!((3..=4).contains(&server.access_lists()));
}

#[tokio::test]
async fn bounded_cache_evicts_an_idle_entry() {
    let server = Server::start(Fixture::default()).await;
    let slots = BalanceSlots::new(Default::default());
    let rpc = RpcClients::new(Duration::from_secs(2))
        .get(&server.url)
        .unwrap();
    for chain in 1..=1025 {
        slots.resolve(&rpc, chain, TOKEN, 16.into()).await.unwrap();
    }
    assert_eq!(server.access_lists(), 2050);
    slots.resolve(&rpc, 1025, TOKEN, 16.into()).await.unwrap();
    assert_eq!(server.access_lists(), 2050);
    // The newly inserted item is cached; at least one of the earlier 1024 was evicted.
    for chain in 1..=1024 {
        slots.resolve(&rpc, chain, TOKEN, 16.into()).await.unwrap();
        if server.access_lists() > 2050 {
            break;
        }
    }
    assert_eq!(server.access_lists(), 2052);
}
