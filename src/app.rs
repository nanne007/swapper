use crate::error::ErrorKind;
use crate::{
    api_error::ApiError,
    competitions::{CompetitionResponse, Competitions, Services},
    config::Config,
    domain::Input,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, header},
    middleware::map_response,
    response::Response,
    routing::{get, post},
};
use axum_extra::extract::WithRejection;
use serde::Serialize;
use std::sync::Arc;

pub async fn create_app(config: Config) -> anyhow::Result<Router> {
    let services = Services::production(&config).await?;
    Ok(create_app_with_services(config, services))
}

pub fn create_app_with_services(config: Config, services: Services) -> Router {
    let competitions = Arc::new(Competitions::new(config, services));
    Router::new()
        .route("/health", get(health))
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/competitions", post(create_competition))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(axum::extract::DefaultBodyLimit::max(16_384))
        .layer(map_response(no_store))
        .with_state(competitions)
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

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        storage: "none",
    })
}

async fn capabilities(State(competitions): State<Arc<Competitions>>) -> Json<CapabilitiesResponse> {
    let chains = competitions
        .chains()
        .iter()
        .map(|chain| {
            let providers = competitions.provider_ids_for_chain(chain.id);
            ChainCapabilities {
                chain_id: chain.id,
                name: chain.name.clone(),
                providers,
                rpc_configured: chain.rpc_url.is_some(),
                router_configured: chain.router.is_some(),
                execution: if chain.router.is_some() {
                    "requires-successful-simulation"
                } else {
                    "unavailable"
                },
                net_fee_comparison: "not-included",
            }
        })
        .collect::<Vec<_>>();
    Json(CapabilitiesResponse { chains })
}

async fn create_competition(
    State(competitions): State<Arc<Competitions>>,
    WithRejection(Json(input), _): WithRejection<Json<Input>, ApiError>,
) -> Result<Json<CompetitionResponse>, ApiError> {
    input.validate()?;
    Ok(Json(competitions.create(input).await?))
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
