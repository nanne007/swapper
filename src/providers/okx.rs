use super::*;
use base64::Engine;
use chrono::{SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

#[derive(Debug, Deserialize)]
struct OkxResponse {
    code: String,
    data: Vec<OkxSwapData>,
}

#[derive(Debug, Deserialize)]
struct OkxSwapData {
    #[serde(rename = "routerResult")]
    router_result: OkxRouterResult,
    tx: OkxTransaction,
}

#[derive(Debug, Deserialize)]
struct OkxRouterResult {
    #[serde(rename = "chainIndex")]
    chain_index: Option<String>,
    #[serde(rename = "fromTokenAmount")]
    from_token_amount: String,
    #[serde(rename = "toTokenAmount")]
    to_token_amount: String,
}

#[derive(Debug, Deserialize)]
struct OkxTransaction {
    to: String,
    data: String,
    value: Value,
    from: Option<String>,
    #[serde(rename = "minReceiveAmount")]
    min_receive_amount: String,
    #[serde(rename = "signatureData", default)]
    signature_data: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OkxApproval {
    #[serde(rename = "approveContract")]
    approve_contract: String,
}

pub(super) struct OkxProvider {
    pub(super) api_key: Option<String>,
    pub(super) secret_key: Option<String>,
    pub(super) passphrase: Option<String>,
    pub(super) project_id: Option<String>,
    pub(super) client: Arc<dyn HttpClient>,
    pub(super) timeout: Duration,
}

const SUPPORTED_CHAINS: &[u64] = &[
    1, 10, 56, 130, 137, 143, 146, 999, 5000, 8453, 9745, 42161, 43114, 59144, 81457, 534352,
];

#[async_trait]
impl Provider for OkxProvider {
    fn id(&self) -> &'static str {
        "okx"
    }

    fn requires_access_key(&self) -> bool {
        true
    }

    fn supported_chains(&self) -> Vec<u64> {
        chains_if_configured(
            self.api_key.is_some() && self.secret_key.is_some() && self.passphrase.is_some(),
            SUPPORTED_CHAINS,
        )
    }

    async fn quote(&self, input: &Input, _chain: &Chain, sender: Address) -> Result<Route, Fault> {
        let (Some(api_key), Some(secret_key), Some(passphrase)) =
            (&self.api_key, &self.secret_key, &self.passphrase)
        else {
            return Err(Fault::with_status("PROVIDER_UNCONFIGURED", 503));
        };
        let mut params = vec![
            ("chainIndex", input.chain_id.to_string()),
            ("amount", input.sell_amount.clone()),
            ("swapMode", String::from("exactIn")),
            ("fromTokenAddress", address_string(input.sell_token)),
            ("toTokenAddress", address_string(input.buy_token)),
            ("slippagePercent", percentage_string(input.slippage_bps)),
            ("userWalletAddress", address_string(sender)),
        ];
        if input.sell_token != NATIVE {
            params.push(("approveTransaction", String::from("true")));
            params.push(("approveAmount", input.sell_amount.clone()));
        }
        let param_refs = params
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect::<Vec<_>>();
        let url = url_with_params(
            "https://web3.okx.com/api/v6/dex/aggregator/swap",
            &param_refs,
        )?;
        let parsed =
            url::Url::parse(&url).map_err(|_| Fault::with_status("UPSTREAM_HTTP_ERROR", 502))?;
        let query = parsed.query().unwrap_or_default();
        let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let signing = format!("{timestamp}GET/api/v6/dex/aggregator/swap?{query}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret_key.as_bytes())
            .map_err(|_| Fault::with_status("UPSTREAM_AUTH_ERROR", 502))?;
        mac.update(signing.as_bytes());
        let signature =
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        let mut headers = auth_headers(&[
            ("OK-ACCESS-KEY", api_key),
            ("OK-ACCESS-SIGN", &signature),
            ("OK-ACCESS-TIMESTAMP", &timestamp),
            ("OK-ACCESS-PASSPHRASE", passphrase),
        ]);
        if let Some(project_id) = &self.project_id {
            headers.insert("OK-ACCESS-PROJECT".into(), project_id.clone());
        }
        let response: OkxResponse = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "GET".into(),
                url,
                headers,
                body: None,
            },
            self.timeout,
        )
        .await?;
        if response.code != "0" {
            return Err(Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502));
        }
        let data = response
            .data
            .into_iter()
            .next()
            .ok_or_else(|| Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502))?;
        if data
            .router_result
            .chain_index
            .as_deref()
            .is_some_and(|chain_id| chain_id != input.chain_id.to_string())
        {
            return Err(Fault::with_status("UPSTREAM_CHAIN_MISMATCH", 502));
        }
        let buy_amount = positive_string(&data.router_result.to_token_amount)?;
        let sell_amount = positive_string(&data.router_result.from_token_amount)?;
        let tx_to = parse_address(&data.tx.to)?;
        let spender = if input.sell_token == NATIVE {
            tx_to
        } else {
            let approval = data
                .tx
                .signature_data
                .first()
                .ok_or_else(|| Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502))?;
            let approval: OkxApproval = serde_json::from_str(approval)
                .map_err(|_| Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502))?;
            parse_address(&approval.approve_contract)?
        };
        normalize_route(
            self.id(),
            input,
            sender,
            RouteCandidate {
                sell_amount,
                buy_amount: buy_amount.clone(),
                min_buy_amount: positive_string(&data.tx.min_receive_amount)?,
                spender,
                tx: RawTransaction {
                    to: data.tx.to,
                    data: data.tx.data,
                    value: data.tx.value,
                    from: data.tx.from,
                },
                expires_at: crate::domain::now_ms().saturating_add(20_000),
            },
        )
    }
}
