mod support;

use metamatch_backend::{
    domain::{NATIVE, PREVIEW_TAKER, parse_address},
    providers::{ProviderRegistry, create_providers},
};
use serde_json::json;
use support::{chain, client, config, config_with, input, provider};

#[tokio::test]
async fn zero_ex_uses_response_transaction_target_for_native_sell() {
    let config = config_with(&[("ZERO_EX_API_KEY", "fixture-key")]);
    let input = input(1, NATIVE);
    let client = client([json!({
        "liquidityAvailable": true,
        "sellAmount": input.sell_amount,
        "buyAmount": "100",
        "minBuyAmount": "99",
        "transaction": {
            "to": "0x3333333333333333333333333333333333333333",
            "data": "0x2213bc0b",
            "value": "0xde0b6b3a7640000"
        }
    })]);
    let quote_provider = provider(&config, "0x", client.clone());
    let route = quote_provider
        .quote(&input, &chain(&config, 1), PREVIEW_TAKER)
        .await
        .unwrap();
    assert_eq!(route.tx.value, input.sell_amount);
    assert_eq!(route.spender, route.tx.to);
    assert!(
        client.requests.lock().unwrap()[0]
            .url
            .contains(&format!("taker={PREVIEW_TAKER:#x}"))
    );
}

#[tokio::test]
async fn zero_ex_keeps_allowance_target_separate_from_entry_point() {
    let config = config_with(&[("ZERO_EX_API_KEY", "fixture-key")]);
    let sell_token = parse_address("0x5555555555555555555555555555555555555555").unwrap();
    let input = input(1, sell_token);
    let client = client([json!({
        "liquidityAvailable": true,
        "sellAmount": input.sell_amount,
        "buyAmount": "100",
        "minBuyAmount": "99",
        "issues": {"allowance": {"spender": "0x2222222222222222222222222222222222222222"}},
        "transaction": {
            "to": "0x3333333333333333333333333333333333333333",
            "data": "0x2213bc0b",
            "value": "0"
        }
    })]);
    let route = provider(&config, "0x", client)
        .quote(&input, &chain(&config, 1), PREVIEW_TAKER)
        .await
        .unwrap();
    assert_eq!(
        route.spender,
        parse_address("0x2222222222222222222222222222222222222222").unwrap()
    );
    assert_eq!(
        route.tx.to,
        parse_address("0x3333333333333333333333333333333333333333").unwrap()
    );
}

#[tokio::test]
async fn barter_uses_route_then_swap_and_enforces_two_percent_minimum() {
    let config = config_with(&[("BARTER_API_KEY", "fixture-key")]);
    let sell_token = parse_address("0x5555555555555555555555555555555555555555").unwrap();
    let mut input = input(1, sell_token);
    input.slippage_bps = 500;
    let client = client([
        json!({
            "to": "0x2222222222222222222222222222222222222222",
            "data": "0x12345678",
            "value": "0",
            "route": {
                "status": "Normal",
                "inputAmount": "1000000000000000000",
                "outputAmount": "200"
            }
        }),
        json!({
            "status": "Normal",
            "inputAmount": input.sell_amount,
            "outputAmount": "200"
        }),
    ]);
    let route = provider(&config, "barter", client.clone())
        .quote(&input, &chain(&config, 1), PREVIEW_TAKER)
        .await
        .unwrap();
    assert_eq!(route.provider, "barter");
    assert_eq!(route.tx.value, "0");
    assert_eq!(route.min_buy_amount, "196");
    let requests = client.requests.lock().unwrap();
    assert!(requests[0].url.ends_with("/route"));
    assert!(requests[1].url.ends_with("/swap"));
    assert!(requests[0].headers.contains_key("Authorization"));
    assert!(requests[0].headers.contains_key("X-Request-Id"));
    let swap_body: serde_json::Value =
        serde_json::from_str(requests[1].body.as_deref().unwrap()).unwrap();
    assert_eq!(swap_body["minReturn"], "196");
}

