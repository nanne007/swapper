use crate::{
    competitions::{BuildResponse, Competitions, CreateResponse, Services, Snapshot},
    config::Config,
    domain::{
        BuildRequest, CreateCompetitionRequest, Fault, validate_build_request, validate_input,
    },
};
use axum::{
    Json, Router,
    extract::{
        Path, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::{HeaderValue, StatusCode, header},
    middleware::map_response,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::{
    TypedHeader,
    extract::WithRejection,
    headers::{Authorization, authorization::Bearer},
    typed_header::TypedHeaderRejection,
};
use serde::Serialize;
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
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(axum::extract::DefaultBodyLimit::max(16_384))
        .layer(map_response(no_store))
        .with_state(state.clone());
    App { router, state }
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    storage: &'static str,
}

#[derive(Debug, Serialize)]
struct CapabilitiesResponse {
    chains: Vec<ChainCapabilities>,
}

#[derive(Debug, Serialize)]
struct ChainCapabilities {
    #[serde(rename = "chainId")]
    chain_id: u64,
    name: String,
    providers: Vec<&'static str>,
    #[serde(rename = "rpcConfigured")]
    rpc_configured: bool,
    #[serde(rename = "routerConfigured")]
    router_configured: bool,
    execution: &'static str,
    #[serde(rename = "netFeeComparison")]
    net_fee_comparison: &'static str,
}

async fn health(State(_state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        storage: "ephemeral-memory",
    })
}

async fn capabilities(State(state): State<AppState>) -> Json<CapabilitiesResponse> {
    let chains = state
        .config
        .chains
        .iter()
        .map(|chain| {
            let providers = state
                .competitions
                .provider_ids_for_chain(chain.id)
                .into_iter()
                .collect::<Vec<_>>();
            ChainCapabilities {
                chain_id: chain.id,
                name: chain.name.clone(),
                providers,
                rpc_configured: chain.rpc_url.is_some(),
                router_configured: chain.router.is_some(),
                execution: if chain.router.is_some() {
                    "requires-successful-build"
                } else {
                    "direct-preview-only"
                },
                net_fee_comparison: "requires-simulation-and-prices",
            }
        })
        .collect::<Vec<_>>();
    Json(CapabilitiesResponse { chains })
}

async fn create_competition(
    State(state): State<AppState>,
    WithRejection(Json(body), _): WithRejection<Json<CreateCompetitionRequest>, AppError>,
) -> Result<(StatusCode, Json<CreateResponse>), AppError> {
    let input = validate_input(body)?;
    let created = state.competitions.create(input).await?;
    Ok((StatusCode::ACCEPTED, Json(created)))
}

async fn get_competition(
    State(state): State<AppState>,
    WithRejection(Path(id), _): WithRejection<Path<Uuid>, AppError>,
    auth: WithRejection<TypedHeader<Authorization<Bearer>>, AppError>,
) -> Result<Json<Snapshot>, AppError> {
    let TypedHeader(Authorization(bearer)) = auth.into_inner();
    Ok(Json(
        state.competitions.get(id, Some(bearer.token())).await?,
    ))
}

async fn build(
    State(state): State<AppState>,
    WithRejection(Path((id, quote_id)), _): WithRejection<Path<(Uuid, Uuid)>, AppError>,
    auth: WithRejection<TypedHeader<Authorization<Bearer>>, AppError>,
    WithRejection(Json(body), _): WithRejection<Json<BuildRequest>, AppError>,
) -> Result<Json<BuildResponse>, AppError> {
    let TypedHeader(Authorization(bearer)) = auth.into_inner();
    let (taker, accepted) = validate_build_request(body)?;
    Ok(Json(
        state
            .competitions
            .build(id, quote_id, Some(bearer.token()), taker, &accepted)
            .await?,
    ))
}

async fn not_found() -> AppError {
    AppError(Fault::with_status(
        "NOT_FOUND",
        StatusCode::NOT_FOUND.as_u16(),
    ))
}

async fn method_not_allowed() -> AppError {
    AppError(Fault::with_status(
        "METHOD_NOT_ALLOWED",
        StatusCode::METHOD_NOT_ALLOWED.as_u16(),
    ))
}

async fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Debug)]
pub struct AppError(Fault);

impl From<Fault> for AppError {
    fn from(fault: Fault) -> Self {
        Self(fault)
    }
}

impl From<JsonRejection> for AppError {
    fn from(rejection: JsonRejection) -> Self {
        let status = match rejection.status() {
            StatusCode::UNSUPPORTED_MEDIA_TYPE => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            StatusCode::PAYLOAD_TOO_LARGE => StatusCode::PAYLOAD_TOO_LARGE,
            _ => StatusCode::BAD_REQUEST,
        };
        Self(Fault::with_status(
            match status {
                StatusCode::UNSUPPORTED_MEDIA_TYPE => "UNSUPPORTED_MEDIA_TYPE",
                StatusCode::PAYLOAD_TOO_LARGE => "PAYLOAD_TOO_LARGE",
                _ => "INVALID_INPUT",
            },
            status.as_u16(),
        ))
    }
}

impl From<PathRejection> for AppError {
    fn from(_rejection: PathRejection) -> Self {
        Self(Fault::new("INVALID_INPUT"))
    }
}

impl From<TypedHeaderRejection> for AppError {
    fn from(_rejection: TypedHeaderRejection) -> Self {
        Self(Fault::with_status(
            "INVALID_ACCESS_TOKEN",
            StatusCode::UNAUTHORIZED.as_u16(),
        ))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut response = (status, Json(serde_json::json!({"error":self.0.code}))).into_response();
        if self.0.code == "INVALID_ACCESS_TOKEN" {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}
