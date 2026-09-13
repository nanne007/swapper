use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct BebopResponse {
    status: String,
    #[serde(rename = "chainId")]
    chain_id: u64,
    expiry: u64,
    #[serde(rename = "approvalTarget")]
    approval_target: String,
    #[serde(rename = "buyTokens")]
    buy_tokens: HashMap<String, BebopToken>,
    #[serde(rename = "sellTokens")]
    sell_tokens: HashMap<String, BebopToken>,
    taker: Option<String>,
    tx: RawTransaction,
}

#[derive(Debug, Deserialize)]
struct BebopToken {
    amount: String,
    #[serde(rename = "minimumAmount")]
    minimum_amount: Option<String>,
}

pub(super) struct BebopProvider {
    pub(super) key: Option<String>,
    pub(super) client: Arc<dyn HttpClient>,
    pub(super) timeout: Duration,
}

const SUPPORTED_CHAINS: &[u64] = &[1, 10, 56, 137, 999, 8453, 42161];

#[async_trait]
impl Provider for BebopProvider {
    fn id(&self) -> &'static str {
        "bebop"
    }

    fn requires_access_key(&self) -> bool {
        false
    }

    fn supported_chains(&self) -> Vec<u64> {
        SUPPORTED_CHAINS.to_vec()
    }

    async fn quote(&self, input: &Input, chain: &Chain, sender: Address) -> Result<Route, Fault> {
        let sell_token = input.sell_token.to_checksum(None);
        let buy_token = input.buy_token.to_checksum(None);
        let amount = input.sell_amount.clone();
        let taker = sender.to_checksum(None);
        let url = url_with_params(
            &format!("https://api.bebop.xyz/pmm/{}/v3/quote", chain.slug),
            &[
                ("sell_tokens", &sell_token),
                ("buy_tokens", &buy_token),
                ("sell_amounts", &amount),
                ("taker_address", &taker),
                ("receiver_address", &taker),
                ("gasless", "false"),
            ],
        )?;
        let mut headers = HashMap::new();
        if let Some(key) = &self.key {
            headers.insert("Authorization".into(), format!("Bearer {key}"));
        }
        let response: BebopResponse = json_request_as(
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
        if response.chain_id != input.chain_id || response.status != "SIG_SUCCESS" {
            return Err(Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502));
        }
        if response
            .taker
            .as_deref()
            .map(parse_address)
            .transpose()?
            .is_some_and(|value| value != sender)
        {
            return Err(Fault::with_status("UPSTREAM_TAKER_MISMATCH", 502));
        }
        let sell = token_amount(&response.sell_tokens, input.sell_token)?;
        let buy = token_amount(&response.buy_tokens, input.buy_token)?;
        let min_buy_amount = positive_string(
            buy.minimum_amount
                .as_deref()
                .ok_or_else(|| Fault::with_status("UPSTREAM_INVALID_RESPONSE", 502))?,
        )?;
        normalize_route(
            self.id(),
            input,
            sender,
            RouteCandidate {
                sell_amount: positive_string(&sell.amount)?,
                buy_amount: positive_string(&buy.amount)?,
                min_buy_amount,
                spender: parse_address(&response.approval_target)?,
                tx: response.tx,
                expires_at: response.expiry.saturating_mul(1_000),
            },
        )
    }
}

fn token_amount(
    tokens: &HashMap<String, BebopToken>,
    expected: Address,
) -> Result<&BebopToken, Fault> {
    tokens
        .iter()
        .find_map(|(address, token)| {
            parse_address(address)
                .ok()
                .filter(|value| *value == expected)
                .map(|_| token)
        })
        .ok_or_else(|| Fault::with_status("UPSTREAM_TOKEN_MISMATCH", 502))
}
