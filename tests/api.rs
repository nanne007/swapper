mod support;

use axum::http::{StatusCode, header};
use metamatch_backend::app::{create_app, create_app_with_services};
use serde_json::json;
use support::{FIXTURE_TAKER, config, config_with, request};
use uuid::Uuid;

#[tokio::test]
async fn serde_and_api_reject_invalid_fields_before_competition() {
    let app = create_app(config()).await.unwrap();
    let input = json!({"chainId":1,"sellToken":"0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","buyToken":"0x1111111111111111111111111111111111111111","sellAmount":"100","taker":"0x3333333333333333333333333333333333333333"});
    for (field, invalid) in [
        ("chainId", json!(0)),
        ("chainId", json!("1")),
        ("chainId", json!(-1)),
        ("sellToken", json!("bad-address")),
        ("buyToken", json!(null)),
        ("taker", json!(42)),
        ("taker", json!([51; 20].to_vec())),
        ("sellToken", json!([17; 20].to_vec())),
        (
            "buyToken",
            json!("1111111111111111111111111111111111111111"),
        ),
        ("sellAmount", json!(100)),
        ("sellAmount", json!("0")),
        ("sellAmount", json!("01")),
        ("sellAmount", json!("1e18")),
        ("sellAmount", json!("1.5")),
        (
            "sellAmount",
            json!("115792089237316195423570985008687907853269984665640564039457584007913129639936"),
        ),
        ("slippageBps", json!(0)),
        ("slippageBps", json!(501)),
        ("slippageBps", json!("30")),
        ("slippageBps", json!(null)),
        ("rpcUrl", json!("http://untrusted")),
        ("buyToken", input["sellToken"].clone()),
        ("sellToken", input["buyToken"].clone()),
    ] {
        let mut invalid_input = input.clone();
        invalid_input[field] = invalid;
        let (status, _, body) =
            request(&app, "POST", "/v1/competitions", Some(invalid_input), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {body}");
        assert_eq!(body, r#"{"error":"INVALID_INPUT"}"#);
    }
    let (status, _, body) =
        request(&app, "POST", "/v1/competitions", Some(input.clone()), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["input"]["slippageBps"],
        30
    );

    use tower::ServiceExt;
    let duplicated = input.to_string().replacen('{', "{\"chainId\":1,", 1);
    let response = app
        .oneshot(
            axum::http::Request::post("/v1/competitions")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(duplicated))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn request_rejections_use_the_api_error_envelope() {
    let app = create_app(config()).await.unwrap();

    let (status, _, body) = request(
        &app,
        "POST",
        "/v1/competitions",
        Some(json!({"chainId": 1})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, r#"{"error":"INVALID_INPUT"}"#);

    let (status, _, body) = request(&app, "POST", "/v1/competitions", None, None).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(body, r#"{"error":"UNSUPPORTED_MEDIA_TYPE"}"#);
}

#[tokio::test]
async fn retired_endpoints_and_router_rejections_are_consistent() {
    let app = create_app(config()).await.unwrap();
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
        let (status, headers, body) = request(&app, method, &path, None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(!headers.contains_key(header::WWW_AUTHENTICATE));
        assert_eq!(body, r#"{"error":"NOT_FOUND"}"#);
    }
    let (status, _, body) = request(&app, "POST", "/health", None, None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(body, r#"{"error":"METHOD_NOT_ALLOWED"}"#);
}

#[tokio::test]
async fn api_responses_are_not_cached() {
    let app = create_app(config_with(
        serde_json::json!({"alchemyApiKey":"fixture-secret-key"}),
    ))
    .await
    .unwrap();
    let (status, headers, body) = request(&app, "GET", "/v1/capabilities", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert!(!body.contains("fixture-secret-key"));
    assert!(!body.contains("alchemy.com"));
    assert!(!body.contains("rpcUrl"));
}

#[tokio::test]
async fn one_request_returns_wallet_bound_transactions_and_simulation_context() {
    use metamatch_backend::domain::NATIVE;
    let config = config();
    let services = support::competition_services_for(
        &config,
        Some(alloy_primitives::Address::repeat_byte(0x22)),
    );
    let app = create_app_with_services(config, services);
    let input = serde_json::to_value(support::input(1, NATIVE)).unwrap();
    for taker in [
        serde_json::Value::Null,
        json!("0x0000000000000000000000000000000000000001"),
    ] {
        let mut invalid = input.clone();
        invalid["taker"] = taker;
        let (status, _, _) = request(&app, "POST", "/v1/competitions", Some(invalid), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let mut missing = input.clone();
    missing.as_object_mut().unwrap().remove("taker");
    assert_eq!(
        request(&app, "POST", "/v1/competitions", Some(missing), None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, headers, body) =
        request(&app, "POST", "/v1/competitions", Some(input), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let response: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(response["input"]["taker"], format!("{FIXTURE_TAKER:#x}"));
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
        json!({"number": 17, "hash": format!("0x{}", "33".repeat(32)), "timestamp": 101})
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
    let app = create_app_with_services(config, services);
    let (status, _, body) = request(
        &app,
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
