use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct OneInchResponse {
    #[serde(rename = "dstAmount")]
    dst_amount: String,
    tx: RawTransaction,
    #[serde(rename = "stateOverrides")]
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
        let transaction = response.tx.into_tx(sender)?;
        let spender = parse_address("0x111111125421ca6dc452d289314280a0f8842a65")?;
        if transaction.to != spender {
            return Err(Fault::with_status("UNEXPECTED_1INCH_CONTRACT", 502));
        }
        Ok(Route {
            provider: self.id(),
            min_buy_amount: minimum(&buy_amount, input.slippage_bps)?,
            buy_amount,
            sell_amount: input.sell_amount.clone(),
            spender,
            tx: transaction,
            expires_at: crate::domain::now_ms() + 20_000,
        })
    }
}

fn is_nonempty_object(value: &Value) -> bool {
    value.as_object().is_some_and(|object| !object.is_empty())
}
