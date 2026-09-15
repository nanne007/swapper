mod support;

use axum::http::{StatusCode, header};
use metamatch_backend::app::create_app;
use serde_json::json;
use support::{config, config_with, request};
use uuid::Uuid;

#[tokio::test]
async fn request_rejections_use_the_api_error_envelope() {
    let app = create_app(config());

    let (status, _, body) = request(
        &app.router,
        "POST",
        "/v1/competitions",
        Some(json!({"chainId": 1})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, r#"{"error":"INVALID_INPUT"}"#);

    let (status, _, body) = request(&app.router, "POST", "/v1/competitions", None, None).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(body, r#"{"error":"UNSUPPORTED_MEDIA_TYPE"}"#);
}

#[tokio::test]
async fn retired_endpoints_and_router_rejections_are_consistent() {
    let app = create_app(config());
    for (method, path) in [
        ("GET", "/v1/competitions/not-a-uuid".to_owned()),
        ("GET", format!("/v1/competitions/{}", Uuid::new_v4())),
        (
            "POST",
            format!(
                "/v1/competitions/{}/quotes/{}/build",
                Uuid::new_v4(),
                Uuid::new_v4()
            ),
        ),
        ("GET", "/not-found".to_owned()),
    ] {
        let (status, headers, body) = request(&app.router, method, &path, None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(!headers.contains_key(header::WWW_AUTHENTICATE));
        assert_eq!(body, r#"{"error":"NOT_FOUND"}"#);
    }
    let (status, _, body) = request(&app.router, "POST", "/health", None, None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(body, r#"{"error":"METHOD_NOT_ALLOWED"}"#);
}

#[tokio::test]
async fn api_responses_are_not_cached() {
    let app = create_app(config_with(&[("ALCHEMY_API_KEY", "fixture-secret-key")]));
    let (status, headers, body) = request(&app.router, "GET", "/v1/capabilities", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert!(!body.contains("fixture-secret-key"));
    assert!(!body.contains("alchemy.com"));
    assert!(!body.contains("rpcUrl"));
}

#[tokio::test]
async fn one_request_returns_wallet_bound_transactions_and_simulation_context() {
    use metamatch_backend::domain::{NATIVE, PREVIEW_TAKER};
    let config = config();
    let services = support::competition_services_for(
        &config,
        Some(alloy_primitives::Address::repeat_byte(0x22)),
    );
    let app = support::fixture_app(config, services);
    let input = serde_json::to_value(support::input(1, NATIVE)).unwrap();
    for taker in [
        serde_json::Value::Null,
        json!("0x0000000000000000000000000000000000000001"),
    ] {
        let mut invalid = input.clone();
        invalid["taker"] = taker;
        let (status, _, _) =
            request(&app.router, "POST", "/v1/competitions", Some(invalid), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let mut missing = input.clone();
    missing.as_object_mut().unwrap().remove("taker");
    assert_eq!(
        request(&app.router, "POST", "/v1/competitions", Some(missing), None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, headers, body) =
        request(&app.router, "POST", "/v1/competitions", Some(input), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let response: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(response["input"]["taker"], format!("{PREVIEW_TAKER:#x}"));
    let quote = &response["quotes"][0];
    assert_eq!(response["failures"], json!([]));
    assert_eq!(quote["route"]["provider"], "kyber");
    assert_eq!(quote["route"]["buyAmount"], "200");
    assert_eq!(quote["route"]["minBuyAmount"], "199");
    assert!(quote["route"]["tx"].is_object());
    for removed in [
        "status",
        "error",
        "quotedAmount",
        "provider",
        "minBuyAmount",
    ] {
        assert!(quote.get(removed).is_none());
    }
    assert!(quote["simulation"].get("status").is_none());
    assert!(quote["simulation"].get("reason").is_none());
    assert_eq!(quote["approvals"], json!([]));
    assert!(
        quote["transaction"]["data"]
            .as_str()
            .unwrap()
            .starts_with("0x")
    );
    assert_eq!(quote["simulation"]["funding"], "overridden");
    assert_eq!(quote["simulation"]["boughtAmount"], "200");
    assert_eq!(
        quote["simulation"]["blockContext"],
        json!({"number": "0x10", "hash": support::fixture_context().block_hash, "timestamp": 100})
    );
    assert_eq!(quote["simulation"]["simulatedTimestamp"], 101);
    for retired in [
        "expiresAt",
        "accessToken",
        "recommendedQuoteId",
        "direct-preview",
    ] {
        assert!(!body.contains(retired));
    }
}

#[tokio::test]
async fn all_failed_competition_returns_diagnostics_outside_quotes() {
    use metamatch_backend::domain::NATIVE;
    let config = config();
    let services = support::competition_services_for(&config, None);
    let app = support::fixture_app(config, services);
    let (status, _, body) = request(
        &app.router,
        "POST",
        "/v1/competitions",
        Some(serde_json::to_value(support::input(1, NATIVE)).unwrap()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let response: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(response["quotes"], json!([]));
    assert_eq!(response["failures"].as_array().unwrap().len(), 1);
    let failure = &response["failures"][0];
    assert_eq!(failure["provider"], "kyber");
    assert_eq!(failure["status"], "unavailable");
    assert_eq!(failure["error"], "ROUTER_NOT_CONFIGURED");
    for absent in [
        "route",
        "approvals",
        "transaction",
        "quotedAmount",
        "minBuyAmount",
    ] {
        assert!(failure.get(absent).is_none());
    }
}
