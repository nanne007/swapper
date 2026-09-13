use super::*;
use serde::Deserialize;

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

pub(super) struct ZeroExProvider {
    pub(super) key: Option<String>,
    pub(super) client: Arc<dyn HttpClient>,
    pub(super) timeout: Duration,
}

const SUPPORTED_CHAINS: &[u64] = &[
    1, 10, 56, 130, 137, 143, 146, 999, 5000, 8453, 9745, 42161, 43114, 59144, 80094, 534352,
];

#[async_trait]
impl Provider for ZeroExProvider {
    fn id(&self) -> &'static str {
        "0x"
    }

    fn requires_access_key(&self) -> bool {
        true
    }

    fn supported_chains(&self) -> Vec<u64> {
        chains_if_configured(self.key.is_some(), SUPPORTED_CHAINS)
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
        let tx_to = parse_address(&response.transaction.to)?;
        let allowance_target = response
            .issues
            .and_then(|issues| issues.allowance)
            .and_then(|allowance| allowance.spender)
            .or(response.allowance_target);
        let spender = if input.sell_token == NATIVE {
            tx_to
        } else {
            parse_address(
                allowance_target
                    .as_deref()
                    .ok_or_else(|| Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502))?,
            )?
        };
        normalize_route(
            self.id(),
            input,
            sender,
            RouteCandidate {
                sell_amount: response.sell_amount,
                buy_amount: response.buy_amount,
                min_buy_amount: response.min_buy_amount,
                spender,
                tx: response.transaction,
                expires_at: crate::domain::now_ms().saturating_add(20_000),
            },
        )
    }
}
