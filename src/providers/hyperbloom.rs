use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyperBloomResponse {
    chain_id: u64,
    sell_amount: String,
    buy_amount: String,
    sell_token_address: String,
    buy_token_address: String,
    allowance_target: Option<String>,
    value: Value,
    to: Option<String>,
    data: Option<String>,
}

pub(super) struct HyperBloomProvider {
    pub(super) key: Option<String>,
    pub(super) client: Arc<dyn HttpClient>,
    pub(super) timeout: Duration,
}

const SUPPORTED_CHAINS: &[u64] = &[999];

#[async_trait]
impl Provider for HyperBloomProvider {
    fn id(&self) -> &'static str {
        "hyperBloom"
    }

    fn requires_access_key(&self) -> bool {
        true
    }

    fn supported_chains(&self) -> Vec<u64> {
        chains_if_configured(self.key.is_some(), SUPPORTED_CHAINS)
    }

    async fn quote(&self, input: &Input, _chain: &Chain, sender: Address) -> anyhow::Result<Route> {
        let Some(key) = &self.key else {
            anyhow::bail!(ErrorKind::ProviderUnconfigured);
        };
        let sell_token = address_string(input.sell_token);
        let buy_token = address_string(input.buy_token);
        let sender_text = address_string(sender);
        let url = url_with_params(
            "https://api.hyperbloom.xyz/swap/v1/quote",
            &[
                ("sellToken", &sell_token),
                ("buyToken", &buy_token),
                ("sellAmount", &input.sell_amount),
                ("slippagePercentage", &fraction_string(input.slippage_bps)),
                ("takerAddress", &sender_text),
            ],
        )?;
        let response: HyperBloomResponse = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "GET".into(),
                url,
                headers: auth_headers(&[("api-key", key)]),
                body: None,
            },
            self.timeout,
        )
        .await?;
        if response.chain_id != 999
            || parse_address(&response.sell_token_address)? != input.sell_token
            || parse_address(&response.buy_token_address)? != input.buy_token
        {
            anyhow::bail!(ErrorKind::UpstreamChainOrTokenMismatch);
        }
        let to = response.to.context(ErrorKind::UpstreamInvalidResponse)?;
        let data = response.data.context(ErrorKind::UpstreamInvalidResponse)?;
        let spender = response
            .allowance_target
            .and_then(|address| (address != format!("{:#x}", Address::ZERO)).then_some(address))
            .map(|address| parse_address(&address))
            .transpose()?
            .unwrap_or(parse_address(&to)?);
        normalize_route(
            self.id(),
            input,
            sender,
            RouteCandidate {
                sell_amount: response.sell_amount,
                buy_amount: response.buy_amount.clone(),
                min_buy_amount: minimum(&response.buy_amount, input.slippage_bps)?,
                spender,
                tx: RawTransaction {
                    to,
                    data,
                    value: response.value,
                    from: None,
                },
                deadline: None,
            },
        )
    }
}