#[tokio::test]
async fn bebop_maps_token_amounts_and_expiry() {
    let config = config();
    let input = input(1, NATIVE);
    let sell = format!("{NATIVE:#x}");
    let buy = format!("{PREVIEW_TAKER:#x}");
    let client = client([json!({
        "status": "SIG_SUCCESS",
        "chainId": 1,
        "expiry": metamatch_backend::domain::now_ms() / 1000 + 60,
        "approvalTarget": "0x2222222222222222222222222222222222222222",
        "sellTokens": {sell: {"amount": input.sell_amount}},
        "buyTokens": {buy: {"amount": "200", "minimumAmount": "199"}},
        "taker": format!("{PREVIEW_TAKER:#x}"),
        "tx": {
            "to": "0x3333333333333333333333333333333333333333",
            "data": "0x12345678",
            "value": input.sell_amount
        }
    })]);
    let route = provider(&config, "bebop", client.clone())
        .quote(&input, &chain(&config, 1), PREVIEW_TAKER)
        .await
        .unwrap();
    assert_eq!(route.min_buy_amount, "199");
    assert!(route.expires_at > metamatch_backend::domain::now_ms());
    let request = &client.requests.lock().unwrap()[0];
    assert!(request.url.contains("gasless=false"));
    assert!(request.url.contains(&format!(
        "sell_tokens={}",
        input.sell_token.to_checksum(None)
    )));
    assert!(
        request
            .url
            .contains(&format!("buy_tokens={}", input.buy_token.to_checksum(None)))
    );
    assert!(request.url.contains(&format!(
        "taker_address={}",
        PREVIEW_TAKER.to_checksum(None)
    )));
}

#[tokio::test]
async fn enso_rejects_cross_chain_route_legs() {
    let config = config_with(&[("ENSO_API_KEY", "fixture-key")]);
    let input = input(1, NATIVE);
    let client = client([json!({
        "amountOut": "200",
        "minAmountOut": "199",
        "route": [{"chainId": 10}],
        "preTransactions": [],
        "tx": {
            "to": "0x3333333333333333333333333333333333333333",
            "data": "0x12345678",
            "value": input.sell_amount
        }
    })]);
    let error = provider(&config, "enso", client.clone())
        .quote(&input, &chain(&config, 1), PREVIEW_TAKER)
        .await
        .unwrap_err();
    assert_eq!(error.code, "CROSS_CHAIN_ROUTE_UNSUPPORTED");
    let request = &client.requests.lock().unwrap()[0];
    assert_eq!(request.method, "POST");
    let body: serde_json::Value = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["tokenIn"], json!([format!("{NATIVE:#x}")]));
    assert_eq!(body["amountIn"], json!([input.sell_amount]));
    assert!(body.get("tokenInAmountToTransfer").is_none());
}

#[tokio::test]
async fn hyperbloom_requires_matching_chain_and_tokens() {
    let config = config_with(&[("HYPERBLOOM_API_KEY", "fixture-key")]);
    let input = input(999, NATIVE);
    let client = client([json!({
        "chainId": 999,
        "sellAmount": input.sell_amount,
        "buyAmount": "200",
        "sellTokenAddress": format!("{NATIVE:#x}"),
        "buyTokenAddress": format!("{PREVIEW_TAKER:#x}"),
        "allowanceTarget": "0x2222222222222222222222222222222222222222",
        "value": input.sell_amount,
        "to": "0x3333333333333333333333333333333333333333",
        "data": "0x12345678"
    })]);
    let route = provider(&config, "hyperBloom", client)
        .quote(&input, &chain(&config, 999), PREVIEW_TAKER)
        .await
        .unwrap();
    assert_eq!(route.buy_amount, "200");
    assert_eq!(
        route.spender,
        parse_address("0x2222222222222222222222222222222222222222").unwrap()
    );
}

#[tokio::test]
async fn liquid_swap_reads_erc20_decimals_and_builds_route() {
    let config = config_with(&[("RPC_URL_999", "https://rpc.example")]);
    let sell_token = parse_address("0x5555555555555555555555555555555555555555").unwrap();
    let input = input(999, sell_token);
    let client = client([
        json!({
            "success": true,
            "tokens": {
                "tokenIn": {"address": format!("{sell_token:#x}")},
                "tokenOut": {"address": format!("{PREVIEW_TAKER:#x}")}
            },
            "execution": {
                "to": "0x3333333333333333333333333333333333333333",
                "calldata": "0x12345678",
                "details": {
                    "amountIn": input.sell_amount,
                    "amountOut": "200",
                    "minAmountOut": "199"
                }
            }
        }),
        json!({"jsonrpc": "2.0", "id": 1, "result": "0x12"}),
    ]);
    let route = provider(&config, "liquidSwap", client.clone())
        .quote(&input, &chain(&config, 999), PREVIEW_TAKER)
        .await
        .unwrap();
    assert_eq!(route.sell_amount, input.sell_amount);
    assert_eq!(route.tx.value, "0");
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].url, "https://rpc.example");
    assert!(requests[1].url.contains("amountIn=1"));
    assert!(
        requests[1]
            .url
            .contains(&format!("tokenIn={sell_token:#x}"))
    );
}

