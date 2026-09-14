use metamatch_backend::error::ErrorKind;
mod support;

use axum::{Json, Router, response::IntoResponse, routing::post};
use http_body_util::BodyExt;
use metamatch_backend::{
    api_error::ApiError,
    http::{HttpRequest, UpstreamHttpError, json_request, json_request_as},
    rpc::{RpcClients, map_rpc_error},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, time::Duration};

#[tokio::test]
async fn internal_causes_survive_context_but_never_enter_api_errors() {
    let cause = std::io::Error::other("fixture-secret https://rpc.test/v2/fixture-key");
    let error = anyhow::Error::new(cause)
        .context(ErrorKind::RpcCallFailed)
        .context("eth_simulateV1")
        .context("quote simulation");
    assert!(error.source().is_some());
    assert!(error.downcast_ref::<std::io::Error>().is_some());
    assert_eq!(error.chain().count(), 4);
    if std::env::var("RUST_LIB_BACKTRACE").as_deref() == Ok("1") {
        assert_eq!(
            error.backtrace().status(),
            std::backtrace::BacktraceStatus::Captured
        );
    }
    assert!(format!("{error:?}").contains("fixture-secret"));
    let outer = error
        .context("fixture-context-secret")
        .context(ErrorKind::BuildError)
        .context("build");
    assert_eq!(
        metamatch_backend::error::kind(&outer),
        ErrorKind::BuildError
    );
    assert!(outer.downcast_ref::<std::io::Error>().is_some());
    assert!(format!("{outer:?}").contains("eth_simulateV1"));
    assert!(outer.chain().any(|source| source.is::<std::io::Error>()));
    let report = format!("{outer:?}");
    assert!(report.contains("BUILD_ERROR"));
    assert!(report.contains("fixture-secret"));
    assert!(report.contains("fixture-context-secret"));
    let response = ApiError::from(outer).into_response();
    assert_eq!(response.status().as_u16(), 422);
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        r#"{"error":"BUILD_ERROR"}"#
    );

    let unknown = anyhow::anyhow!("unrecognized-private-detail").context("INVALID_INPUT");
    let response = ApiError::from(unknown).into_response();
    assert_eq!(response.status().as_u16(), 500);
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        r#"{"error":"INTERNAL_ERROR"}"#
    );
}

#[derive(Debug, Deserialize)]
struct Envelope {
    data: Vec<Item>,
}
#[derive(Debug, Deserialize)]
struct Item {
    amount: String,
}

fn http_request() -> HttpRequest {
    HttpRequest {
        method: "GET".into(),
        url: "https://fixture.test/quote?key=fixture-secret".into(),
        headers: HashMap::new(),
        body: None,
    }
}

#[tokio::test]
async fn http_status_and_serde_path_are_preserved_as_internal_causes() {
    let client = support::MockHttp::default();
    client
        .responses
        .lock()
        .unwrap()
        .push(metamatch_backend::http::HttpResponse {
            status: 403,
            body: br#"{"message":"fixture-secret access denied"}"#.to_vec(),
        });
    let error = json_request(&client, http_request(), Duration::from_secs(1))
        .await
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<UpstreamHttpError>().unwrap().status,
        403
    );
    let report = format!("{error:?}");
    assert!(report.contains("403"));
    assert!(report.contains("fixture-secret access denied"));
    let source = error.downcast_ref::<UpstreamHttpError>().unwrap();
    assert_eq!(source.body, r#"{"message":"fixture-secret access denied"}"#);
    let response = ApiError::from(error).into_response();
    assert_eq!(response.status().as_u16(), 502);
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        r#"{"error":"UPSTREAM_HTTP_ERROR"}"#
    );

    let client = support::client([json!({"data": [{"amount": "1"}]})]);
    let decoded: Envelope =
        json_request_as(client.as_ref(), http_request(), Duration::from_secs(1))
            .await
            .unwrap();
    assert_eq!(decoded.data[0].amount, "1");
    let client = support::client([json!({"data": [{}]})]);
    let error =
        json_request_as::<Envelope>(client.as_ref(), http_request(), Duration::from_secs(1))
            .await
            .unwrap_err();
    let source = error
        .downcast_ref::<serde_path_to_error::Error<serde_json::Error>>()
        .unwrap();
    assert_eq!(source.path().to_string(), "data[0]");
    assert!(source.inner().to_string().contains("amount"));
    assert_eq!(ApiError::from(&error).code, "UPSTREAM_INVALID_RESPONSE");
}

