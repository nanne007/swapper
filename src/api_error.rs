use crate::error::{ErrorKind, kind};
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use axum_extra::typed_header::TypedHeaderRejection;
use serde::Serialize;

/// Public contract: a closed mapping of safe codes; no internal cause, URL or backtrace.
#[derive(Debug, Serialize)]
pub struct ApiError {
    #[serde(rename = "error")]
    pub code: &'static str,
    #[serde(skip)]
    status: StatusCode,
}

impl From<&anyhow::Error> for ApiError {
    fn from(error: &anyhow::Error) -> Self {
        let (code, status) = match kind(error) {
            ErrorKind::InvalidInput => ("INVALID_INPUT", 400),
            ErrorKind::InvalidTaker => ("INVALID_TAKER", 400),
            ErrorKind::InvalidAccessToken => ("INVALID_ACCESS_TOKEN", 401),
            ErrorKind::TakerMismatch => ("TAKER_MISMATCH", 403),
            ErrorKind::NotFound => ("NOT_FOUND", 404),
            ErrorKind::QuoteNotFound => ("QUOTE_NOT_FOUND", 404),
            ErrorKind::CompetitionNotFoundOrExpired => ("COMPETITION_NOT_FOUND_OR_EXPIRED", 404),
            ErrorKind::MethodNotAllowed => ("METHOD_NOT_ALLOWED", 405),
            ErrorKind::QuoteExpired => ("QUOTE_EXPIRED", 409),
            ErrorKind::MinimumDowngradeRejected => ("MINIMUM_DOWNGRADE_REJECTED", 409),
            ErrorKind::PriceMovedBelowAcceptedMinimum => {
                ("PRICE_MOVED_BELOW_ACCEPTED_MINIMUM", 409)
            }
            ErrorKind::RequoteRequired => ("REQUOTE_REQUIRED", 409),
            ErrorKind::PayloadTooLarge => ("PAYLOAD_TOO_LARGE", 413),
            ErrorKind::UnsupportedMediaType => ("UNSUPPORTED_MEDIA_TYPE", 415),
            ErrorKind::BuildError => ("BUILD_ERROR", 422),
            ErrorKind::BuildReverted => ("BUILD_REVERTED", 422),
            ErrorKind::BuildUnsupported => ("BUILD_UNSUPPORTED", 422),
            ErrorKind::UnsupportedChain => ("UNSUPPORTED_CHAIN", 422),
            ErrorKind::NativeSellUnsupported => ("NATIVE_SELL_UNSUPPORTED", 422),
            ErrorKind::CrossChainRouteUnsupported => ("CROSS_CHAIN_ROUTE_UNSUPPORTED", 422),
            ErrorKind::UpstreamStateOverridesUnsupported => {
                ("UPSTREAM_STATE_OVERRIDES_UNSUPPORTED", 422)
            }
            ErrorKind::RouteNotAllowlisted => ("ROUTE_NOT_ALLOWLISTED", 422),
            ErrorKind::CapacityExceeded => ("CAPACITY_EXCEEDED", 429),
            ErrorKind::BuildCapacityExceeded => ("BUILD_CAPACITY_EXCEEDED", 429),
            ErrorKind::ProviderUnconfigured => ("PROVIDER_UNCONFIGURED", 503),
            ErrorKind::RpcNotConfigured => ("RPC_NOT_CONFIGURED", 503),
            ErrorKind::RpcChainMismatch => ("RPC_CHAIN_MISMATCH", 503),
            ErrorKind::RouterNotConfigured => ("ROUTER_NOT_CONFIGURED", 503),
            ErrorKind::UpstreamTimeout => ("UPSTREAM_TIMEOUT", 504),
            ErrorKind::RpcTimeout => ("RPC_TIMEOUT", 504),
            ErrorKind::UpstreamHttpError => ("UPSTREAM_HTTP_ERROR", 502),
            ErrorKind::UpstreamAuthError => ("UPSTREAM_AUTH_ERROR", 502),
            ErrorKind::UpstreamRateLimited => ("UPSTREAM_RATE_LIMITED", 502),
            ErrorKind::UpstreamEmpty => ("UPSTREAM_EMPTY", 502),
            ErrorKind::UpstreamResponseTooLarge => ("UPSTREAM_RESPONSE_TOO_LARGE", 502),
            ErrorKind::UpstreamInvalidJson => ("UPSTREAM_INVALID_JSON", 502),
            ErrorKind::UpstreamInvalidResponse => ("UPSTREAM_INVALID_RESPONSE", 502),
            ErrorKind::UpstreamAmountMismatch => ("UPSTREAM_AMOUNT_MISMATCH", 502),
            ErrorKind::UpstreamChainMismatch => ("UPSTREAM_CHAIN_MISMATCH", 502),
            ErrorKind::UpstreamChainOrTokenMismatch => ("UPSTREAM_CHAIN_OR_TOKEN_MISMATCH", 502),
            ErrorKind::UpstreamTokenMismatch => ("UPSTREAM_TOKEN_MISMATCH", 502),
            ErrorKind::UpstreamTakerMismatch => ("UPSTREAM_TAKER_MISMATCH", 502),
            ErrorKind::UpstreamRouterChanged => ("UPSTREAM_ROUTER_CHANGED", 502),
            ErrorKind::UpstreamMinimumExceedsQuote => ("UPSTREAM_MINIMUM_EXCEEDS_QUOTE", 502),
            ErrorKind::UnexpectedTransactionValue => ("UNEXPECTED_TRANSACTION_VALUE", 502),
            ErrorKind::InvalidCalldata => ("INVALID_CALLDATA", 502),
            ErrorKind::InvalidRoute => ("INVALID_ROUTE", 502),
            ErrorKind::RpcCallFailed => ("RPC_CALL_FAILED", 502),
            ErrorKind::RpcInvalidResponse => ("RPC_INVALID_RESPONSE", 502),
            ErrorKind::RpcMethodUnsupported => ("RPC_METHOD_UNSUPPORTED", 502),
            ErrorKind::ChainReorgRequote => ("CHAIN_REORG_REQUOTE", 502),
            _ => ("INTERNAL_ERROR", 500),
        };
        Self {
            code,
            status: StatusCode::from_u16(status).expect("static HTTP status"),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        tracing::warn!(error = ?error, "API request failed");
        Self::from(&error)
    }
}
impl From<JsonRejection> for ApiError {
    fn from(error: JsonRejection) -> Self {
        let code = match error.status() {
            StatusCode::UNSUPPORTED_MEDIA_TYPE => ErrorKind::UnsupportedMediaType,
            StatusCode::PAYLOAD_TOO_LARGE => ErrorKind::PayloadTooLarge,
            _ => ErrorKind::InvalidInput,
        };
        anyhow::Error::new(error)
            .context(code)
            .context("API JSON extraction")
            .into()
    }
}
impl From<PathRejection> for ApiError {
    fn from(error: PathRejection) -> Self {
        anyhow::Error::new(error)
            .context(ErrorKind::InvalidInput)
            .context("API path extraction")
            .into()
    }
}
impl From<TypedHeaderRejection> for ApiError {
    fn from(error: TypedHeaderRejection) -> Self {
        anyhow::Error::new(error)
            .context(ErrorKind::InvalidAccessToken)
            .context("API bearer extraction")
            .into()
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let auth = self.code == "INVALID_ACCESS_TOKEN";
        let mut response = (self.status, Json(self)).into_response();
        if auth {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}
