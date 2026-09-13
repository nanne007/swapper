mod support;

use axum::http::StatusCode;
use metamatch_backend::{
    app::create_app,
    chains::{CHAIN_CATALOG, configured_chains, spec},
    competitions::{Competitions, Services},
    config::load_config,
    domain::{
        Input, NATIVE, PREVIEW_TAKER, Route, Rule, Simulation, Tx, minimum, now_ms, parse_address,
        parse_input, parse_positive, parse_uint,
    },
    execution::{swap_transaction, validate_route},
    http::{HttpClient, HttpRequest, HttpResponse, json_request, url_with_params},
    rpc::{ContextProvider, ContextSource},
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use support::{FixtureFactory, chain, config, fixture_context, input, request, run_simulation};

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
fn catalog_is_generated_without_chain_configuration() {
    let chains = configured_chains(&HashMap::new()).unwrap();
    assert_eq!(chains.len(), CHAIN_CATALOG.len());
    assert_eq!(spec(143).unwrap().name, "Monad");
    assert!(chains.iter().all(|chain| chain.router.is_none()));
}

#[test]
fn rpc_is_selected_by_chain_id() {
    let env = HashMap::from([(String::from("RPC_URL_8453"), String::from("http://base"))]);
    let chains = configured_chains(&env).unwrap();
    assert_eq!(
        chains
            .iter()
            .find(|chain| chain.id == 8453)
            .unwrap()
            .rpc_url
            .as_deref(),
        Some("http://base")
    );
}

#[test]
fn defaults_to_the_catalog_without_chain_or_provider_configuration() {
    let config = config();
    assert_eq!(config.port, 3000);
    assert!(config.chains.len() > 1);
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
    ) -> Result<HttpResponse, metamatch_backend::domain::Fault> {
        Ok(self.response.clone())
    }
}

#[tokio::test]
async fn json_request_redacts_upstream_body_and_caps_json() {
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
    assert_eq!(error.code, "UPSTREAM_RATE_LIMITED");

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
    assert_eq!(error.code, "UPSTREAM_INVALID_JSON");
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
    let mut chain = config().chains.remove(0);
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
    let chain = config().chains.into_iter().next().unwrap();
    let mut bad = execution_route();
    bad.tx.value = "0".into();
    assert_eq!(
        validate_route(&execution_input(), &chain, &bad, &[], false)
            .unwrap_err()
            .code,
        "UNEXPECTED_TRANSACTION_VALUE"
    );
}

#[tokio::test]
async fn typed_rpc_provider_supplies_block_context() {
    let source = ContextSource::with_factory(
        Duration::from_secs(1),
        Arc::new(FixtureFactory {
            rpc: Default::default(),
        }),
    );
    let config = config();
    let mut chain = chain(&config, 1);
    chain.rpc_url = Some("http://fixture".into());
    let context = source.get(&input(1, NATIVE), &chain).await.unwrap();
    assert_eq!(context.block_number, fixture_context().block_number);
    assert_eq!(context.block_hash, fixture_context().block_hash);
    assert_eq!(context.gas_price, fixture_context().gas_price);
}

#[tokio::test]
async fn derives_balance_delta_and_gas_from_sequential_calls() {
    let result = run_simulation(false, false, false).await;
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
    let result = run_simulation(true, false, false).await;
    assert!(matches!(result.simulation, Simulation::Error { .. }));
}

#[tokio::test]
async fn unsupported_simulation_method_is_not_reported_as_transport_failure() {
    let result = run_simulation(false, false, true).await;
    assert!(matches!(result.simulation, Simulation::Unsupported { .. }));
}

#[tokio::test]
async fn create_completes_without_configured_providers() {
    let config = config();
    let service = Competitions::new(
        config.clone(),
        Some(Services::new(
            Vec::new(),
            &config.chains,
            Arc::new(support::MockContext),
            Arc::new(support::MockSimulation),
        )),
    );
    let created = service.create(input(1, NATIVE)).await.unwrap();
    let state = support::wait_complete(&service, &created).await;
    assert_eq!(state.status, "complete");
    assert!(state.quotes.is_empty());
    assert!(state.recommended_quote_id.is_none());
    service.close().await;
}

#[tokio::test]
async fn provider_failure_isolated_from_available_provider() {
    let mut config = config();
    let router = parse_address("0x2222222222222222222222222222222222222222").unwrap();
    config.chains[0].router = Some(router);
    let service = Competitions::new(
        config.clone(),
        Some(support::competition_services_for(&config)),
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
async fn create_rejects_chain_outside_catalog() {
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
    assert_eq!(error.code, "INVALID_INPUT");
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
