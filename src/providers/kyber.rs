use super::*;
use alloy_chains::{Chain as AlloyChain, NamedChain};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct KyberResponse<T> {
    code: i64,
    data: T,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KyberRouteData {
    router_address: String,
    route_summary: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KyberBuildData {
    router_address: String,
    amount_out: String,
    transaction_value: Option<Option<String>>,
    data: String,
}

pub(super) struct KyberProvider {
    pub(super) client: Arc<dyn HttpClient>,
    pub(super) client_id: Option<String>,
    pub(super) timeout: Duration,
}

const SUPPORTED_CHAINS: &[u64] = &[
    1, 10, 56, 130, 137, 143, 146, 999, 5000, 8453, 9745, 42161, 43114, 59144, 80094,
];

#[async_trait]
impl Provider for KyberProvider {
    fn id(&self) -> &'static str {
        "kyber"
    }

    fn requires_access_key(&self) -> bool {
        false
    }

    fn supported_chains(&self) -> Vec<u64> {
        SUPPORTED_CHAINS.to_vec()
    }

    async fn quote(&self, input: &Input, chain: &Chain, sender: Address) -> anyhow::Result<Route> {
        let chain_slug = chain_slug(chain.id)?;
        let base = format!("https://aggregator-api.kyberswap.com/{chain_slug}/api/v1");
        let mut headers = auth_headers(&[("Content-Type", "application/json")]);
        if let Some(client_id) = &self.client_id {
            headers.insert("x-client-id".into(), client_id.clone());
        }
        let routes_url = url_with_params(
            &format!("{base}/routes"),
            &[
                ("tokenIn", &address_string(input.sell_token)),
                ("tokenOut", &address_string(input.buy_token)),
                ("amountIn", &input.sell_amount),
            ],
        )?;
        let route_response: KyberResponse<KyberRouteData> = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "GET".into(),
                url: routes_url,
                headers: headers.clone(),
                body: None,
            },
            self.timeout,
        )
        .await?;
        if route_response.code != 0 {
            anyhow::bail!(ErrorKind::UpstreamInvalidResponse);
        }
        let router_address = parse_address(&route_response.data.router_address)?;
        let summary = route_response.data.route_summary;
        let amount_in = positive_value_field(&summary, "amountIn")?;
        if amount_in != input.sell_amount {
            anyhow::bail!(ErrorKind::UpstreamAmountMismatch);
        }
        let build_body = json!({
            "routeSummary": summary,
            "sender": address_string(sender),
            "recipient": address_string(sender),
            "slippageTolerance": input.slippage_bps,
            "deadline": crate::domain::now_ms() / 1000 + 60,
            "enableGasEstimation": false,
        });
        let built: KyberResponse<KyberBuildData> = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "POST".into(),
                url: format!("{base}/route/build"),
                headers,
                body: Some(build_body.to_string()),
            },
            self.timeout,
        )
        .await?;
        if built.code != 0 {
            anyhow::bail!(ErrorKind::UpstreamInvalidResponse);
        }
        let data = built.data;
        let built_router = parse_address(&data.router_address)?;
        if built_router != router_address {
            anyhow::bail!(ErrorKind::UpstreamRouterChanged);
        }
        let amount_out = parse_positive(&data.amount_out)?.to_string();
        let transaction_value = match data.transaction_value {
            Some(Some(value)) => quantity_string(&value)?,
            Some(None) => return Err(anyhow::Error::new(ErrorKind::UpstreamInvalidResponse)),
            None if input.sell_token == NATIVE => input.sell_amount.clone(),
            None => String::from("0"),
        };
        Ok(Route {
            provider: self.id(),
            buy_amount: amount_out.clone(),
            min_buy_amount: minimum(&amount_out, input.slippage_bps)?,
            sell_amount: input.sell_amount.clone(),
            spender: built_router,
            tx: Tx {
                to: built_router,
                data: parse_hex(&data.data)?,
                value: transaction_value,
            },
            deadline: None,
        })
    }
}

fn chain_slug(chain_id: u64) -> anyhow::Result<&'static str> {
    match AlloyChain::from_id(chain_id).named() {
        Some(NamedChain::Mainnet) => Ok("ethereum"),
        Some(NamedChain::Hyperliquid) => Ok("hyperevm"),
        Some(chain) if SUPPORTED_CHAINS.contains(&chain_id) => Ok(chain.as_str()),
        _ => Err(anyhow::Error::new(ErrorKind::UnsupportedChain)),
    }
}

fn positive_value_field(value: &Value, key: &str) -> anyhow::Result<String> {
    let value = value
        .get(key)
        .and_then(Value::as_str)
        .context(ErrorKind::UpstreamInvalidResponse)?;
    positive_string(value)
}

fn quantity_string(value: &str) -> anyhow::Result<String> {
    quantity_value(&Value::String(value.into()))
}