#[tokio::test]
async fn alloy_rpc_error_keeps_method_code_and_original_cause() {
    use alloy_provider::Provider;
    let router = Router::new().route("/", post(|Json(request): Json<Value>| async move {
        Json(json!({"jsonrpc":"2.0", "id":request["id"], "error":{"code":-38014,"message":"insufficient funds fixture-secret"}}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let rpc = RpcClients::new(Duration::from_secs(2))
        .get(&format!("http://{address}/?key=fixture-secret"))
        .unwrap();
    let error = rpc
        .simulate(&Default::default())
        .block_id(1_u64.into())
        .await
        .map_err(|error| map_rpc_error("eth_simulateV1", error))
        .unwrap_err();
    task.abort();
    let source = error
        .downcast_ref::<alloy_provider::transport::TransportError>()
        .unwrap();
    assert_eq!(source.as_error_resp().unwrap().code, -38014);
    assert!(
        source
            .as_error_resp()
            .unwrap()
            .message
            .contains("fixture-secret")
    );
    let report = format!("{error:?}");
    assert!(report.contains("eth_simulateV1"));
    assert!(report.contains("-38014"));
    assert!(report.contains("fixture-secret"));
}

#[tokio::test]
async fn simulation_preserves_per_call_revert_details_internally() {
    use metamatch_backend::{
        domain::{NATIVE, PREVIEW_TAKER, Simulation},
        simulation::{SimulationCallError, SimulationRequest, Simulator},
    };
    let mut chain = support::chain(&support::config(), 1);
    let server = support::FixtureRpc {
        revert: true,
        ..Default::default()
    }
    .start()
    .await;
    chain.rpc_url = Some(server.url.clone());
    let input = support::input(1, NATIVE);
    let simulator = Simulator::new(std::sync::Arc::new(RpcClients::new(Duration::from_secs(1))));
    let error = simulator
        .run(SimulationRequest {
            input: &input,
            chain: &chain,
            route: &support::fixture_route(&input),
            context: &support::fixture_context(),
            rules: &[],
            taker: PREVIEW_TAKER,
            actual: false,
            min: None,
        })
        .await
        .unwrap_err();
    let simulation = Simulation::from(
        *error
            .downcast_ref::<metamatch_backend::simulation::SimulationFailure>()
            .unwrap(),
    );
    assert!(
        matches!(simulation, Simulation::Reverted { ref reason } if reason == "SIMULATION_REVERTED")
    );
    let source = error.downcast_ref::<SimulationCallError>().unwrap();
    assert_eq!(source.index, 1);
    assert_eq!(source.result.error.as_ref().unwrap().code, 3);
    assert_eq!(
        source
            .result
            .error
            .as_ref()
            .unwrap()
            .data
            .as_ref()
            .unwrap()
            .as_ref(),
        &[0xde, 0xad]
    );
    assert!(format!("{error:?}").contains("fixture-secret"));
    assert!(
        !serde_json::to_string(&simulation)
            .unwrap()
            .contains("fixture-secret")
    );
}

struct SlowProvider;
#[async_trait::async_trait]
impl metamatch_backend::providers::Provider for SlowProvider {
    fn id(&self) -> &'static str {
        "slow"
    }
    fn requires_access_key(&self) -> bool {
        false
    }
    fn supported_chains(&self) -> Vec<u64> {
        vec![1]
    }
    async fn quote(
        &self,
        _: &metamatch_backend::domain::Input,
        _: &metamatch_backend::domain::Chain,
        _: alloy_primitives::Address,
    ) -> anyhow::Result<metamatch_backend::domain::Route> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn provider_deadline_still_produces_a_quote_failure() {
    use metamatch_backend::competitions::{Competitions, Services};
    use std::sync::Arc;
    let config = support::config_with(&[("PROVIDER_TIMEOUT_MS", "100")]);
    let services = Services::new(
        vec![Arc::new(SlowProvider)],
        &[support::chain(&config, 1)],
        Arc::new(support::MockContext),
        Arc::new(support::MockSimulation),
    );
    let competitions = Competitions::new(config, Some(services));
    let created = competitions
        .create(support::input(1, metamatch_backend::domain::NATIVE))
        .await
        .unwrap();
    let result = support::wait_complete(&competitions, &created).await;
    competitions.close().await;
    assert_eq!(result.quotes.len(), 1);
    assert_eq!(result.quotes[0].provider, "slow");
    assert_eq!(result.quotes[0].error.as_deref(), Some("UPSTREAM_TIMEOUT"));
    assert!(result.recommended_quote_id.is_none());
}
