use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OogaBoogaResponse {
    status: String,
    amount_in: String,
    amount_out: String,
    min_amount_out: String,
    value: Value,
    #[serde(rename = "routerAddr")]
    router_address: String,
    calldata: String,
}

pub(super) struct OogaBoogaProvider {
    pub(super) key: Option<String>,
    pub(super) client: Arc<dyn HttpClient>,
    pub(super) timeout: Duration,
}

const SUPPORTED_CHAINS: &[u64] = &[999, 80094];

#[async_trait]
impl Provider for OogaBoogaProvider {
    fn id(&self) -> &'static str {
        "oogaBooga"
    }

    fn requires_access_key(&self) -> bool {
        true
    }

    fn supported_chains(&self) -> Vec<u64> {
        chains_if_configured(self.key.is_some(), SUPPORTED_CHAINS)
    }

    async fn quote(&self, input: &Input, chain: &Chain, sender: Address) -> anyhow::Result<Route> {
        let Some(key) = &self.key else {
            anyhow::bail!(ErrorKind::ProviderUnconfigured);
        };
        let base = ooga_host(chain.id)?;
        let sender_text = address_string(sender);
        let url = url_with_params(
            &format!("{base}/v1/swap"),
            &[
                ("tokenIn", &ooga_token(input.sell_token)),
                ("tokenOut", &ooga_token(input.buy_token)),
                ("amount", &input.sell_amount),
                ("to", &sender_text),
                ("slippage", &fraction_string(input.slippage_bps)),
            ],
        )?;
        let response: OogaBoogaResponse = json_request_as(
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
        if response.status != "Success" {
            anyhow::bail!(ErrorKind::UpstreamInvalidResponse);
        }
        let router = parse_address(&response.router_address)?;
        normalize_route(
            self.id(),
            input,
            sender,
            RouteCandidate {
                sell_amount: positive_string(&response.amount_in)?,
                buy_amount: positive_string(&response.amount_out)?,
                min_buy_amount: positive_string(&response.min_amount_out)?,
                spender: router,
                tx: RawTransaction {
                    to: response.router_address,
                    data: response.calldata,
                    value: response.value,
                    from: None,
                },
                expires_at: crate::domain::now_ms().saturating_add(20_000),
            },
        )
    }
}

fn ooga_token(address: Address) -> String {
    if address == NATIVE {
        format!("{:#x}", Address::ZERO)
    } else {
        address_string(address)
    }
}

fn ooga_host(chain_id: u64) -> anyhow::Result<&'static str> {
    match chain_id {
        80094 => Ok("https://mainnet.api.oogabooga.io"),
        999 => Ok("https://hyperevm.api.oogabooga.io"),
        _ => Err(anyhow::Error::new(ErrorKind::UnsupportedChain)),
    }
}
