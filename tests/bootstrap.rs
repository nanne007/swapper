use alloy_primitives::Address;
use alloy_sol_types::SolCall;
use axum::{Json, Router, routing::post};
use metamatch_backend::{
    chains::bootstrap_chains,
    config::load_config,
    error::{ErrorKind, kind},
    execution::allowanceHolderCall,
    rpc::RpcClients,
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const ROUTER: Address = Address::repeat_byte(0x22);
const HOLDER: Address = Address::repeat_byte(0x45);

#[derive(Clone, Copy, Default, Debug)]
enum Fault {
    #[default]
    None,
    Chain,
    RouterCode,
    HolderCode,
    EmptyReturn,
    ShortReturn,
    DirtyReturn,
    ZeroHolder,
    NativeHolder,
    SelfHolder,
    Revert,
    Timeout,
}

struct RpcFixture {
    url: String,
    calls: Arc<Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for RpcFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl RpcFixture {
    async fn start(fault: Fault) -> Self {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let app = Router::new().route("/", post(move |Json(request): Json<Value>| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(request.clone());
                if matches!(fault, Fault::Timeout) { tokio::time::sleep(Duration::from_secs(1)).await; }
                let result = match request["method"].as_str().unwrap() {
                    "eth_chainId" => json!(if matches!(fault, Fault::Chain) { "0x2" } else { "0x1" }),
                    "eth_blockNumber" => json!("0x10"),
                    "eth_getCode" => {
                        assert_eq!(request["params"][1], "0x10");
                        let router = request["params"][0] == json!(ROUTER);
                        json!(if (router && matches!(fault, Fault::RouterCode)) || (!router && matches!(fault, Fault::HolderCode)) { "0x" } else { "0x6000" })
                    },
                    "eth_call" => {
                        assert_eq!(request["params"][1], "0x10");
                        assert_eq!(request["params"][0]["to"], json!(ROUTER));
                        let tx = &request["params"][0];
                        let data = tx.get("input").or_else(|| tx.get("data")).unwrap();
                        assert_eq!(data, &json!(format!("0x{}", hex::encode(allowanceHolderCall {}.abi_encode()))));
                        if matches!(fault, Fault::Revert) { return Json(json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":3,"message":"execution reverted"}})); }
                        let holder = match fault { Fault::ZeroHolder => Address::ZERO, Fault::NativeHolder => metamatch_backend::domain::NATIVE, Fault::SelfHolder => ROUTER, _ => HOLDER };
                        let data = match fault {
                            Fault::EmptyReturn => "0x".to_owned(),
                            Fault::ShortReturn => format!("{holder:#x}"),
                            Fault::DirtyReturn => format!("0x01{}{}", "00".repeat(11), hex::encode(holder)),
                            _ => format!("0x{}{}", "00".repeat(12), hex::encode(holder)),
                        };
                        json!(data)
                    },
                    method => panic!("unexpected RPC method {method}"),
                };
                Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { url, calls, task }
    }
}

#[tokio::test]
async fn bootstrap_reads_holder_and_rejects_invalid_deployments() {
    for fault in [
        Fault::None,
        Fault::Chain,
        Fault::RouterCode,
        Fault::HolderCode,
        Fault::EmptyReturn,
        Fault::ShortReturn,
        Fault::DirtyReturn,
        Fault::ZeroHolder,
        Fault::NativeHolder,
        Fault::SelfHolder,
        Fault::Revert,
        Fault::Timeout,
    ] {
        let server = RpcFixture::start(fault).await;
        let config = load_config(&json!({"competitionTimeoutMs":100,"chains":{"1":{"rpcUrl":server.url,"router":ROUTER}}}).to_string()).unwrap();
        let clients = RpcClients::new(Duration::from_millis(100));
        let result = bootstrap_chains(&config, [1, 8453], &clients).await;
        if matches!(fault, Fault::None) {
            let chains = result.unwrap();
            let deployment = chains[0].router.unwrap();
            assert_eq!(deployment.address, ROUTER);
            assert_eq!(deployment.holder, HOLDER);
            assert!(chains[1].router.is_none());
            assert_eq!(server.calls.lock().unwrap().len(), 5);
        } else {
            let error = result.unwrap_err();
            assert_eq!(
                kind(&error),
                ErrorKind::InvalidConfig,
                "{fault:?}: {error:#}"
            );
            assert!(format!("{error:#}").contains("bootstrap router for chain 1"));
        }
    }
}

#[tokio::test]
async fn bootstrap_requires_supported_chain_and_rpc_but_allows_unconfigured_chains() {
    let clients = RpcClients::new(Duration::from_millis(100));
    let config = load_config("{}").unwrap();
    assert!(
        bootstrap_chains(&config, [1], &clients).await.unwrap()[0]
            .router
            .is_none()
    );
    let config = load_config(&json!({"chains":{"1":{"router":ROUTER}}}).to_string()).unwrap();
    assert!(bootstrap_chains(&config, [1], &clients).await.is_err());
    assert!(bootstrap_chains(&config, [8453], &clients).await.is_err());
}

#[tokio::test]
async fn production_app_bootstraps_before_exposing_router_capability() {
    use tower::ServiceExt;
    let server = RpcFixture::start(Fault::None).await;
    let config =
        load_config(&json!({"chains":{"1":{"rpcUrl":server.url,"router":ROUTER}}}).to_string())
            .unwrap();
    let app = metamatch_backend::app::create_app(config).await.unwrap();
    assert_eq!(server.calls.lock().unwrap().len(), 5);
    let response = app
        .oneshot(
            axum::http::Request::get("/v1/capabilities")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), 16384)
        .await
        .unwrap();
    let capabilities: Value = serde_json::from_slice(&body).unwrap();
    let chains = capabilities["chains"].as_array().unwrap();
    let ethereum = chains.iter().find(|chain| chain["chainId"] == 1).unwrap();
    assert_eq!(ethereum["routerConfigured"], true);
    assert_eq!(ethereum["rpcConfigured"], true);
    assert_eq!(
        chains
            .iter()
            .find(|chain| chain["chainId"] == 8453)
            .unwrap()["routerConfigured"],
        false
    );
    assert!(
        !String::from_utf8(body.to_vec())
            .unwrap()
            .contains(&server.url)
    );
}
