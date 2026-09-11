use crate::{
    config::Config,
    domain::{
        Address, Chain, Fault, HOLDER, Input, NATIVE, ProviderId, Route, Tx, format_quantity,
        minimum, parse_address, parse_hex, parse_positive, parse_uint,
    },
    http::{HttpClient, HttpRequest, auth_headers, json_request_as, url_with_params},
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn enabled(&self) -> bool;
    async fn quote(&self, input: &Input, chain: &Chain, sender: Address) -> Result<Route, Fault>;
}

pub fn create_providers(config: &Config, client: Arc<dyn HttpClient>) -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(ZeroExProvider {
            key: config.zero_ex_key.clone(),
            client: client.clone(),
            timeout: timeout(config),
        }),
        Arc::new(OneInchProvider {
            key: config.one_inch_key.clone(),
            client: client.clone(),
            timeout: timeout(config),
        }),
        Arc::new(KyberProvider {
            client,
            client_id: config.kyber_client_id.clone(),
            timeout: timeout(config),
        }),
    ]
}

fn timeout(config: &Config) -> Duration {
    Duration::from_millis(config.timeout_ms)
}

struct ZeroExProvider {
    key: Option<String>,
    client: Arc<dyn HttpClient>,
    timeout: Duration,
}

#[async_trait]
impl Provider for ZeroExProvider {
    fn id(&self) -> ProviderId {
        ProviderId::ZeroEx
    }

    fn enabled(&self) -> bool {
        self.key.is_some()
    }

    async fn quote(&self, input: &Input, _chain: &Chain, sender: Address) -> Result<Route, Fault> {
        let Some(key) = &self.key else {
            return Err(Fault::with_status("PROVIDER_UNCONFIGURED", 503));
        };
        let url = url_with_params(
            "https://api.0x.org/swap/allowance-holder/quote",
            &[
                ("chainId", &input.chain_id.to_string()),
                ("sellToken", &address_string(input.sell_token)),
                ("buyToken", &address_string(input.buy_token)),
                ("sellAmount", &input.sell_amount),
                ("taker", &address_string(sender)),
                ("slippageBps", &input.slippage_bps.to_string()),
            ],
        )?;
        let response: ZeroExResponse = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "GET".into(),
                url,
                headers: auth_headers(&[("0x-api-key", key), ("0x-version", "v2")]),
                body: None,
            },
            self.timeout,
        )
        .await?;
        if !response.liquidity_available.unwrap_or(false) {
            return Err(Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502));
        }
        let sell_amount = positive_string(&response.sell_amount)?;
        let buy_amount = positive_string(&response.buy_amount)?;
        let min_buy_amount = positive_string(&response.min_buy_amount)?;
        if sell_amount != input.sell_amount {
            return Err(Fault::with_status("UPSTREAM_AMOUNT_MISMATCH", 502));
        }
        let transaction = response.transaction.into_tx()?;
        let spender = response
            .issues
            .and_then(|issues| issues.allowance)
            .and_then(|allowance| allowance.spender)
            .or(response.allowance_target)
            .map(|value| parse_address(&value))
            .transpose()?
            .unwrap_or(HOLDER);
        if spender != HOLDER || transaction.to != HOLDER {
            return Err(Fault::with_status("UNEXPECTED_0X_CONTRACT", 502));
        }
        Ok(Route {
            provider: ProviderId::ZeroEx,
            buy_amount,
            min_buy_amount,
            sell_amount,
            spender,
            tx: transaction,
            expires_at: crate::domain::now_ms() + 20_000,
        })
    }
}

struct OneInchProvider {
    key: Option<String>,
    client: Arc<dyn HttpClient>,
    timeout: Duration,
}

