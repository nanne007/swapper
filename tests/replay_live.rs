mod support;

use metamatch_backend::{app::create_app, config::load_config_file};
use serde_json::{Value, json};
use support::request;

/// Exact user request, real adapters and configured RPC. Opt-in; never signs or broadcasts.
#[tokio::test]
#[ignore = "uses configured provider keys and production RPC quota"]
async fn replay_eth_to_usdc() {
    assert_eq!(
        std::env::var("METAMATCH_RUN_LIVE_REPLAY").as_deref(),
        Ok("1")
    );
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter("metamatch_backend=info")
        .with_test_writer()
        .try_init();
    let app = create_app(load_config_file("config.json").expect("invalid runtime config"))
        .await
        .unwrap();
    let taker = std::env::var("METAMATCH_LIVE_TAKER").expect("set a real funded taker address");
    let input = json!({"chainId":1,"sellToken":"0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","buyToken":"0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48","sellAmount":"1000000000000000000","slippageBps":30,"taker":taker});
    let (status, _, body) = request(&app, "POST", "/v1/competitions", Some(input), None).await;
    assert_eq!(status.as_u16(), 200);
    let snapshot: Value = serde_json::from_str(&body).unwrap();
    for quote in snapshot["quotes"].as_array().unwrap() {
        println!(
            "provider={} amount={} simulation={}",
            quote["route"]["provider"], quote["route"]["buyAmount"], quote["simulation"]
        );
    }
    for failure in snapshot["failures"].as_array().unwrap() {
        println!("failure={failure}");
    }
    assert!(
        !snapshot["quotes"].as_array().unwrap().is_empty(),
        "no quote passed live simulation"
    );
}
