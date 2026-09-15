use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OneInchResponse {
    dst_amount: String,
    tx: RawTransaction,
    state_overrides: Option<Value>,
}

pub(super) struct OneInchProvider {
    pub(super) key: Option<String>,
    pub(super) client: Arc<dyn HttpClient>,
    pub(super) timeout: Duration,
}

const SUPPORTED_CHAINS: &[u64] = &[
    1, 10, 56, 130, 137, 143, 146, 999, 8453, 42161, 43114, 59144,
];

#[async_trait]
impl Provider for OneInchProvider {
    fn id(&self) -> &'static str {
        "1inch"
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
            anyhow::bail!(ErrorKind::UpstreamStateOverridesUnsupported);
        }
        let buy_amount = positive_string(&response.dst_amount)?;
        let spender = parse_address("0x111111125421ca6dc452d289314280a0f8842a65")?;
        if parse_address(&response.tx.to)? != spender {
            anyhow::bail!(ErrorKind::Unexpected1inchContract);
        }
        normalize_route(
            self.id(),
            input,
            sender,
            RouteCandidate {
                min_buy_amount: minimum(&buy_amount, input.slippage_bps)?,
                buy_amount,
                sell_amount: input.sell_amount.clone(),
                spender,
                tx: response.tx,
                deadline: None,
            },
        )
    }
}

fn is_nonempty_object(value: &Value) -> bool {
    value.as_object().is_some_and(|object| !object.is_empty())
}
