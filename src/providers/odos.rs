use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OdosQuoteResponse {
    path_id: String,
    in_amounts: Vec<String>,
    out_amounts: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OdosAssembleResponse {
    transaction: RawTransaction,
}

pub(super) struct OdosProvider {
    pub(super) key: Option<String>,
    pub(super) client: Arc<dyn HttpClient>,
    pub(super) timeout: Duration,
}

const SUPPORTED_CHAINS: &[u64] = &[
    1, 10, 56, 130, 137, 146, 5000, 8453, 42161, 43114, 59144, 534352,
];

#[async_trait]
impl Provider for OdosProvider {
    fn id(&self) -> &'static str {
        "odos"
    }

    fn requires_access_key(&self) -> bool {
        false
    }

    fn supported_chains(&self) -> Vec<u64> {
        SUPPORTED_CHAINS.to_vec()
    }

    async fn quote(&self, input: &Input, _chain: &Chain, sender: Address) -> anyhow::Result<Route> {
        let body = json!({
            "chainId": input.chain_id,
            "inputTokens": [{
                "tokenAddress": odos_token(input.sell_token),
                "amount": input.sell_amount,
            }],
            "outputTokens": [{
                "tokenAddress": odos_token(input.buy_token),
                "proportion": 1,
            }],
            "slippageLimitPercent": percentage_value(input.slippage_bps),
            "userAddr": address_string(sender),
        });
        let mut headers = auth_headers(&[("Content-Type", "application/json")]);
        if let Some(key) = &self.key {
            headers.insert("x-api-key".into(), key.clone());
        }
        let quote: OdosQuoteResponse = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "POST".into(),
                url: "https://api.odos.xyz/sor/quote/v2".into(),
                headers: headers.clone(),
                body: Some(body.to_string()),
            },
            self.timeout,
        )
        .await?;
        let sell_amount = quote
            .in_amounts
            .first()
            .context(ErrorKind::UpstreamInvalidResponse)?;
        let buy_amount = quote
            .out_amounts
            .first()
            .context(ErrorKind::UpstreamInvalidResponse)?;
        let assemble: OdosAssembleResponse = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "POST".into(),
                url: "https://api.odos.xyz/sor/assemble".into(),
                headers,
                body: Some(
                    json!({
                        "userAddr": address_string(sender),
                        "pathId": quote.path_id,
                        "simulate": false,
                    })
                    .to_string(),
                ),
            },
            self.timeout,
        )
        .await?;
        let tx_to = parse_address(&assemble.transaction.to)?;
        normalize_route(
            self.id(),
            input,
            sender,
            RouteCandidate {
                sell_amount: positive_string(sell_amount)?,
                buy_amount: positive_string(buy_amount)?,
                min_buy_amount: minimum(buy_amount, input.slippage_bps)?,
                spender: tx_to,
                tx: assemble.transaction,
                deadline: None,
            },
        )
    }
}

fn odos_token(address: Address) -> String {
    if address == NATIVE {
        address_string(Address::ZERO)
    } else {
        address_string(address)
    }
}

fn percentage_value(bps: u64) -> Value {
    serde_json::from_str(&percentage_string(bps)).expect("generated percentage is valid JSON")
}
