use metamatch_backend::{chains::configured_chain, config::Config};
use serde_json::json;

#[test]
fn json_config_reads_chain_router_and_rejects_unknown_fields() {
    let value = json!({
        "chains": {"8453": {
            "rpcUrl": "http://localhost:8545",
            "router": "0x2222222222222222222222222222222222222222"
        }},
        "providerKeys": {"odos": "fixture-key"}
    });
    let config: Config = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        configured_chain(&config, 8453).rpc_url.as_deref(),
        Some("http://localhost:8545")
    );
    assert_eq!(config.provider_keys.get("odos").unwrap(), "fixture-key");
    for invalid in [
        json!({"PORT": 3000}),
        json!({"chains": {"8453": {"holder": "0x1111111111111111111111111111111111111111"}}}),
        json!({"providerKeys": {"misspelledProvider": "fixture-key"}}),
        json!({"port": "3000"}),
        json!({"port": 0}),
        json!({"port": 65536}),
        json!({"host": "not-an-ip"}),
        json!({"competitionTimeoutMs": 99}),
        json!({"competitionTimeoutMs": 30001}),
        json!({"maxActive": 0}),
        json!({"chains": {"0": {"rpcUrl": "http://localhost"}}}),
        json!({"chains": {"1": {"rpcUrl": "file:///tmp/rpc"}}}),
        json!({"chains": {"1": {"router": "0x0000000000000000000000000000000000000000"}}}),
        json!({"chains": {"1": {"router": "not-an-address"}}}),
        json!({"providerKeys": {"odos": " "}}),
        json!({"balanceSlots": {"0": {}}}),
    ] {
        assert!(
            serde_json::from_value::<Config>(invalid.clone()).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn config_rejects_duplicate_struct_fields() {
    assert!(serde_json::from_str::<Config>(r#"{"port":3000,"port":3001}"#).is_err());
    assert!(
        serde_json::from_str::<Config>(
            r#"{"chains":{"1":{"rpcUrl":"http://a","rpcUrl":"http://b"}}}"#
        )
        .is_err()
    );
}

#[test]
fn example_is_valid_and_parser_rejects_trailing_documents() {
    use metamatch_backend::config::load_config;
    let config = load_config(include_str!("../config.example.json")).unwrap();
    assert_eq!(config.port, 3000);
    assert_eq!(config.max_active, 20);
    assert!(config.chains.values().all(|chain| chain.router.is_none()));
    for invalid in ["", "{} {}", "{\"port\":3000,}", "null"] {
        assert!(load_config(invalid).is_err(), "{invalid}");
    }
}
