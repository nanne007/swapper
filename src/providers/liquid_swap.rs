use super::*;
use alloy_primitives::U256;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct LiquidSwapResponse {
    success: bool,
    tokens: LiquidSwapTokens,
    execution: LiquidSwapExecution,
}

#[derive(Debug, Deserialize)]
struct LiquidSwapTokens {
    #[serde(rename = "tokenIn")]
    token_in: LiquidSwapToken,
    #[serde(rename = "tokenOut")]
    token_out: LiquidSwapToken,
}

#[derive(Debug, Deserialize)]
struct LiquidSwapToken {
    address: String,
}

#[derive(Debug, Deserialize)]
struct LiquidSwapExecution {
    to: String,
    calldata: String,
    details: LiquidSwapDetails,
}

#[derive(Debug, Deserialize)]
struct LiquidSwapDetails {
    #[serde(rename = "amountIn")]
    amount_in: Value,
    #[serde(rename = "amountOut")]
    amount_out: Value,
    #[serde(rename = "minAmountOut")]
    min_amount_out: Value,
}

pub(super) struct LiquidSwapProvider {
    pub(super) client: Arc<dyn HttpClient>,
    pub(super) timeout: Duration,
}

const SUPPORTED_CHAINS: &[u64] = &[999];

#[async_trait]
impl Provider for LiquidSwapProvider {
    fn id(&self) -> &'static str {
        "liquidSwap"
    }

    fn requires_access_key(&self) -> bool {
        false
    }

    fn supported_chains(&self) -> Vec<u64> {
        SUPPORTED_CHAINS.to_vec()
    }

    async fn quote(&self, input: &Input, chain: &Chain, sender: Address) -> Result<Route, Fault> {
        if input.sell_token == NATIVE {
            return Err(Fault::with_status("NATIVE_SELL_UNSUPPORTED", 422));
        }
        let decimals = self.token_decimals(chain, input.sell_token).await?;
        let amount_in = raw_to_decimal(&input.sell_amount, decimals)?;
        let url = url_with_params(
            "https://api.liqd.ag/v2/route",
            &[
                ("tokenIn", &address_string(input.sell_token)),
                ("tokenOut", &address_string(input.buy_token)),
                ("amountIn", &amount_in),
                ("chainId", &chain.id.to_string()),
                ("multiHop", "true"),
                ("slippage", &percentage_string(input.slippage_bps)),
            ],
        )?;
        let response: LiquidSwapResponse = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "GET".into(),
                url,
                headers: HashMap::new(),
                body: None,
            },
            self.timeout,
        )
        .await?;
        if !response.success
            || parse_address(&response.tokens.token_in.address)? != input.sell_token
            || parse_address(&response.tokens.token_out.address)? != input.buy_token
        {
            return Err(Fault::with_status("UPSTREAM_TOKEN_MISMATCH", 502));
        }
        let details = response.execution.details;
        let sell_amount = quantity_value(&details.amount_in)?;
        let buy_amount = positive_value(&details.amount_out)?;
        let min_buy_amount = positive_value(&details.min_amount_out)?;
        if sell_amount != input.sell_amount {
            return Err(Fault::with_status("UPSTREAM_AMOUNT_MISMATCH", 502));
        }
        let target = parse_address(&response.execution.to)?;
        normalize_route(
            self.id(),
            input,
            sender,
            RouteCandidate {
                sell_amount,
                buy_amount,
                min_buy_amount,
                spender: target,
                tx: RawTransaction {
                    to: response.execution.to,
                    data: response.execution.calldata,
                    value: Value::String(String::from("0")),
                    from: None,
                },
                expires_at: crate::domain::now_ms().saturating_add(20_000),
            },
        )
    }
}

impl LiquidSwapProvider {
    async fn token_decimals(&self, chain: &Chain, token: Address) -> Result<u8, Fault> {
        let Some(rpc_url) = &chain.rpc_url else {
            return Err(Fault::with_status("RPC_NOT_CONFIGURED", 503));
        };
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_call",
            "params": [{
                "to": address_string(token),
                "data": "0x313ce567"
            }, "latest"]
        });
        let response: Value = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "POST".into(),
                url: rpc_url.clone(),
                headers: auth_headers(&[("Content-Type", "application/json")]),
                body: Some(body.to_string()),
            },
            self.timeout,
        )
        .await?;
        let result = response
            .get("result")
            .and_then(Value::as_str)
            .ok_or_else(|| Fault::with_status("RPC_INVALID_RESPONSE", 502))?;
        let decimals = crate::domain::parse_hex_quantity(result)?;
        decimals
            .to_string()
            .parse::<u8>()
            .map_err(|_| Fault::with_status("RPC_INVALID_RESPONSE", 502))
    }
}

fn positive_value(value: &Value) -> Result<String, Fault> {
    let quantity = quantity_value(value)?;
    positive_string(&quantity)
}

fn raw_to_decimal(value: &str, decimals: u8) -> Result<String, Fault> {
    let value = parse_positive(value)?;
    if decimals == 0 {
        return Ok(value.to_string());
    }
    let scale = U256::from(10_u64).pow(U256::from(decimals));
    let whole = value / scale;
    let remainder = value % scale;
    if remainder.is_zero() {
        return Ok(whole.to_string());
    }
    let fraction = format!("{:0width$}", remainder, width = decimals as usize);
    Ok(format!("{whole}.{}", fraction.trim_end_matches('0')))
}
