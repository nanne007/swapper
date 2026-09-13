use super::*;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct BarterRouteResponse {
    status: String,
    #[serde(rename = "outputAmount")]
    output_amount: String,
    #[serde(rename = "inputAmount")]
    input_amount: String,
}

#[derive(Debug, Deserialize)]
struct BarterSwapResponse {
    to: String,
    data: String,
    value: Value,
    route: BarterRouteResponse,
}

pub(super) struct BarterProvider {
    pub(super) key: Option<String>,
    pub(super) client: Arc<dyn HttpClient>,
    pub(super) timeout: Duration,
}

const SUPPORTED_CHAINS: &[u64] = &[1, 8453, 42161];

#[async_trait]
impl Provider for BarterProvider {
    fn id(&self) -> &'static str {
        "barter"
    }

    fn requires_access_key(&self) -> bool {
        true
    }

    fn supported_chains(&self) -> Vec<u64> {
        chains_if_configured(self.key.is_some(), SUPPORTED_CHAINS)
    }

    async fn quote(&self, input: &Input, chain: &Chain, sender: Address) -> Result<Route, Fault> {
        if input.sell_token == NATIVE {
            return Err(Fault::with_status("NATIVE_SELL_UNSUPPORTED", 422));
        }
        let Some(key) = &self.key else {
            return Err(Fault::with_status("PROVIDER_UNCONFIGURED", 503));
        };
        let host = barter_host(chain.id)?;
        let route_request = json!({
            "source": address_string(input.sell_token),
            "target": address_string(input.buy_token),
            "sellAmount": input.sell_amount,
        });
        let route_id = barter_request_id();
        let route: BarterRouteResponse = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "POST".into(),
                url: format!("{host}/route"),
                headers: barter_headers(key, &route_id),
                body: Some(route_request.to_string()),
            },
            self.timeout,
        )
        .await?;
        if route.status != "Normal" {
            return Err(Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502));
        }
        let route_input = positive_string(&route.input_amount)?;
        let route_output = positive_string(&route.output_amount)?;
        if route_input != input.sell_amount {
            return Err(Fault::with_status("UPSTREAM_AMOUNT_MISMATCH", 502));
        }
        // Barter requires minReturn to be at least 98% of the quoted output.
        let min_return = minimum(&route_output, input.slippage_bps.min(200))?;
        let swap_request = json!({
            "source": address_string(input.sell_token),
            "target": address_string(input.buy_token),
            "sellAmount": input.sell_amount,
            "recipient": address_string(sender),
            "origin": address_string(sender),
            "minReturn": min_return,
            "deadline": crate::domain::now_ms().saturating_add(20_000),
        });
        let swap_id = barter_request_id();
        let response: BarterSwapResponse = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "POST".into(),
                url: format!("{host}/swap"),
                headers: barter_headers(key, &swap_id),
                body: Some(swap_request.to_string()),
            },
            self.timeout,
        )
        .await?;
        if response.route.status != "Normal" {
            return Err(Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502));
        }
        let amount_in = positive_string(&response.route.input_amount)?;
        let amount_out = positive_string(&response.route.output_amount)?;
        if amount_in != input.sell_amount || amount_out != route_output {
            return Err(Fault::with_status("UPSTREAM_AMOUNT_MISMATCH", 502));
        }
        let spender = parse_address(&response.to)?;
        normalize_route(
            self.id(),
            input,
            sender,
            RouteCandidate {
                sell_amount: amount_in,
                buy_amount: amount_out,
                min_buy_amount: min_return,
                spender,
                tx: RawTransaction {
                    to: response.to,
                    data: response.data,
                    value: response.value,
                    from: None,
                },
                expires_at: crate::domain::now_ms().saturating_add(20_000),
            },
        )
    }
}

fn barter_host(chain_id: u64) -> Result<&'static str, Fault> {
    match chain_id {
        1 => Ok("https://api2.eth.barterswap.xyz"),
        8453 => Ok("https://api2.base.barterswap.xyz"),
        42161 => Ok("https://api2.arb.barterswap.xyz"),
        _ => Err(Fault::with_status("UNSUPPORTED_CHAIN", 422)),
    }
}

fn barter_request_id() -> String {
    Uuid::new_v4().to_string()
}

fn barter_headers(key: &str, request_id: &str) -> HashMap<String, String> {
    auth_headers(&[
        ("Authorization", &format!("Bearer {key}")),
        ("Content-Type", "application/json"),
        ("X-Request-Id", request_id),
    ])
}
