use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnsoResponse {
    amount_out: String,
    min_amount_out: String,
    tx: RawTransaction,
    #[serde(default)]
    route: Vec<EnsoLeg>,
    #[serde(default)]
    pre_transactions: Vec<EnsoPreTransaction>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnsoLeg {
    chain_id: Option<u64>,
    source_chain_id: Option<u64>,
    destination_chain_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct EnsoPreTransaction {
    token: Option<String>,
    spender: Option<String>,
}

pub(super) struct EnsoProvider {
    pub(super) key: Option<String>,
    pub(super) client: Arc<dyn HttpClient>,
    pub(super) timeout: Duration,
}

const SUPPORTED_CHAINS: &[u64] = &[
    1, 10, 56, 130, 137, 143, 146, 999, 8453, 9745, 42161, 43114, 59144, 80094,
];

#[async_trait]
impl Provider for EnsoProvider {
    fn id(&self) -> &'static str {
        "enso"
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
        let sender_text = address_string(sender);
        let body = json!({
            "chainId": chain.id,
            "fromAddress": sender_text,
            "routingStrategy": "router",
            "receiver": address_string(sender),
            "tokenIn": [address_string(input.sell_token)],
            "tokenOut": [address_string(input.buy_token)],
            "amountIn": [input.sell_amount],
            "slippage": input.slippage_bps.to_string(),
        });
        let response: EnsoResponse = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "POST".into(),
                url: "https://api.enso.build/api/v1/shortcuts/route".into(),
                headers: auth_headers(&[
                    ("Authorization", &format!("Bearer {key}")),
                    ("Content-Type", "application/json"),
                ]),
                body: Some(body.to_string()),
            },
            self.timeout,
        )
        .await?;
        if response.route.iter().any(|leg| {
            leg.chain_id.is_some_and(|id| id != chain.id)
                || leg.source_chain_id.is_some_and(|id| id != chain.id)
                || leg.destination_chain_id.is_some_and(|id| id != chain.id)
        }) {
            anyhow::bail!(ErrorKind::CrossChainRouteUnsupported);
        }
        let tx_to = parse_address(&response.tx.to)?;
        let spender = response
            .pre_transactions
            .iter()
            .find(|transaction| {
                transaction
                    .token
                    .as_deref()
                    .and_then(|value| parse_address(value).ok())
                    .is_some_and(|token| token == input.sell_token)
            })
            .and_then(|transaction| transaction.spender.as_deref())
            .map(parse_address)
            .transpose()?
            .unwrap_or(tx_to);
        normalize_route(
            self.id(),
            input,
            sender,
            RouteCandidate {
                sell_amount: input.sell_amount.clone(),
                buy_amount: response.amount_out,
                min_buy_amount: response.min_amount_out,
                spender,
                tx: response.tx,
                expires_at: crate::domain::now_ms().saturating_add(20_000),
            },
        )
    }
}
