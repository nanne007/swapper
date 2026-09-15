use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VeloraResponse {
    price_route: VeloraPriceRoute,
    tx_params: RawTransaction,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VeloraPriceRoute {
    network: u64,
    src_amount: String,
    dest_amount: String,
    contract_address: Option<String>,
    token_transfer_proxy: Option<String>,
}

pub(super) struct VeloraProvider {
    pub(super) client: Arc<dyn HttpClient>,
    pub(super) timeout: Duration,
}

const SUPPORTED_CHAINS: &[u64] = &[1, 10, 56, 130, 137, 146, 8453, 9745, 42161, 43114];

#[async_trait]
impl Provider for VeloraProvider {
    fn id(&self) -> &'static str {
        "velora"
    }

    fn requires_access_key(&self) -> bool {
        false
    }

    fn supported_chains(&self) -> Vec<u64> {
        SUPPORTED_CHAINS.to_vec()
    }

    async fn quote(&self, input: &Input, _chain: &Chain, sender: Address) -> anyhow::Result<Route> {
        let network = input.chain_id.to_string();
        let sell_token = address_string(input.sell_token);
        let buy_token = address_string(input.buy_token);
        let sender_text = sender.to_checksum(None);
        let slippage = input.slippage_bps.to_string();
        let url = url_with_params(
            "https://api.paraswap.io/swap",
            &[
                ("srcToken", &sell_token),
                ("destToken", &buy_token),
                ("amount", &input.sell_amount),
                ("side", "SELL"),
                ("network", &network),
                ("userAddress", &sender_text),
                ("slippage", &slippage),
                ("version", "6.2"),
                ("ignoreBadUsdPrice", "true"),
            ],
        )?;
        let response: VeloraResponse = json_request_as(
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
        if response.price_route.network != input.chain_id {
            anyhow::bail!(ErrorKind::UpstreamChainMismatch);
        }
        let proxy = response
            .price_route
            .token_transfer_proxy
            .or(response.price_route.contract_address)
            .context(ErrorKind::UpstreamInvalidResponse)?;
        let buy_amount = positive_string(&response.price_route.dest_amount)?;
        normalize_route(
            self.id(),
            input,
            sender,
            RouteCandidate {
                sell_amount: positive_string(&response.price_route.src_amount)?,
                buy_amount: buy_amount.clone(),
                min_buy_amount: minimum(&buy_amount, input.slippage_bps)?,
                spender: parse_address(&proxy)?,
                tx: RawTransaction {
                    to: response.tx_params.to,
                    data: response.tx_params.data,
                    value: response.tx_params.value,
                    from: response.tx_params.from,
                },
                deadline: None,
            },
        )
    }
}
