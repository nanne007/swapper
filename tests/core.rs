use metamatch_backend::error::ErrorKind;
mod support;

use axum::http::StatusCode;
use metamatch_backend::{
    app::create_app,
    chains::{alchemy_rpc_url, configured_chain, configured_chains},
    competitions::{Competitions, Services},
    config::load_config,
    domain::{
        Input, NATIVE, PREVIEW_TAKER, Route, Rule, Simulation, Tx, minimum, now_ms, parse_address,
        parse_positive, parse_uint, validate_input,
    },
    execution::{swap_transaction, validate_route},
    http::{HttpClient, HttpRequest, HttpResponse, json_request, url_with_params},
    rpc::{ContextProvider, ContextSource, RpcClients},
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use support::{FixtureRpc, chain, config, fixture_context, input, request, run_simulation};

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
    fn parse_input(value: Value) -> anyhow::Result<Input> {
        validate_input(serde_json::from_value(value)?)
    }
    let base = json!({
        "chainId": 1,
        "sellToken": format!("{NATIVE:#x}"),
        "buyToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
        "sellAmount": "1"
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
    let env = HashMap::from([(String::from("RPC_URL_8453"), String::from("http://base"))]);
    let config = load_config(&env).unwrap();
    assert_eq!(
        configured_chain(&config, 8453).rpc_url.as_deref(),
        Some("http://base")
    );
}

#[test]
fn alchemy_base_urls_are_keyless_and_fill_provider_chains() {
    let env = HashMap::from([(String::from("ALCHEMY_API_KEY"), String::from("fixture/key"))]);
    let config = load_config(&env).unwrap();
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
    let env = HashMap::from([
        (String::from("ALCHEMY_API_KEY"), String::from("fixture-key")),
        (String::from("RPC_URL_8453"), String::from("http://base")),
        (String::from("RPC_URL_1"), String::from("http://ethereum")),
        (
            String::from("ETHEREUM_RPC_URL"),
            String::from("http://legacy-ethereum"),
        ),
    ]);
    let config = load_config(&env).unwrap();
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

    let legacy_env = HashMap::from([
        (String::from("ALCHEMY_API_KEY"), String::from("fixture-key")),
        (
            String::from("ETHEREUM_RPC_URL"),
            String::from("http://legacy-ethereum"),
        ),
    ]);
    let legacy_config = load_config(&legacy_env).unwrap();
    let ethereum = configured_chain(&legacy_config, 1);
    assert_eq!(ethereum.rpc_url.as_deref(), Some("http://legacy-ethereum"));
}

#[test]
fn defaults_do_not_contain_chain_or_provider_selection() {
    let config = config();
    assert_eq!(config.port, 3000);
    assert!(config.provider_keys.is_empty());
}

#[test]
fn rejects_bad_rpc_and_reads_provider_keys() {
    let mut env = HashMap::from([
        (
            String::from("RPC_URL_8453"),
            String::from("file:///tmp/rpc"),
        ),
        (String::from("ODOS_API_KEY"), String::from("test-key")),
    ]);
    assert!(load_config(&env).is_err());
    env.insert(String::from("RPC_URL_8453"), String::from("http://base"));
    let config = load_config(&env).unwrap();
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
    let error = json_request(
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
    let error = json_request(
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
        taker: None,
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
            data: "0x12345678".into(),
            value: "1000000000000000000".into(),
        },
        expires_at: now_ms() + 20_000,
    }
}

#[test]
fn unified_transaction_has_holder_and_minimum() {
    let mut chain = configured_chain(&config(), 1);
    let route = execution_route();
    let provider = route.tx.to;
    chain.router = Some(parse_address("0x2222222222222222222222222222222222222222").unwrap());
    let rules = vec![Rule {
        target: provider,
        spender: provider,
        selector: "0x12345678".into(),
    }];
    let tx = swap_transaction(
        &execution_input(),
        &chain,
        &route,
        &rules,
        Some(&minimum("10000", 80).unwrap()),
    )
    .unwrap();
    assert_eq!(tx.to, metamatch_backend::domain::HOLDER);
    assert_eq!(tx.value, execution_input().sell_amount);
    assert!(tx.data.starts_with("0x"));
}

#[test]
fn route_target_and_value_are_checked() {
    let chain = configured_chain(&config(), 1);
    let mut bad = execution_route();
    bad.tx.value = "0".into();
    assert_eq!(
        validate_route(&execution_input(), &chain, &bad, &[], false)
            .unwrap_err()
            .downcast_ref::<ErrorKind>()
            .unwrap()
            .to_string(),
        "UNEXPECTED_TRANSACTION_VALUE"
    );
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
    match result.simulation {
        Simulation::Success {
            bought_amount,
            gas_used,
            funding,
            ..
        } => {
            assert_eq!(bought_amount, "100");
            assert_eq!(gas_used, "21000");
            assert_eq!(funding, "overridden");
        }
        other => panic!("expected success: {other:?}"),
    }
}

#[tokio::test]
async fn reorg_is_not_success() {
    let error = run_simulation(true, false, false).await.unwrap_err();
    let failure = *error
        .downcast_ref::<metamatch_backend::simulation::SimulationFailure>()
        .unwrap();
    assert!(matches!(
        Simulation::from(failure),
        Simulation::Error { .. }
    ));
}

#[tokio::test]
async fn unsupported_simulation_method_is_not_reported_as_transport_failure() {
    let error = run_simulation(false, false, true).await.unwrap_err();
    let failure = *error
        .downcast_ref::<metamatch_backend::simulation::SimulationFailure>()
        .unwrap();
    assert!(matches!(
        Simulation::from(failure),
        Simulation::Unsupported { .. }
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
        Some(Services::new(
            Vec::new(),
            &[configured_chain(&config, 1)],
            Arc::new(support::MockContext),
            Arc::new(support::MockSimulation),
        )),
    );
    let error = service.create(input(1, NATIVE)).await.unwrap_err();
    assert_eq!(
        metamatch_backend::error::kind(&error).to_string(),
        "INVALID_INPUT"
    );
    assert!(service.chains().is_empty());
    service.close().await;
}

#[tokio::test]
async fn provider_failure_isolated_from_available_provider() {
    let config = config();
    let router = parse_address("0x2222222222222222222222222222222222222222").unwrap();
    let service = Competitions::new(
        config.clone(),
        Some(support::competition_services_for(&config, Some(router))),
    );
    let created = service
        .create(Input {
            chain_id: 1,
            sell_token: NATIVE,
            buy_token: PREVIEW_TAKER,
            sell_amount: "1".into(),
            slippage_bps: 30,
            taker: None,
        })
        .await
        .unwrap();
    let state = support::wait_complete(&service, &created).await;
    assert_eq!(state.quotes.len(), 1);
    assert!(state.quotes[0].simulation.as_ref().unwrap().is_success());
    service.close().await;
}

#[tokio::test]
async fn create_rejects_chain_outside_current_provider_union() {
    let service = Competitions::new(config(), Some(support::competition_services()));
    let error = service
        .create(Input {
            chain_id: 999_999,
            sell_token: NATIVE,
            buy_token: PREVIEW_TAKER,
            sell_amount: "1".into(),
            slippage_bps: 30,
            taker: None,
        })
        .await
        .unwrap_err();
    assert_eq!(
        metamatch_backend::error::kind(&error).to_string(),
        "INVALID_INPUT"
    );
    service.close().await;
}

#[tokio::test]
async fn polling_lifecycle_requires_authentication() {
    let config = config();
    let capability_app = create_app(config.clone());
    let (status, _, body) = request(
        &capability_app.router,
        "GET",
        "/v1/capabilities",
        None,
        None,
    )
    .await;
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
    capability_app.close().await;
}
