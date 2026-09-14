use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct OpenOceanResponse {
    code: i64,
    data: OpenOceanData,
}

#[derive(Debug, Deserialize)]
struct OpenOceanGasResponse {
    code: i64,
    data: OpenOceanGasData,
}

#[derive(Debug, Deserialize)]
struct OpenOceanGasData {
    standard: OpenOceanGasPrice,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OpenOceanGasPrice {
    Eip1559 {
        #[serde(rename = "legacyGasPrice")]
        legacy_gas_price: Value,
    },
    Legacy(Value),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenOceanData {
    in_amount: String,
    out_amount: String,
    min_out_amount: String,
    from: Option<String>,
    to: String,
    value: Value,
    data: String,
    chain_id: Option<u64>,
}

pub(super) struct OpenOceanProvider {
    pub(super) client: Arc<dyn HttpClient>,
    pub(super) timeout: Duration,
}

const SUPPORTED_CHAINS: &[u64] = &[
    1, 10, 56, 130, 137, 143, 146, 999, 5000, 8453, 9745, 42161, 43114, 59144, 80094, 81457, 534352,
];

#[async_trait]
impl Provider for OpenOceanProvider {
    fn id(&self) -> &'static str {
        "openOcean"
    }

    fn requires_access_key(&self) -> bool {
        false
    }

    fn supported_chains(&self) -> Vec<u64> {
        SUPPORTED_CHAINS.to_vec()
    }

    async fn quote(&self, input: &Input, chain: &Chain, sender: Address) -> anyhow::Result<Route> {
        let chain_id = chain.id.to_string();
        let gas: OpenOceanGasResponse = json_request_as(
            self.client.as_ref(),
            HttpRequest {
                method: "GET".into(),
                url: format!("https://open-api.openocean.finance/v4/{chain_id}/gasPrice"),
                headers: HashMap::new(),
                body: None,
            },
            self.timeout,
        )
        .await?;
        if gas.code != 200 {
            anyhow::bail!(ErrorKind::UpstreamInvalidResponse);
        }
        let price = match gas.data.standard {
            OpenOceanGasPrice::Eip1559 { legacy_gas_price } => legacy_gas_price,
            OpenOceanGasPrice::Legacy(value) => value,
        };
        let gas_price = positive_string(
            &quantity_value(&price)
                .map_err(|error| error.context("OpenOcean standard gas price"))?,
        )?;
        let sell_token = open_ocean_token(chain.id, input.sell_token);
        let buy_token = open_ocean_token(chain.id, input.buy_token);
        let slippage = percentage_string(input.slippage_bps);
        let sender_text = address_string(sender);
        let url = url_with_params(
            &format!("https://open-api.openocean.finance/v4/{chain_id}/swap"),
            &[
                ("inTokenAddress", &sell_token),
                ("outTokenAddress", &buy_token),
                ("amountDecimals", &input.sell_amount),
                ("gasPriceDecimals", &gas_price),
                ("slippage", &slippage),
                ("account", &sender_text),
            ],
        )?;
        let response: OpenOceanResponse = json_request_as(
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
        if response.code != 200 {
            anyhow::bail!(ErrorKind::UpstreamInvalidResponse);
        }
        if response
            .data
            .chain_id
            .is_some_and(|value| value != input.chain_id)
        {
            anyhow::bail!(ErrorKind::UpstreamChainMismatch);
        }
        let tx = RawTransaction {
            to: response.data.to,
            data: response.data.data,
            value: response.data.value,
            from: response.data.from,
        };
        let spender = parse_address(&tx.to)?;
        normalize_route(
            self.id(),
            input,
            sender,
            RouteCandidate {
                sell_amount: positive_string(&response.data.in_amount)?,
                buy_amount: positive_string(&response.data.out_amount)?,
                min_buy_amount: positive_string(&response.data.min_out_amount)?,
                spender,
                tx,
                expires_at: crate::domain::now_ms().saturating_add(20_000),
            },
        )
    }
}

fn open_ocean_token(chain_id: u64, address: Address) -> String {
    if address != NATIVE {
        return address_string(address);
    }
    match chain_id {
        137 => String::from("0x0000000000000000000000000000000000001010"),
        1 | 10 | 56 | 130 | 8453 | 42161 | 59144 | 81457 | 534352 => address_string(NATIVE),
        _ => address_string(Address::ZERO),
    }
}
