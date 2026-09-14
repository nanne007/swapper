use super::*;
use alloy_chains::{Chain as AlloyChain, NamedChain};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BebopResponse {
    status: String,
    chain_id: u64,
    expiry: u64,
    approval_target: String,
    buy_tokens: HashMap<String, BebopToken>,
    sell_tokens: HashMap<String, BebopToken>,
    taker: Option<String>,
    tx: RawTransaction,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BebopToken {
    amount: String,
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

    async fn quote(&self, input: &Input, chain: &Chain, sender: Address) -> anyhow::Result<Route> {
        let chain_slug = chain_slug(chain.id)?;
        let sell_token = input.sell_token.to_checksum(None);
        let buy_token = input.buy_token.to_checksum(None);
        let amount = input.sell_amount.clone();
        let taker = sender.to_checksum(None);
        let url = url_with_params(
            &format!("https://api.bebop.xyz/pmm/{chain_slug}/v3/quote"),
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
            anyhow::bail!(ErrorKind::UpstreamInvalidResponse);
        }
        if response
            .taker
            .as_deref()
            .map(parse_address)
            .transpose()?
            .is_some_and(|value| value != sender)
        {
            anyhow::bail!(ErrorKind::UpstreamTakerMismatch);
        }
        let sell = token_amount(&response.sell_tokens, input.sell_token)?;
        let buy = token_amount(&response.buy_tokens, input.buy_token)?;
        let min_buy_amount = positive_string(
            buy.minimum_amount
                .as_deref()
                .context(ErrorKind::UpstreamInvalidResponse)?,
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

fn chain_slug(chain_id: u64) -> anyhow::Result<&'static str> {
    match AlloyChain::from_id(chain_id).named() {
        Some(NamedChain::Mainnet) => Ok("ethereum"),
        Some(NamedChain::Hyperliquid) => Ok("hyperevm"),
        Some(chain) if SUPPORTED_CHAINS.contains(&chain_id) => Ok(chain.as_str()),
        _ => Err(anyhow::Error::new(ErrorKind::UnsupportedChain)),
    }
}

fn token_amount(
    tokens: &HashMap<String, BebopToken>,
    expected: Address,
) -> anyhow::Result<&BebopToken> {
    tokens
        .iter()
        .find_map(|(address, token)| {
            parse_address(address)
                .ok()
                .filter(|value| *value == expected)
                .map(|_| token)
        })
        .context(ErrorKind::UpstreamTokenMismatch)
}
