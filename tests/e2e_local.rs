//! Manual local-chain E2E for the Rust runtime.
//!
//! Run with `forge build --root contracts` first, then:
//! `cargo test --test e2e_local -- --ignored --nocapture`.

mod support;

use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::{DynProvider, Provider as AlloyProvider, ProviderBuilder};
use alloy_rpc_types_eth::{BlockId, TransactionInput, TransactionRequest};
use alloy_sol_types::{SolCall, sol};
use async_trait::async_trait;
use metamatch_backend::{
    app::create_app_with_services,
    chains::bootstrap_chains,
    competitions::Services,
    config::{Config, load_config},
    domain::{Input, Route, Tx, parse_address},
    providers::Provider,
    rpc::RpcClients,
    simulation::Simulator,
};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    path::Path,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::{net::TcpListener, sync::oneshot, time::sleep};

sol! {
    function mint(address to, uint256 amount);
    function approve(address spender, uint256 amount) returns (bool);
    function swap(address sell, address buy, uint256 spend, uint256 output, address recipient, uint256 refund);
    function balanceOf(address owner) view returns (uint256);
    function allowance(address owner, address spender) view returns (uint256);
}

struct AnvilRpc {
    provider: DynProvider,
}

impl AnvilRpc {
    fn new(url: &str) -> Result<Self, String> {
        let url = url
            .parse::<reqwest::Url>()
            .map_err(|error| format!("invalid Anvil URL: {error}"))?;
        Ok(Self {
            provider: ProviderBuilder::default().connect_http(url).erased(),
        })
    }

    async fn chain_id(&self) -> Result<u64, String> {
        self.provider
            .get_chain_id()
            .await
            .map_err(|error| format!("rpc eth_chainId: {error}"))
    }

    async fn accounts(&self) -> Result<Vec<Address>, String> {
        self.provider
            .get_accounts()
            .await
            .map_err(|error| format!("rpc eth_accounts: {error}"))
    }

    async fn call_contract(&self, to: Address, data: Bytes) -> Result<Bytes, String> {
        self.provider
            .call(
                TransactionRequest::default()
                    .to(to)
                    .input(TransactionInput::both(data)),
            )
            .latest()
            .await
            .map_err(|error| format!("rpc eth_call: {error}"))
    }

    async fn send_transaction(
        &self,
        from: Address,
        to: Option<Address>,
        data: String,
        value: U256,
    ) -> Result<B256, String> {
        let data = hex::decode(data.strip_prefix("0x").unwrap_or(&data))
            .map_err(|error| format!("invalid transaction data: {error}"))?;
        let mut transaction = TransactionRequest::default()
            .from(from)
            .input(TransactionInput::both(Bytes::from(data)))
            .value(value)
            .gas_limit(0x1e8480);
        if let Some(to) = to {
            transaction = transaction.to(to);
        }
        self.provider
            .raw_request(Cow::Borrowed("eth_sendTransaction"), (transaction,))
            .await
            .map_err(|error| format!("rpc eth_sendTransaction: {error}"))
    }

    async fn wait_receipt(&self, hash: B256) -> Result<Value, String> {
        for _ in 0..100 {
            if let Some(receipt) = self
                .provider
                .get_transaction_receipt(hash)
                .await
                .map_err(|error| format!("rpc eth_getTransactionReceipt: {error}"))?
            {
                return serde_json::to_value(receipt)
                    .map_err(|error| format!("serialize transaction receipt: {error}"));
            }
            sleep(Duration::from_millis(20)).await;
        }
        Err(format!("transaction receipt timeout: {hash:#x}"))
    }
}

struct FixtureProvider {
    router: Address,
    route: Route,
}

#[async_trait]
impl Provider for FixtureProvider {
    fn id(&self) -> &'static str {
        "kyber"
    }

    fn requires_access_key(&self) -> bool {
        false
    }

    fn supported_chains(&self) -> Vec<u64> {
        vec![1]
    }

    async fn quote(
        &self,
        _input: &Input,
        _chain: &metamatch_backend::domain::Chain,
        sender: Address,
    ) -> anyhow::Result<Route> {
        if sender != self.router {
            return Err(anyhow::anyhow!("FIXTURE_SENDER_MISMATCH"));
        }
        let mut route = self.route.clone();
        route.deadline = None;
        Ok(route)
    }
}

fn artifact(path: &str, field: &str) -> Result<String, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("contracts/out")
        .join(path);
    let value: Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?,
    )
    .map_err(|error| format!("parse {}: {error}", path.display()))?;
    value
        .pointer(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing {field} in {}", path.display()))
}

fn hex_bytes(value: impl AsRef<[u8]>) -> String {
    format!("0x{}", hex::encode(value))
}

fn address_word(address: Address) -> String {
    let mut word = [0_u8; 32];
    word[12..].copy_from_slice(address.as_slice());
    hex::encode(word)
}

