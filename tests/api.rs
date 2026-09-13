mod support;

use axum::http::{StatusCode, header};
use metamatch_backend::app::create_app;
use serde_json::json;
use support::{config, request};
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

    app.close().await;
}

#[tokio::test]
async fn path_auth_and_router_rejections_are_consistent() {
    let app = create_app(config());

    let (status, _, body) = request(
        &app.router,
        "GET",
        "/v1/competitions/not-a-uuid",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, r#"{"error":"INVALID_INPUT"}"#);

    let (status, headers, body) = request(
        &app.router,
        "GET",
        &format!("/v1/competitions/{}", Uuid::new_v4()),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(headers[header::WWW_AUTHENTICATE], "Bearer");
    assert_eq!(body, r#"{"error":"INVALID_ACCESS_TOKEN"}"#);

    let (status, _, body) = request(&app.router, "GET", "/not-found", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, r#"{"error":"NOT_FOUND"}"#);

    let (status, _, body) = request(&app.router, "POST", "/health", None, None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(body, r#"{"error":"METHOD_NOT_ALLOWED"}"#);

    app.close().await;
}

#[tokio::test]
async fn api_responses_are_not_cached() {
    let app = create_app(config());
    let (status, headers, _) = request(&app.router, "GET", "/v1/capabilities", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    app.close().await;
}
