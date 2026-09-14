use crate::error::ErrorKind;
use crate::{
    api_error::ApiError,
    competitions::{BuildResponse, Competitions, CreateResponse, Services, Snapshot},
    config::Config,
    domain::{BuildRequest, CreateCompetitionRequest, validate_build_request, validate_input},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    middleware::map_response,
    response::Response,
    routing::{get, post},
};
use axum_extra::{
    TypedHeader,
    extract::WithRejection,
    headers::{Authorization, authorization::Bearer},
};
use serde::Serialize;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
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
    let state = AppState {
        competitions: Competitions::new(config, services),
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
#[serde(rename_all = "camelCase")]
struct ChainCapabilities {
    chain_id: u64,
    name: String,
    providers: Vec<&'static str>,
    rpc_configured: bool,
    router_configured: bool,
    execution: &'static str,
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
        .competitions
        .chains()
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
    WithRejection(Json(body), _): WithRejection<Json<CreateCompetitionRequest>, ApiError>,
) -> Result<(StatusCode, Json<CreateResponse>), ApiError> {
    let input = validate_input(body)?;
    let created = state.competitions.create(input).await?;
    Ok((StatusCode::ACCEPTED, Json(created)))
}

async fn get_competition(
    State(state): State<AppState>,
    WithRejection(Path(id), _): WithRejection<Path<Uuid>, ApiError>,
    auth: WithRejection<TypedHeader<Authorization<Bearer>>, ApiError>,
) -> Result<Json<Snapshot>, ApiError> {
    let TypedHeader(Authorization(bearer)) = auth.into_inner();
    Ok(Json(
        state.competitions.get(id, Some(bearer.token())).await?,
    ))
}

async fn build(
    State(state): State<AppState>,
    WithRejection(Path((id, quote_id)), _): WithRejection<Path<(Uuid, Uuid)>, ApiError>,
    auth: WithRejection<TypedHeader<Authorization<Bearer>>, ApiError>,
    WithRejection(Json(body), _): WithRejection<Json<BuildRequest>, ApiError>,
) -> Result<Json<BuildResponse>, ApiError> {
    let TypedHeader(Authorization(bearer)) = auth.into_inner();
    let (taker, accepted) = validate_build_request(body)?;
    Ok(Json(
        state
            .competitions
            .build(id, quote_id, Some(bearer.token()), taker, &accepted)
            .await?,
    ))
}

async fn not_found() -> ApiError {
    anyhow::Error::new(ErrorKind::NotFound).into()
}

async fn method_not_allowed() -> ApiError {
    anyhow::Error::new(ErrorKind::MethodNotAllowed).into()
}

async fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