async fn deploy(
    rpc: &AnvilRpc,
    from: Address,
    bytecode: String,
    constructor: String,
) -> Result<Address, String> {
    let hash = rpc
        .send_transaction(from, None, format!("{bytecode}{constructor}"), U256::ZERO)
        .await?;
    let receipt = rpc.wait_receipt(hash).await?;
    receipt
        .get("contractAddress")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("deployment failed: {receipt}"))
        .and_then(|address| parse_address(address).map_err(|error| error.to_string()))
}

async fn send_call(rpc: &AnvilRpc, from: Address, to: Address, data: String) -> Result<(), String> {
    let hash = rpc
        .send_transaction(from, Some(to), data, U256::ZERO)
        .await?;
    let receipt = rpc.wait_receipt(hash).await?;
    if receipt.get("status").and_then(Value::as_str) != Some("0x1") {
        return Err(format!("call reverted: {receipt}"));
    }
    Ok(())
}

async fn send_tx(rpc: &AnvilRpc, from: Address, transaction: &Value) -> Result<(), String> {
    let to = parse_address(
        transaction
            .get("to")
            .and_then(Value::as_str)
            .ok_or_else(|| String::from("transaction missing to"))?,
    )
    .map_err(|error| error.to_string())?;
    send_call(
        rpc,
        from,
        to,
        transaction
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| String::from("transaction missing data"))?
            .to_owned(),
    )
    .await
}

async fn run_e2e() -> Result<(), String> {
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|error| error.to_string())?;
    let port = probe
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    drop(probe);
    let anvil_bin = std::env::var("ANVIL_BIN").unwrap_or_else(|_| String::from("anvil"));
    let mut anvil = Command::new(anvil_bin)
        .args(["--port", &port.to_string(), "--chain-id", "1", "--silent"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("start anvil: {error}"))?;
    let url = format!("http://127.0.0.1:{port}");
    let rpc = Arc::new(AnvilRpc::new(&url)?);
    let ready = wait_for_anvil(&mut anvil, &rpc).await;
    if let Err(error) = ready {
        let _ = anvil.kill();
        return Err(error);
    }
    let result = run_against_anvil(&rpc, &url).await;
    let _ = anvil.kill();
    let _ = anvil.wait();
    result
}

async fn wait_for_anvil(anvil: &mut Child, rpc: &AnvilRpc) -> Result<(), String> {
    for _ in 0..100 {
        if anvil
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Err(String::from("anvil exited before becoming ready"));
        }
        if rpc.chain_id().await.is_ok() {
            return Ok(());
        }
        sleep(Duration::from_millis(50)).await;
    }
    Err(String::from("anvil startup timeout"))
}