#[tokio::test]
async fn liquid_swap_rejects_undocumented_native_sell_marker() {
    let config = config();
    let error = provider(&config, "liquidSwap", client([]))
        .quote(&input(999, NATIVE), &chain(&config, 999), PREVIEW_TAKER)
        .await
        .unwrap_err();
    assert_eq!(error.code, "NATIVE_SELL_UNSUPPORTED");
}

#[tokio::test]
async fn odos_assembles_the_quote_path_into_a_route() {
    let config = config();
    let input = input(1, NATIVE);
    let client = client([
        json!({
            "transaction": {
                "to": "0x3333333333333333333333333333333333333333",
                "data": "0x12345678",
                "value": input.sell_amount
            }
        }),
        json!({
            "pathId": "fixture-path",
            "inAmounts": [input.sell_amount],
            "outAmounts": ["200"]
        }),
    ]);
    let route = provider(&config, "odos", client.clone())
        .quote(&input, &chain(&config, 1), PREVIEW_TAKER)
        .await
        .unwrap();
    assert_eq!(route.buy_amount, "200");
    let requests = client.requests.lock().unwrap();
    let body: serde_json::Value =
        serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(
        body["inputTokens"][0]["tokenAddress"],
        "0x0000000000000000000000000000000000000000"
    );
    assert!(!requests[0].headers.contains_key("x-api-key"));
}

#[tokio::test]
async fn ooga_booga_uses_chain_host_and_router_address() {
    let config = config_with(&[("OOGABOOGA_API_KEY", "fixture-key")]);
    let input = input(80094, NATIVE);
    let client = client([json!({
        "status": "Success",
        "amountIn": input.sell_amount,
        "amountOut": "200",
        "minAmountOut": "199",
        "value": input.sell_amount,
        "routerAddr": "0x3333333333333333333333333333333333333333",
        "calldata": "0x12345678"
    })]);
    let route = provider(&config, "oogaBooga", client.clone())
        .quote(&input, &chain(&config, 80094), PREVIEW_TAKER)
        .await
        .unwrap();
    assert_eq!(route.provider, "oogaBooga");
    assert!(
        client.requests.lock().unwrap()[0]
            .url
            .starts_with("https://mainnet.api.oogabooga.io/v1/swap?")
    );
}

#[tokio::test]
async fn okx_v6_uses_upstream_minimum_and_approval_target() {
    let config = config_with(&[
        ("OKX_API_KEY", "fixture-key"),
        ("OKX_SECRET_KEY", "fixture-secret"),
        ("OKX_API_PASSPHRASE", "fixture-passphrase"),
    ]);
    let sell_token = parse_address("0x5555555555555555555555555555555555555555").unwrap();
    let input = input(1, sell_token);
    let client = client([json!({
        "code": "0",
        "data": [{
            "routerResult": {
                "chainIndex": "1",
                "fromTokenAmount": input.sell_amount,
                "toTokenAmount": "200"
            },
            "tx": {
                "to": "0x3333333333333333333333333333333333333333",
                "data": "0x12345678",
                "value": "0",
                "minReceiveAmount": "197",
                "signatureData": ["{\"approveContract\":\"0x2222222222222222222222222222222222222222\",\"approveTxCalldata\":\"0x095ea7b3\"}"]
            }
        }]
    })]);
    let route = provider(&config, "okx", client.clone())
        .quote(&input, &chain(&config, 1), PREVIEW_TAKER)
        .await
        .unwrap();
    assert_eq!(route.buy_amount, "200");
    assert_eq!(route.min_buy_amount, "197");
    assert_eq!(
        route.spender,
        parse_address("0x2222222222222222222222222222222222222222").unwrap()
    );
    let request = &client.requests.lock().unwrap()[0];
    assert!(request.url.contains("/api/v6/dex/aggregator/swap?"));
    assert!(request.url.contains("slippagePercent=0.30"));
    assert!(request.url.contains("approveTransaction=true"));
    assert!(
        request
            .url
            .contains(&format!("approveAmount={}", input.sell_amount))
    );
    assert!(request.headers.contains_key("OK-ACCESS-SIGN"));
    assert!(request.headers.contains_key("OK-ACCESS-TIMESTAMP"));
    assert!(!request.headers.contains_key("OK-ACCESS-PROJECT"));
}

