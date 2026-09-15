use metamatch_backend::error::ErrorKind;
mod support;

use axum::http::StatusCode;
use metamatch_backend::{
    app::create_app,
    chains::{alchemy_rpc_url, configured_chain, configured_chains},
    competitions::{Competitions, Services},
    config::load_config,
    domain::{Input, NATIVE, Route, Tx, minimum, parse_address, parse_positive, parse_uint},
    execution::{swap_transaction, validate_route},
    http::{HttpClient, HttpRequest, HttpResponse, json_request_as, url_with_params},
    rpc::{ContextProvider, ContextSource, RpcClients},
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use support::{
    FIXTURE_TAKER, FixtureRpc, chain, config, fixture_context, input, request, run_simulation,
};

fn parse_input(value: Value) -> anyhow::Result<Input> {
    let input: Input = serde_json::from_value(value)?;
    input.validate()?;
    Ok(input)
}

#[test]
fn integer_math_never_uses_floating_point() {
    assert_eq!(
        minimum("900719925474099312345", 30).unwrap(),
        "898017765697677014407"
    );
    assert_eq!(minimum("100", 30).unwrap(), "99");
    for invalid in ["0", "1.1", "1e18", "-1", "00"] {
        assert!(
            parse_positive(invalid).is_err(),
            "{invalid} must be rejected"
        );
    }
    assert!(
        parse_uint(
            "115792089237316195423570985008687907853269984665640564039457584007913129639936"
        )
        .is_err()
    );
}

#[test]
fn input_rejects_unknown_fields_and_reserved_takers() {
    let base = json!({
        "chainId": 1,
        "sellToken": format!("{NATIVE:#x}"),
        "buyToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
        "sellAmount": "1",
        "taker": format!("{FIXTURE_TAKER:#x}")
    });
    assert!(parse_input(base.clone()).is_ok());
    assert!(parse_input(json!({"x": 1})).is_err());
    assert!(
        parse_input(json!({
            "chainId": 1,
            "sellToken": format!("{NATIVE:#x}"),
            "buyToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            "sellAmount": "1",
            "taker": "0x0000000000000000000000000000000000000001"
        }))
        .is_err()
    );
}

#[test]
fn input_wire_contract_preserves_defaults_ranges_and_error_kinds() {
    let base = json!({
        "chainId": u64::MAX,
        "sellToken": format!("{NATIVE:#x}"),
        "buyToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
        "sellAmount": "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        "taker": format!("{FIXTURE_TAKER:#x}")
    });
    let parsed = parse_input(base.clone()).unwrap();
    assert_eq!(parsed.chain_id, u64::MAX);
    assert_eq!(parsed.slippage_bps, 30);
    let mut expected = base.clone();
    expected["slippageBps"] = json!(30);
    assert_eq!(serde_json::to_value(&parsed).unwrap(), expected);
    assert_eq!(serde_json::from_value::<Input>(expected).unwrap(), parsed);

    for (field, value) in [
        ("chainId", json!(0)),
        ("chainId", json!("1")),
        ("chainId", json!(-1)),
        ("slippageBps", json!(0)),
        ("slippageBps", json!(501)),
        ("slippageBps", Value::Null),
        ("sellAmount", json!(0)),
        ("sellAmount", json!("0")),
        ("sellAmount", json!("01")),
        ("sellAmount", json!("1e18")),
        ("extra", json!(true)),
    ] {
        let mut invalid = base.clone();
        invalid[field] = value;
        assert!(
            serde_json::from_value::<Input>(invalid).is_err(),
            "must reject {field}"
        );
    }
    for (field, value, kind) in [
        ("buyToken", format!("{NATIVE:#x}"), ErrorKind::InvalidInput),
        ("taker", format!("{NATIVE:#x}"), ErrorKind::InvalidTaker),
    ] {
        let mut invalid = base.clone();
        invalid[field] = json!(value);
        let error = parse_input(invalid).unwrap_err();
        assert_eq!(error.downcast_ref::<ErrorKind>(), Some(&kind));
    }
}

#[test]
fn chains_are_built_from_provider_chain_ids() {
    let config = config();
    let chains = configured_chains(&config, [143, 1]);
    assert_eq!(
        chains.iter().map(|chain| chain.id).collect::<Vec<_>>(),
        [143, 1]
    );
    assert_eq!(chains[0].name, "monad");
    assert!(chains.iter().all(|chain| chain.router.is_none()));
    assert!(chains.iter().all(|chain| chain.rpc_url.is_none()));
}

#[test]
fn rpc_is_selected_by_chain_id() {
    let config = support::config_with(json!({"chains":{"8453":{"rpcUrl":"http://base"}}}));
    assert_eq!(
        configured_chain(&config, 8453).rpc_url.as_deref(),
        Some("http://base")
    );
}

#[test]
fn alchemy_base_urls_are_keyless_and_fill_provider_chains() {
    let config = support::config_with(json!({"alchemyApiKey":"fixture/key"}));
    let expected = HashMap::from([
        (1, "https://eth-mainnet.g.alchemy.com/v2/"),
        (10, "https://opt-mainnet.g.alchemy.com/v2/"),
        (56, "https://bnb-mainnet.g.alchemy.com/v2/"),
        (130, "https://unichain-mainnet.g.alchemy.com/v2/"),
        (137, "https://polygon-mainnet.g.alchemy.com/v2/"),
        (143, "https://monad-mainnet.g.alchemy.com/v2/"),
        (146, "https://sonic-mainnet.g.alchemy.com/v2/"),
        (999, "https://hyperliquid-mainnet.g.alchemy.com/v2/"),
        (5000, "https://mantle-mainnet.g.alchemy.com/v2/"),
        (8453, "https://base-mainnet.g.alchemy.com/v2/"),
        (9745, "https://plasma-mainnet.g.alchemy.com/v2/"),
        (42161, "https://arb-mainnet.g.alchemy.com/v2/"),
        (43114, "https://avax-mainnet.g.alchemy.com/v2/"),
        (59144, "https://linea-mainnet.g.alchemy.com/v2/"),
        (80094, "https://berachain-mainnet.g.alchemy.com/v2/"),
        (81457, "https://blast-mainnet.g.alchemy.com/v2/"),
        (534352, "https://scroll-mainnet.g.alchemy.com/v2/"),
    ]);
    let chains = configured_chains(&config, expected.keys().copied());

    for chain in &chains {
        let base_url = expected.get(&chain.id).unwrap();
        assert_eq!(alchemy_rpc_url(chain.id), Some(*base_url));
        let full_url = format!("{base_url}fixture%2Fkey");
        assert_eq!(chain.rpc_url.as_deref(), Some(full_url.as_str()));
    }
    assert_eq!(alchemy_rpc_url(999_999), None);
}

#[test]
fn explicit_rpc_url_wins_over_alchemy() {
    let config = support::config_with(
        json!({"alchemyApiKey":"fixture-key","chains":{"8453":{"rpcUrl":"http://base"},"1":{"rpcUrl":"http://ethereum"}}}),
    );
    let chains = configured_chains(&config, [8453, 1]);

    assert_eq!(
        chains
            .iter()
            .find(|chain| chain.id == 8453)
            .unwrap()
            .rpc_url
            .as_deref(),
        Some("http://base")
    );
    assert_eq!(
        chains
            .iter()
            .find(|chain| chain.id == 1)
            .unwrap()
            .rpc_url
            .as_deref(),
        Some("http://ethereum")
    );

    assert!(load_config(r#"{"ETHEREUM_RPC_URL":"http://legacy-ethereum"}"#).is_err());
}

#[test]
fn defaults_do_not_contain_chain_or_provider_selection() {
    let config = config();
    assert_eq!(config.port, 3000);
    assert!(config.provider_keys.is_empty());
}

#[test]
fn rejects_bad_rpc_and_reads_provider_keys() {
    let mut document =
        json!({"chains":{"8453":{"rpcUrl":"file:///tmp/rpc"}},"providerKeys":{"odos":"test-key"}});
    assert!(load_config(&document.to_string()).is_err());
    document["chains"]["8453"]["rpcUrl"] = json!("http://base");
    let config = load_config(&document.to_string()).unwrap();
    assert_eq!(config.provider_keys.get("odos").unwrap(), "test-key");
}

struct HttpFixture {
    response: HttpResponse,
}

#[async_trait::async_trait]
impl HttpClient for HttpFixture {
    async fn execute(
        &self,
        _request: HttpRequest,
        _timeout: Duration,
    ) -> anyhow::Result<HttpResponse> {
        Ok(self.response.clone())
    }
}

#[tokio::test]
async fn json_request_classifies_http_and_json_failures() {
    let client = HttpFixture {
        response: HttpResponse {
            status: 429,
            body: b"secret".to_vec(),
        },
    };
    let error = json_request_as::<Value>(
        &client,
        HttpRequest {
            method: "GET".into(),
            url: "https://example.test".into(),
            headers: HashMap::new(),
            body: None,
        },
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();
    assert_eq!(
        metamatch_backend::error::kind(&error).to_string(),
        "UPSTREAM_RATE_LIMITED"
    );

    let client = HttpFixture {
        response: HttpResponse {
            status: 200,
            body: b"not-json".to_vec(),
        },
    };
    let error = json_request_as::<Value>(
        &client,
        HttpRequest {
            method: "GET".into(),
            url: "https://example.test".into(),
            headers: HashMap::new(),
            body: None,
        },
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();
    assert_eq!(
        metamatch_backend::error::kind(&error).to_string(),
        "UPSTREAM_INVALID_JSON"
    );
}

#[test]
fn url_parameters_are_encoded_by_url() {
    let url = url_with_params(
        "https://example.test/quote",
        &[("amount", "1 2"), ("token", "0xabc")],
    )
    .unwrap();
    assert!(url.contains("amount=1+2"));
    assert!(url.contains("token=0xabc"));
}

fn execution_input() -> Input {
    Input {
        chain_id: 1,
        sell_token: NATIVE,
        buy_token: parse_address("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48").unwrap(),
        sell_amount: "1000000000000000000".into(),
        slippage_bps: 30,
        taker: FIXTURE_TAKER,
    }
}

fn execution_route() -> Route {
    Route {
        provider: "kyber",
        buy_amount: "10000".into(),
        min_buy_amount: "9970".into(),
        sell_amount: "1000000000000000000".into(),
        spender: parse_address("0x1111111111111111111111111111111111111111").unwrap(),
        tx: Tx {
            to: parse_address("0x1111111111111111111111111111111111111111").unwrap(),
            data: "0x12345678".parse().unwrap(),
            value: "1000000000000000000".into(),
        },
        deadline: None,
    }
}

#[test]
fn unified_transaction_has_holder_and_minimum() {
    let mut chain = configured_chain(&config(), 1);
    let route = execution_route();
    chain.router = Some(support::deployment(
        parse_address("0x2222222222222222222222222222222222222222").unwrap(),
    ));
    let route = validate_route(&execution_input(), route).unwrap();
    let tx = swap_transaction(&execution_input(), chain.router.unwrap(), &route).unwrap();
    assert_eq!(tx.to, support::FIXTURE_HOLDER);
    assert_eq!(tx.value, execution_input().sell_amount);
    assert!(!tx.data.is_empty());
}

#[test]
fn route_target_and_value_are_checked() {
    let mut bad = execution_route();
    bad.tx.value = "0".into();
    assert_eq!(
        validate_route(&execution_input(), bad)
            .unwrap_err()
            .downcast_ref::<ErrorKind>()
            .unwrap()
            .to_string(),
        "UNEXPECTED_TRANSACTION_VALUE"
    );
}

#[test]
fn permissionless_routes_preserve_arbitrary_target_spender_and_selector() {
    use alloy_primitives::{Address, Bytes};
    use alloy_sol_types::SolCall;
    use metamatch_backend::execution::{execCall, executeCall};
    let input = execution_input();
    let router = support::deployment(Address::repeat_byte(0x22));
    let mut route = execution_route();
    route.tx.to = Address::repeat_byte(0x55);
    route.spender = Address::repeat_byte(0x66);
    route.tx.data = Bytes::from_static(&[0x87, 0x65, 0x43, 0x21]);
    let route = validate_route(&input, route).unwrap();
    let tx = swap_transaction(&input, router, &route).unwrap();
    let outer = execCall::abi_decode(&tx.data).unwrap();
    let inner = executeCall::abi_decode(&outer.data).unwrap();
    assert_eq!(inner.target, route.tx.to);
    assert_eq!(inner.spender, route.spender);
    assert_eq!(inner.data, route.tx.data);
}

#[tokio::test]
async fn typed_rpc_provider_supplies_block_context() {
    let server = FixtureRpc::default().start().await;
    let source = ContextSource::new(Arc::new(RpcClients::new(Duration::from_secs(1))));
    let config = config();
    let mut chain = chain(&config, 1);
    chain.rpc_url = Some(server.url.clone());
    let context = source.get(&input(1, NATIVE), &chain).await.unwrap();
    assert_eq!(context.block_number, fixture_context().block_number);
    assert_eq!(context.block_hash, fixture_context().block_hash);
    assert_eq!(context.gas_price, fixture_context().gas_price);
}

#[tokio::test]
async fn derives_balance_delta_and_gas_from_sequential_calls() {
    let result = run_simulation(false, false, false).await.unwrap();
    assert_eq!(result.simulation.bought_amount, "100");
    assert_eq!(result.simulation.gas_used, "21000");
    assert_eq!(result.simulation.funding, "overridden");
}

#[tokio::test]
async fn reorg_is_not_success() {
    let error = run_simulation(true, false, false).await.unwrap_err();
    let failure = *error
        .downcast_ref::<metamatch_backend::simulation::SimulationFailure>()
        .unwrap();
    assert!(matches!(
        failure,
        metamatch_backend::simulation::SimulationFailure::Error(_)
    ));
}

#[tokio::test]
async fn unsupported_simulation_method_is_not_reported_as_transport_failure() {
    let error = run_simulation(false, false, true).await.unwrap_err();
    let failure = *error
        .downcast_ref::<metamatch_backend::simulation::SimulationFailure>()
        .unwrap();
    assert!(matches!(
        failure,
        metamatch_backend::simulation::SimulationFailure::Unsupported(_)
    ));
    assert_eq!(
        metamatch_backend::error::kind(&error).to_string(),
        "RPC_METHOD_UNSUPPORTED"
    );
}

#[tokio::test]
async fn create_rejects_a_chain_not_supported_by_current_providers() {
    let config = config();
    let service = Competitions::new(
        config.clone(),
        Services::new(
            Vec::new(),
            &[configured_chain(&config, 1)],
            Arc::new(support::MockContext),
            Arc::new(support::MockSimulation),
        ),
    );
    let error = service.create(input(1, NATIVE)).await.unwrap_err();
    assert_eq!(
        metamatch_backend::error::kind(&error).to_string(),
        "INVALID_INPUT"
    );
    assert!(service.chains().is_empty());
}

#[tokio::test]
async fn provider_failure_isolated_from_available_provider() {
    let config = config();
    let router = parse_address("0x2222222222222222222222222222222222222222").unwrap();
    let service = Competitions::new(
        config.clone(),
        support::competition_services_for(&config, Some(router)),
    );
    let created = service
        .create(Input {
            chain_id: 1,
            sell_token: NATIVE,
            buy_token: FIXTURE_TAKER,
            sell_amount: "1".into(),
            slippage_bps: 30,
            taker: FIXTURE_TAKER,
        })
        .await
        .unwrap();
    let state = created;
    assert_eq!(state.quotes.len(), 1);
    assert!(state.failures.is_empty());
    assert_eq!(state.quotes[0].simulation.bought_amount, "200");
}

#[tokio::test]
async fn create_rejects_chain_outside_current_provider_union() {
    let service = Competitions::new(config(), support::competition_services());
    let error = service
        .create(Input {
            chain_id: 999_999,
            sell_token: NATIVE,
            buy_token: FIXTURE_TAKER,
            sell_amount: "1".into(),
            slippage_bps: 30,
            taker: FIXTURE_TAKER,
        })
        .await
        .unwrap_err();
    assert_eq!(
        metamatch_backend::error::kind(&error).to_string(),
        "INVALID_INPUT"
    );
}

#[tokio::test]
async fn capabilities_list_only_eligible_providers() {
    let config = config();
    let capability_app = create_app(config.clone()).await.unwrap();
    let (status, _, body) = request(&capability_app, "GET", "/v1/capabilities", None, None).await;
    assert_eq!(status, StatusCode::OK);
    let capabilities: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(capabilities["chains"].as_array().unwrap().len(), 17);
    let ethereum = capabilities["chains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|chain| chain["chainId"] == 1)
        .unwrap();
    assert_eq!(
        ethereum["providers"],
        json!(["kyber", "bebop", "odos", "openOcean", "velora"])
    );
}

#[test]
fn execution_preserves_upstream_deadline_without_inventing_a_ttl() {
    use alloy_primitives::U256;
    use alloy_sol_types::{SolCall, sol};
    sol! {
        function exec(address operator, address token, uint256 amount, address target, bytes data) payable returns (bytes);
        function execute(address sellToken, address buyToken, address receiver, uint256 sellAmount, uint256 minBuyAmount, uint256 deadline, address spender, address target, uint256 value, bytes data) payable returns (uint256);
    }
    let mut chain = configured_chain(&config(), 1);
    chain.router = Some(support::deployment(alloy_primitives::Address::repeat_byte(
        0x22,
    )));
    let mut route = execution_route();
    let upstream_deadline = metamatch_backend::domain::now_ms() / 1000 + 120;
    for deadline in [None, Some(upstream_deadline)] {
        route.deadline = deadline;
        let validated = validate_route(&execution_input(), route.clone()).unwrap();
        let tx = swap_transaction(&execution_input(), chain.router.unwrap(), &validated).unwrap();
        let outer = execCall::abi_decode(&tx.data).unwrap();
        let inner = executeCall::abi_decode(&outer.data).unwrap();
        assert_eq!(
            inner.deadline,
            deadline.map(U256::from).unwrap_or(U256::MAX)
        );
        assert_eq!(inner.minBuyAmount, U256::from(9970));
        assert_eq!(inner.receiver, execution_input().taker);
        assert_eq!(outer.operator, chain.router.unwrap().address);
        assert_eq!(tx.to, support::FIXTURE_HOLDER);
        assert_eq!(inner.sellAmount, parse_uint(&route.sell_amount).unwrap());
        assert_eq!(inner.data, route.tx.data);
    }
}

#[test]
fn competition_timeout_configuration_is_bounded() {
    assert_eq!(config().timeout_ms, 6000);
    for (value, valid) in [
        ("99", false),
        ("100", true),
        ("30000", true),
        ("30001", false),
        ("abc", false),
    ] {
        let value_json = value.parse::<u64>().map_or(json!(value), |n| json!(n));
        let result = load_config(&json!({"competitionTimeoutMs":value_json}).to_string());
        assert_eq!(result.is_ok(), valid);
        if valid {
            assert_eq!(result.unwrap().timeout_ms.to_string(), value);
        }
    }
}
