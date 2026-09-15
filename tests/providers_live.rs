use async_trait::async_trait;
use metamatch_backend::{
    config::load_config_from_env,
    domain::{Address, Chain, Input, NATIVE, parse_address, parse_positive},
    http::{HttpClient, HttpRequest, HttpResponse, ReqwestClient},
    providers::create_providers,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const ETH_WETH: &str = "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2";
const ETH_USDC: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
const OP_WETH: &str = "0x4200000000000000000000000000000000000006";
const OP_USDC: &str = "0x0b2C639c533813f4Aa9D7837CAf62653D097Ff85";
const BASE_WETH: &str = "0x4200000000000000000000000000000000000006";
const BASE_USDC: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
const ARB_WETH: &str = "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1";
const ARB_USDC: &str = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831";
const HYPEREVM_WHYPE: &str = "0x5555555555555555555555555555555555555555";
const HYPEREVM_USDT0: &str = "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb";
const BERACHAIN_HONEY: &str = "0xFCBD14DC51f0A4d49d5E53C2E0950e0bC26d0Dce";
const LIVE_TAKER: &str = "0x5Bad996643a924De21b6b2875c85C33F3c5bBcB6";

#[derive(Default)]
struct Observation {
    statuses: Mutex<Vec<u16>>,
    transport_errors: Mutex<Vec<String>>,
}

/// A real reqwest client that records only status and transport metadata for diagnostics.
/// It never replaces, fabricates, or rewrites an upstream response or body.
struct ObservedReqwestClient {
    inner: ReqwestClient,
    observation: Arc<Observation>,
}

#[async_trait]
impl HttpClient for ObservedReqwestClient {
    async fn execute(
        &self,
        request: HttpRequest,
        timeout: Duration,
    ) -> anyhow::Result<HttpResponse> {
        match self.inner.execute(request, timeout).await {
            Ok(response) => {
                self.observation
                    .statuses
                    .lock()
                    .unwrap()
                    .push(response.status);
                Ok(response)
            }
            Err(error) => {
                self.observation
                    .transport_errors
                    .lock()
                    .unwrap()
                    .push(metamatch_backend::error::kind(&error).to_string());
                Err(error)
            }
        }
    }
}

#[derive(Clone, Copy)]
struct ProviderSpec {
    id: &'static str,
    default_chain_id: u64,
}

fn live_enabled() -> bool {
    std::env::var("METAMATCH_RUN_LIVE_PROVIDER_TESTS").as_deref() == Ok("1")
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn env_address(name: &str) -> Option<Address> {
    env_value(name).map(|value| {
        parse_address(&value).unwrap_or_else(|_| panic!("{name} must be a 20-byte 0x address"))
    })
}

fn live_chain_id(default: u64) -> u64 {
    env_value("METAMATCH_LIVE_CHAIN_ID")
        .map(|value| {
            value
                .parse::<u64>()
                .unwrap_or_else(|_| panic!("METAMATCH_LIVE_CHAIN_ID must be an integer"))
        })
        .unwrap_or(default)
}

fn configured_address(kind: &str, chain_id: u64) -> Option<Address> {
    env_address(&format!("METAMATCH_LIVE_{kind}_{chain_id}"))
        .or_else(|| env_address(&format!("METAMATCH_LIVE_{kind}")))
}

fn address(value: &str) -> Address {
    parse_address(value).expect("live test default address is valid")
}

fn live_input(chain_id: u64) -> Option<Input> {
    let (default_sell, default_buy, default_amount) = match chain_id {
        1 => (
            Some(address(ETH_WETH)),
            Some(address(ETH_USDC)),
            "10000000000000000",
        ),
        10 => (
            Some(address(OP_WETH)),
            Some(address(OP_USDC)),
            "10000000000000000",
        ),
        8453 => (
            Some(address(BASE_WETH)),
            Some(address(BASE_USDC)),
            "10000000000000000",
        ),
        42161 => (
            Some(address(ARB_WETH)),
            Some(address(ARB_USDC)),
            "10000000000000000",
        ),
        999 => (
            Some(address(HYPEREVM_WHYPE)),
            Some(address(HYPEREVM_USDT0)),
            "10000000000000000",
        ),
        80094 => (
            Some(NATIVE),
            Some(address(BERACHAIN_HONEY)),
            "1000000000000000",
        ),
        _ => (None, None, "1000000000000000"),
    };
    let sell_token = configured_address("SELL_TOKEN", chain_id).or(default_sell)?;
    let buy_token = configured_address("BUY_TOKEN", chain_id).or(default_buy)?;
    if sell_token == buy_token || buy_token == NATIVE {
        panic!("live sell and buy token must be different and buy token cannot be native");
    }
    let sell_amount = env_value(&format!("METAMATCH_LIVE_SELL_AMOUNT_{chain_id}"))
        .or_else(|| env_value("METAMATCH_LIVE_SELL_AMOUNT"))
        .unwrap_or_else(|| default_amount.into());
    parse_positive(&sell_amount).unwrap_or_else(|_| {
        panic!("METAMATCH_LIVE_SELL_AMOUNT must be a positive decimal integer")
    });
    Some(Input {
        chain_id,
        sell_token,
        buy_token,
        sell_amount,
        slippage_bps: 100,
        taker: address(LIVE_TAKER),
    })
}

fn chain(config: &metamatch_backend::config::Config, chain_id: u64) -> Chain {
    let mut chain = metamatch_backend::chains::configured_chain(config, chain_id);
    if chain_id == 999 && chain.rpc_url.is_none() {
        chain.rpc_url = Some("https://rpc.hyperliquid.xyz/evm".into());
    }
    chain
}

async fn exercise(spec: ProviderSpec) {
    if !live_enabled() {
        eprintln!("SKIP {}: set METAMATCH_RUN_LIVE_PROVIDER_TESTS=1", spec.id);
        return;
    }

    let config = load_config_from_env().expect("live test environment must load as valid config");
    let observation = Arc::new(Observation::default());
    let client: Arc<dyn HttpClient> = Arc::new(ObservedReqwestClient {
        inner: ReqwestClient::default(),
        observation: observation.clone(),
    });
    let provider = create_providers(&config, client)
        .into_iter()
        .find(|provider| provider.id() == spec.id)
        .expect("live provider is registered");
    let chain_id = live_chain_id(spec.default_chain_id);
    if !provider.supported_chains().contains(&chain_id) {
        let reason = if provider.requires_access_key() {
            "required access key is missing or the selected chain is unsupported"
        } else {
            "selected chain is unsupported by this provider"
        };
        eprintln!("SKIP {} on chain {}: {reason}", spec.id, chain_id);
        return;
    }
    let input = live_input(chain_id).unwrap_or_else(|| {
        panic!(
            "{} on chain {} requires METAMATCH_LIVE_BUY_TOKEN_{chain_id} and optionally METAMATCH_LIVE_SELL_TOKEN_{chain_id}",
            spec.id, chain_id
        )
    });
    let chain = chain(&config, chain_id);
    let result = provider.quote(&input, &chain, address(LIVE_TAKER)).await;
    let statuses = observation.statuses.lock().unwrap().clone();
    let transport_errors = observation.transport_errors.lock().unwrap().clone();

    assert!(
        transport_errors.is_empty(),
        "{} transport failure before receiving a complete upstream response: {transport_errors:?}",
        spec.id
    );
    assert!(
        !statuses.is_empty(),
        "{} did not receive an HTTP response from its upstream endpoint",
        spec.id
    );
    assert!(
        statuses.iter().all(|status| (200..300).contains(status)),
        "{} upstream returned a non-success HTTP status: {statuses:?}",
        spec.id
    );

    let route = result.unwrap_or_else(|error| {
        panic!(
            "{} reached upstream with statuses {statuses:?}, but returned {} instead of a usable quote",
            spec.id, metamatch_backend::error::kind(&error)
        )
    });
    assert_eq!(route.provider, spec.id);
    assert_eq!(route.sell_amount, input.sell_amount);
    assert!(parse_positive(&route.buy_amount).is_ok());
    assert!(parse_positive(&route.min_buy_amount).is_ok());
    assert!(
        route
            .deadline
            .is_none_or(|deadline| deadline > metamatch_backend::domain::now_ms() / 1000)
    );
    eprintln!(
        "PASS {} on chain {}: upstream statuses {statuses:?}, quote {} -> {}, minimum {}",
        spec.id, chain_id, route.sell_amount, route.buy_amount, route.min_buy_amount
    );
}

#[tokio::test]
#[ignore = "requires explicit live endpoint opt-in; never run in the default suite"]
async fn live_zero_ex() {
    exercise(ProviderSpec {
        id: "0x",
        default_chain_id: 1,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires explicit live endpoint opt-in; never run in the default suite"]
async fn live_one_inch() {
    exercise(ProviderSpec {
        id: "1inch",
        default_chain_id: 1,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires explicit live endpoint opt-in; never run in the default suite"]
async fn live_kyber() {
    exercise(ProviderSpec {
        id: "kyber",
        default_chain_id: 1,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires explicit live endpoint opt-in; never run in the default suite"]
async fn live_barter() {
    exercise(ProviderSpec {
        id: "barter",
        default_chain_id: 1,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires explicit live endpoint opt-in; never run in the default suite"]
async fn live_bebop() {
    exercise(ProviderSpec {
        id: "bebop",
        default_chain_id: 1,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires explicit live endpoint opt-in; never run in the default suite"]
async fn live_enso() {
    exercise(ProviderSpec {
        id: "enso",
        default_chain_id: 1,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires explicit live endpoint opt-in; never run in the default suite"]
async fn live_hyperbloom() {
    exercise(ProviderSpec {
        id: "hyperBloom",
        default_chain_id: 999,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires explicit live endpoint opt-in; never run in the default suite"]
async fn live_liquid_swap() {
    exercise(ProviderSpec {
        id: "liquidSwap",
        default_chain_id: 999,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires explicit live endpoint opt-in; never run in the default suite"]
async fn live_odos() {
    exercise(ProviderSpec {
        id: "odos",
        default_chain_id: 1,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires explicit live endpoint opt-in; never run in the default suite"]
async fn live_ooga_booga() {
    exercise(ProviderSpec {
        id: "oogaBooga",
        default_chain_id: 80094,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires explicit live endpoint opt-in; never run in the default suite"]
async fn live_okx() {
    exercise(ProviderSpec {
        id: "okx",
        default_chain_id: 1,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires explicit live endpoint opt-in; never run in the default suite"]
async fn live_open_ocean() {
    exercise(ProviderSpec {
        id: "openOcean",
        default_chain_id: 10,
    })
    .await;
}

#[tokio::test]
#[ignore = "requires explicit live endpoint opt-in; never run in the default suite"]
async fn live_velora() {
    exercise(ProviderSpec {
        id: "velora",
        default_chain_id: 1,
    })
    .await;
}