#[async_trait]
impl Provider for OneInchProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OneInch
    }

    fn enabled(&self) -> bool {
        self.key.is_some()
    }

    async fn quote(&self, input: &Input, _chain: &Chain, sender: Address) -> Result<Route, Fault> {
        let Some(key) = &self.key else {
            return Err(Fault::with_status("PROVIDER_UNCONFIGURED", 503));
        };
        let slippage = format!(
            "{}.{:02}",
            input.slippage_bps / 100,
            input.slippage_bps % 100
        );
        let url = url_with_params(
            &format!("https://api.1inch.com/swap/v6.1/{}/swap", input.chain_id),
            &[
                ("src", &address_string(input.sell_token)),
                ("dst", &address_string(input.buy_token)),
                ("amount", &input.sell_amount),
                ("from", &address_string(sender)),
                ("receiver", &address_string(sender)),
                ("slippage", &slippage),
                ("disableEstimate", "true"),
                ("allowPartialFill", "false"),
            ],
        )?;
        let response: OneInchResponse = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "GET".into(),
                url,
                headers: auth_headers(&[("Authorization", &format!("Bearer {key}"))]),
                body: None,
            },
            self.timeout,
        )
        .await?;
        if response
            .state_overrides
            .as_ref()
            .is_some_and(is_nonempty_object)
        {
            return Err(Fault::with_status(
                "UPSTREAM_STATE_OVERRIDES_UNSUPPORTED",
                422,
            ));
        }
        let buy_amount = positive_string(&response.dst_amount)?;
        let transaction = response.tx.into_tx()?;
        let spender = parse_address("0x111111125421ca6dc452d289314280a0f8842a65")?;
        if transaction.to != spender {
            return Err(Fault::with_status("UNEXPECTED_1INCH_CONTRACT", 502));
        }
        Ok(Route {
            provider: ProviderId::OneInch,
            min_buy_amount: minimum(&buy_amount, input.slippage_bps)?,
            buy_amount,
            sell_amount: input.sell_amount.clone(),
            spender,
            tx: transaction,
            expires_at: crate::domain::now_ms() + 20_000,
        })
    }
}

struct KyberProvider {
    client: Arc<dyn HttpClient>,
    client_id: Option<String>,
    timeout: Duration,
}

#[async_trait]
impl Provider for KyberProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Kyber
    }

    fn enabled(&self) -> bool {
        self.client_id.is_some()
    }

    async fn quote(&self, input: &Input, _chain: &Chain, sender: Address) -> Result<Route, Fault> {
        let Some(client_id) = &self.client_id else {
            return Err(Fault::with_status("PROVIDER_UNCONFIGURED", 503));
        };
        let base = "https://aggregator-api.kyberswap.com/ethereum/api/v1";
        let headers = auth_headers(&[
            ("x-client-id", client_id),
            ("Content-Type", "application/json"),
        ]);
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
            return Err(Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502));
        }
        let router_address = parse_address(&route_response.data.router_address)?;
        let summary = route_response.data.route_summary;
        let amount_in = positive_value_field(&summary, "amountIn")?;
        if amount_in != input.sell_amount {
            return Err(Fault::with_status("UPSTREAM_AMOUNT_MISMATCH", 502));
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
            return Err(Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502));
        }
        let data = built.data;
        let built_router = parse_address(&data.router_address)?;
        if built_router != router_address {
            return Err(Fault::with_status("UPSTREAM_ROUTER_CHANGED", 502));
        }
        let amount_out = format_quantity(parse_positive(&data.amount_out)?);
        let transaction_value = match data.transaction_value {
            Some(Some(value)) => quantity_string(&value)?,
            Some(None) => return Err(Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502)),
            None if input.sell_token == NATIVE => input.sell_amount.clone(),
            None => String::from("0"),
        };
        Ok(Route {
            provider: ProviderId::Kyber,
            buy_amount: amount_out.clone(),
            min_buy_amount: minimum(&amount_out, input.slippage_bps)?,
            sell_amount: input.sell_amount.clone(),
            spender: built_router,
            tx: Tx {
                to: built_router,
                data: parse_hex(&data.data)?,
                value: transaction_value,
            },
            expires_at: crate::domain::now_ms() + 20_000,
        })
    }
}

fn address_string(address: Address) -> String {
    format!("{address:#x}")
}

#[derive(Debug, Deserialize)]
struct RawTransaction {
    to: String,
    data: String,
    value: String,
}

impl RawTransaction {
    fn into_tx(self) -> Result<Tx, Fault> {
        Ok(Tx {
            to: parse_address(&self.to)?,
            data: parse_hex(&self.data)?,
            value: quantity_string(&self.value)?,
        })
    }
}

#[derive(Debug, Deserialize)]
struct ZeroExResponse {
    #[serde(rename = "liquidityAvailable")]
    liquidity_available: Option<bool>,
    #[serde(rename = "sellAmount")]
    sell_amount: String,
    #[serde(rename = "buyAmount")]
    buy_amount: String,
    #[serde(rename = "minBuyAmount")]
    min_buy_amount: String,
    transaction: RawTransaction,
    #[serde(rename = "allowanceTarget")]
    allowance_target: Option<String>,
    issues: Option<ZeroExIssues>,
}

