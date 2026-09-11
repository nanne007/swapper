use crate::{
    competitions::{BuildResponse, Competitions, CreateResponse, Services, Snapshot},
    config::Config,
    domain::{Fault, parse_build_request, parse_input},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub competitions: Competitions,
}

pub struct App {
    pub router: Router,
    pub state: AppState,
}

impl App {
    pub async fn close(&self) {
        self.state.competitions.close().await;
    }
}

pub fn create_app(config: Config) -> App {
    create_app_with_services(config, None)
}

pub fn create_app_with_services(config: Config, services: Option<Services>) -> App {
    let competitions = Competitions::new(config.clone(), services);
    let state = AppState {
        config: Arc::new(config),
        competitions,
    };
    let router = Router::new()
        .route("/health", get(health))
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/competitions", post(create_competition))
        .route("/v1/competitions/{id}", get(get_competition))
        .route("/v1/competitions/{id}/quotes/{quote_id}/build", post(build))
        .layer(axum::extract::DefaultBodyLimit::max(16_384))
        .with_state(state.clone());
    App { router, state }
}

async fn health(State(_state): State<AppState>) -> Json<Value> {
    Json(json!({"status":"ok","chainId":1,"storage":"ephemeral-memory"}))
}

async fn capabilities(State(state): State<AppState>) -> Json<Value> {
    let chains = state.config.chains.iter().map(|chain| {
        let tokens = chain.tokens.iter().map(|token| json!({"address":token.address,"symbol":token.symbol,"decimals":token.decimals})).collect::<Vec<_>>();
        json!({
            "chainId": chain.id,
            "name": chain.name,
            "tokens": tokens,
            "rpcConfigured": chain.rpc_url.is_some(),
            "routerConfigured": chain.router.is_some(),
            "execution": if chain.router.is_some() { "requires-successful-build" } else { "direct-preview-only" },
            "netFeeComparison": "requires-simulation-and-prices"
        })
    }).collect::<Vec<_>>();
    Json(
        json!({"chainId":1,"chains":chains,"providers":[{"id":"0x","configured":state.config.zero_ex_key.is_some()},{"id":"1inch","configured":state.config.one_inch_key.is_some()},{"id":"kyber","configured":state.config.kyber_client_id.is_some()}]}),
    )
}

async fn create_competition(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<CreateResponse>), AppError> {
    let input = parse_input(body).map_err(AppError)?;
    let created = state.competitions.create(input).await.map_err(AppError)?;
    Ok((StatusCode::ACCEPTED, Json(created)))
}

async fn get_competition(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<Snapshot>, AppError> {
    Ok(Json(
        state
            .competitions
            .get(id, bearer(&headers))
            .await
            .map_err(AppError)?,
    ))
}

async fn build(
    State(state): State<AppState>,
    Path((id, quote_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<BuildResponse>, AppError> {
    let (taker, accepted) = parse_build_request(body).map_err(AppError)?;
    Ok(Json(
        state
            .competitions
            .build(id, quote_id, bearer(&headers), taker, &accepted)
            .await
            .map_err(AppError)?,
    ))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

pub struct AppError(Fault);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(json!({"error":self.0.code}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{NATIVE, PREVIEW_TAKER};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn request(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<Value>,
        authorization: Option<&str>,
    ) -> (StatusCode, String) {
        let mut builder = axum::http::Request::builder().method(method).uri(uri);
        if body.is_some() {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        if let Some(value) = authorization {
            builder = builder.header(header::AUTHORIZATION, value);
        }
        let request = builder
            .body(axum::body::Body::from(
                body.map_or_else(Vec::new, |body| body.to_string().into_bytes()),
            ))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn polling_lifecycle_requires_authentication() {
        let config = crate::config::load_config(&std::collections::HashMap::new()).unwrap();
        let app = create_app(config);
        let input = json!({"chainId":1,"sellToken":format!("{NATIVE:#x}"),"buyToken":"0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48","sellAmount":"1"});
        let (status, _) = request(&app.router, "POST", "/v1/competitions", Some(json!({"chainId":1,"sellToken":format!("{NATIVE:#x}"),"buyToken":"0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48","sellAmount":1})), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, body) =
            request(&app.router, "POST", "/v1/competitions", Some(input), None).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let created: Value = serde_json::from_str(&body).unwrap();
        let id = created["id"].as_str().unwrap();
        let token = created["accessToken"].as_str().unwrap();
        let path = format!("/v1/competitions/{id}");
        let (status, _) = request(&app.router, "GET", &path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let mut completed = None;
        for _ in 0..100 {
            let response = request(
                &app.router,
                "GET",
                &path,
                None,
                Some(&format!("Bearer {token}")),
            )
            .await;
            if response.1.contains("\"status\":\"complete\"") {
                completed = Some(response);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let (status, body) = completed.expect("competition did not complete");
        assert_eq!(status, StatusCode::OK);
        let snapshot: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(snapshot["status"], "complete");
        assert_eq!(snapshot["quotes"].as_array().unwrap().len(), 3);
        assert!(snapshot["recommendedQuoteId"].is_null());
        let quote_id = snapshot["quotes"][0]["id"].as_str().unwrap();
        let (status, body) = request(
            &app.router,
            "POST",
            &format!("{path}/quotes/{quote_id}/build"),
            Some(json!({"taker":format!("{PREVIEW_TAKER:#x}"),"acceptedMinBuyAmount":"1"})),
            Some(&format!("Bearer {token}")),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["error"],
            "ROUTER_NOT_CONFIGURED"
        );
        app.close().await;
    }
}