async fn run_against_anvil(rpc: &Arc<AnvilRpc>, url: &str) -> Result<(), String> {
    let account = rpc
        .accounts()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| String::from("anvil returned no account"))?;
    let holder = deploy(
        rpc,
        account,
        artifact(
            "MetaRouter.t.sol/MockAllowanceHolder.json",
            "/bytecode/object",
        )?,
        String::new(),
    )
    .await?;
    let sell = deploy(
        rpc,
        account,
        artifact("MetaRouter.t.sol/MockToken.json", "/bytecode/object")?,
        String::new(),
    )
    .await?;
    let buy = deploy(
        rpc,
        account,
        artifact("MetaRouter.t.sol/MockToken.json", "/bytecode/object")?,
        String::new(),
    )
    .await?;
    let target = deploy(
        rpc,
        account,
        artifact("MetaRouter.t.sol/MockProvider.json", "/bytecode/object")?,
        String::new(),
    )
    .await?;
    let router = deploy(
        rpc,
        account,
        artifact("MetaRouter.sol/MetaRouter.json", "/bytecode/object")?,
        address_word(holder),
    )
    .await?;
    let swap_data = hex_bytes(
        swapCall {
            sell,
            buy,
            spend: U256::from(100),
            output: U256::from(200),
            recipient: router,
            refund: U256::ZERO,
        }
        .abi_encode(),
    );
    send_call(
        rpc,
        account,
        sell,
        hex_bytes(
            mintCall {
                to: account,
                amount: U256::from(1_000_000),
            }
            .abi_encode(),
        ),
    )
    .await?;

    let config: Config =
        load_config(&json!({"chains":{"1":{"rpcUrl":url,"router":router}}}).to_string())
            .map_err(|error| error.to_string())?;
    let clients = RpcClients::new(Duration::from_millis(config.timeout_ms));
    let chain = bootstrap_chains(&config, [1], &clients)
        .await
        .map_err(|error| format!("{error:#}"))?
        .remove(0);
    assert_eq!(chain.router.unwrap().holder, holder);
    let input = Input {
        chain_id: 1,
        sell_token: sell,
        buy_token: buy,
        sell_amount: String::from("100"),
        slippage_bps: 30,
        taker: account,
    };
    let route = Route {
        provider: "kyber",
        buy_amount: String::from("200"),
        min_buy_amount: String::from("199"),
        sell_amount: String::from("100"),
        spender: target,
        tx: Tx {
            to: target,
            data: swap_data.parse().unwrap(),
            value: String::from("0"),
        },
        deadline: None,
    };
    // Resolve layout independently before either wallet enters simulation.
    let slots = Arc::new(metamatch_backend::balance_slots::BalanceSlots::new(
        Default::default(),
    ));
    let simulator = Simulator::new(
        Arc::new(metamatch_backend::rpc::RpcClients::new(
            Duration::from_secs(2),
        )),
        slots.clone(),
    );
    let mut preview_input = input.clone();
    preview_input.taker = Address::repeat_byte(0x77);
    slots
        .resolve(&rpc.provider, chain.id, sell, BlockId::latest())
        .await
        .map_err(|error| format!("standalone balance mapping detection: {error:#}"))?;
    for owner in [Address::repeat_byte(0x77), Address::repeat_byte(0x88)] {
        preview_input.taker = owner;
        let result = support::simulate(&simulator, &preview_input, &chain, &route)
            .await
            .map_err(|error| format!("preview discovery: {error:#}"))?;
        if result.simulation.bought_amount != "200" || result.simulation.funding != "overridden" {
            return Err("unexpected preview discovery simulation result".into());
        }
    }
    for token in [sell, buy] {
        let balance = rpc
            .call_contract(
                token,
                balanceOfCall {
                    owner: preview_input.taker,
                }
                .abi_encode()
                .into(),
            )
            .await?;
        if U256::from_be_slice(&balance) != U256::ZERO {
            return Err("preview state override changed on-chain balance".into());
        }
    }
    let services = Services::new(
        vec![Arc::new(FixtureProvider { router, route })],
        &[chain],
        Arc::new(Simulator::new(
            Arc::new(metamatch_backend::rpc::RpcClients::new(
                Duration::from_secs(2),
            )),
            Arc::new(metamatch_backend::balance_slots::BalanceSlots::new(
                Default::default(),
            )),
        )),
    );
    let app = create_app_with_services(config, services);
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| error.to_string())?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let result = async {
        let http = reqwest::Client::new();
        let base = format!("http://{address}");
        let response = http
            .post(format!("{base}/v1/competitions"))
            .header("content-type", "application/json")
            .body(serde_json::to_string(&input).map_err(|error| error.to_string())?)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(format!("competition status {}", response.status()));
        }
        let result: Value = response.json().await.map_err(|error| error.to_string())?;
        let quote = &result["quotes"][0];
        if quote["simulation"]["boughtAmount"] != "200"
            || !result["failures"].as_array().is_some_and(Vec::is_empty)
            || quote["simulation"]["funding"] != "overridden"
            || !quote["simulation"]["blockContext"]["number"].is_u64()
            || !quote["simulation"]["blockContext"]["timestamp"].is_u64()
        {
            return Err(format!("unexpected quote: {result}"));
        }
        if result.to_string().contains("expiresAt") || result.to_string().contains("accessToken") {
            return Err("retired response fields present".into());
        }
        if quote["approvals"].as_array().map_or(0, Vec::len) != 1 {
            return Err(format!("expected one approval: {quote}"));
        }
        let approval_data = quote["approvals"][0]["data"]
            .as_str()
            .ok_or("approval data missing")?;
        let approval =
            approveCall::abi_decode(&hex::decode(&approval_data[2..]).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        assert_eq!(approval.spender, holder);
        assert_eq!(quote["transaction"]["to"], json!(holder));
        // Execute exactly the payload returned by the first request. No rebuild.
        send_tx(rpc, account, &quote["approvals"][0]).await?;
        send_tx(rpc, account, &quote["transaction"]).await?;
        let balance = rpc
            .call_contract(
                buy,
                Bytes::from(balanceOfCall { owner: account }.abi_encode()),
            )
            .await?;
        if U256::from_be_slice(&balance) != U256::from(200) {
            return Err(format!(
                "unexpected buy balance: 0x{}",
                hex::encode(&balance)
            ));
        }
        let allowance = rpc
            .call_contract(
                sell,
                Bytes::from(
                    allowanceCall {
                        owner: router,
                        spender: target,
                    }
                    .abi_encode(),
                ),
            )
            .await?;
        if U256::from_be_slice(&allowance) != U256::ZERO {
            return Err(format!(
                "router allowance was not cleared: 0x{}",
                hex::encode(&allowance)
            ));
        }
        Ok::<(), String>(())
    }
    .await;

    let _ = shutdown_tx.send(());
    server
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    result
}

#[tokio::test]
#[ignore = "requires local Anvil and contracts/out artifacts"]
async fn local_anvil_runs_rust_http_simulation_approval_and_swap() {
    run_e2e().await.expect("local Rust E2E should pass");
}