#[derive(Debug, Deserialize)]
struct ZeroExIssues {
    allowance: Option<AllowanceIssue>,
}

#[derive(Debug, Deserialize)]
struct AllowanceIssue {
    spender: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OneInchResponse {
    #[serde(rename = "dstAmount")]
    dst_amount: String,
    tx: RawTransaction,
    #[serde(rename = "stateOverrides")]
    state_overrides: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct KyberResponse<T> {
    code: i64,
    data: T,
}

#[derive(Debug, Deserialize)]
struct KyberRouteData {
    #[serde(rename = "routerAddress")]
    router_address: String,
    #[serde(rename = "routeSummary")]
    route_summary: Value,
}

#[derive(Debug, Deserialize)]
struct KyberBuildData {
    #[serde(rename = "routerAddress")]
    router_address: String,
    #[serde(rename = "amountOut")]
    amount_out: String,
    #[serde(rename = "transactionValue")]
    transaction_value: Option<Option<String>>,
    data: String,
}

fn positive_string(value: &str) -> Result<String, Fault> {
    Ok(format_quantity(parse_positive(value)?))
}

fn positive_value_field(value: &Value, key: &str) -> Result<String, Fault> {
    let value = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502))?;
    positive_string(value)
}

fn quantity_value(value: &Value) -> Result<String, Fault> {
    let value = value
        .as_str()
        .ok_or_else(|| Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502))?;
    if value.starts_with("0x") {
        return Ok(format_quantity(crate::domain::parse_hex_quantity(value)?));
    }
    Ok(format_quantity(parse_uint(value)?))
}

fn quantity_string(value: &str) -> Result<String, Fault> {
    quantity_value(&Value::String(value.into()))
}

fn is_nonempty_object(value: &Value) -> bool {
    value.as_object().is_some_and(|object| !object.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::load_config,
        domain::{NATIVE, PREVIEW_TAKER},
        http::HttpResponse,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockHttp {
        requests: Mutex<Vec<HttpRequest>>,
        responses: Mutex<Vec<HttpResponse>>,
    }

    #[async_trait]
    impl HttpClient for MockHttp {
        async fn execute(
            &self,
            request: HttpRequest,
            _timeout: Duration,
        ) -> Result<crate::http::HttpResponse, Fault> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| Fault::new("NO_FIXTURE"))
        }
    }

    fn response(value: Value) -> HttpResponse {
        HttpResponse {
            status: 200,
            body: value.to_string().into_bytes(),
        }
    }

    #[tokio::test]
    async fn providers_normalize_contracts_and_fields() {
        let mut env = std::collections::HashMap::from([
            (String::from("ZERO_EX_API_KEY"), String::from("test-key")),
            (String::from("ONE_INCH_API_KEY"), String::from("test-key")),
        ]);
        let config = load_config(&env).unwrap();
        let chain = config.chains.first().unwrap();
        let input = Input {
            chain_id: 1,
            sell_token: NATIVE,
            buy_token: chain.tokens[2].address,
            sell_amount: "1000000000000000000".into(),
            slippage_bps: 30,
            taker: None,
        };
        let client = Arc::new(MockHttp::default());
        client.responses.lock().unwrap().push(response(json!({"liquidityAvailable":true,"sellAmount":input.sell_amount,"buyAmount":"100","minBuyAmount":"99","allowanceTarget":format!("{HOLDER:#x}"),"transaction":{"to":format!("{HOLDER:#x}"),"data":"0x2213bc0b","value":"0xde0b6b3a7640000"}})));
        let provider = ZeroExProvider {
            key: Some("test-key".into()),
            client: client.clone(),
            timeout: Duration::from_secs(1),
        };
        let route = provider.quote(&input, chain, PREVIEW_TAKER).await.unwrap();
        assert_eq!(route.tx.value, input.sell_amount);
        assert!(
            client.requests.lock().unwrap()[0]
                .url
                .contains(&format!("taker={PREVIEW_TAKER:#x}"))
        );
        env.clear();
    }

    #[test]
    fn quantity_accepts_decimal_and_hex_strings() {
        assert_eq!(
            quantity_value(&Value::String("0xde".into())).unwrap(),
            "222"
        );
        assert_eq!(quantity_value(&Value::String("0".into())).unwrap(), "0");
    }
}