#[tokio::test]
async fn open_ocean_and_velora_normalize_swap_transactions() {
    let config = config();
    let open_input = input(10, NATIVE);
    let open_ocean_client = client([
        json!({
            "code": 200,
            "data": {
                "inAmount": open_input.sell_amount,
                "outAmount": "200",
                "minOutAmount": "199",
                "chainId": 10,
                "from": format!("{PREVIEW_TAKER:#x}"),
                "to": "0x3333333333333333333333333333333333333333",
                "value": open_input.sell_amount,
                "data": "0x12345678"
            }
        }),
        json!({"code": 200, "data": {"standard": 1000384}}),
    ]);
    assert_eq!(
        provider(&config, "openOcean", open_ocean_client.clone())
            .quote(&open_input, &chain(&config, 10), PREVIEW_TAKER)
            .await
            .unwrap()
            .buy_amount,
        "200"
    );
    {
        let requests = open_ocean_client.requests.lock().unwrap();
        assert!(requests[0].url.ends_with("/v4/10/gasPrice"));
        assert!(requests[1].url.contains("gasPriceDecimals=1000384"));
        assert!(
            requests[1]
                .url
                .contains(&format!("inTokenAddress={NATIVE:#x}"))
        );
    }

    let input = input(1, NATIVE);
    let velora_client = client([json!({
        "priceRoute": {
            "network": 1,
            "srcAmount": input.sell_amount,
            "destAmount": "200",
            "tokenTransferProxy": "0x2222222222222222222222222222222222222222"
        },
        "txParams": {
            "to": "0x3333333333333333333333333333333333333333",
            "data": "0x12345678",
            "value": input.sell_amount
        }
    })]);
    assert_eq!(
        provider(&config, "velora", velora_client)
            .quote(&input, &chain(&config, 1), PREVIEW_TAKER)
            .await
            .unwrap()
            .min_buy_amount,
        "199"
    );
}

#[test]
fn registry_is_key_aware_and_chain_specific() {
    let config = config_with(&[("ZERO_EX_API_KEY", "test-key")]);
    let providers = create_providers(&config, std::sync::Arc::new(support::MockHttp::default()));
    assert_eq!(providers.len(), 13);
    let registry = ProviderRegistry::new(providers, &config.chains);
    let ethereum = registry.by_chain().get(&1).unwrap();
    assert!(ethereum.contains(&"0x"));
    assert!(!ethereum.contains(&"1inch"));
    assert!(!ethereum.contains(&"barter"));
    assert!(ethereum.contains(&"openOcean"));
    assert!(ethereum.contains(&"kyber"));
    assert!(ethereum.contains(&"odos"));
    assert!(ethereum.contains(&"bebop"));
    assert!(ethereum.contains(&"velora"));
    assert!(
        !registry
            .by_chain()
            .get(&999)
            .unwrap()
            .contains(&"hyperBloom")
    );
    assert!(
        registry
            .for_chain(8453)
            .iter()
            .any(|provider| provider.id() == "0x")
    );
}

#[test]
fn required_provider_credentials_gate_capability() {
    let config = config_with(&[
        ("OKX_API_KEY", "fixture-key"),
        ("OKX_SECRET_KEY", "fixture-secret"),
        ("OKX_API_PASSPHRASE", "fixture-passphrase"),
    ]);
    let providers = create_providers(&config, std::sync::Arc::new(support::MockHttp::default()));
    let registry = ProviderRegistry::new(providers, &config.chains);
    assert!(
        registry
            .for_chain(1)
            .iter()
            .any(|provider| provider.id() == "okx")
    );
}

#[test]
fn provider_metadata_exposes_effective_supported_chains() {
    let without_keys = config();
    let providers = create_providers(
        &without_keys,
        std::sync::Arc::new(support::MockHttp::default()),
    );
    let zero_ex = providers
        .iter()
        .find(|provider| provider.id() == "0x")
        .unwrap();
    assert!(zero_ex.requires_access_key());
    assert!(zero_ex.supported_chains().is_empty());

    let bebop = providers
        .iter()
        .find(|provider| provider.id() == "bebop")
        .unwrap();
    assert!(!bebop.requires_access_key());
    assert!(bebop.supported_chains().contains(&1));

    let kyber = providers
        .iter()
        .find(|provider| provider.id() == "kyber")
        .unwrap();
    assert!(!kyber.requires_access_key());
    assert!(kyber.supported_chains().contains(&1));

    let odos = providers
        .iter()
        .find(|provider| provider.id() == "odos")
        .unwrap();
    assert!(!odos.requires_access_key());
    assert!(odos.supported_chains().contains(&1));

    let with_optional_key = config_with(&[("BEBOP_API_KEY", "optional-key")]);
    let bebop = create_providers(
        &with_optional_key,
        std::sync::Arc::new(support::MockHttp::default()),
    )
    .into_iter()
    .find(|provider| provider.id() == "bebop")
    .unwrap();
    assert!(!bebop.requires_access_key());
    assert_eq!(
        with_optional_key.provider_keys.get("bebop"),
        Some(&String::from("optional-key"))
    );
    assert!(bebop.supported_chains().contains(&1));
}
