# MetaMatch Backend v1

类似 Matcha Meta 的非托管多链 EVM 聚合器后端：按 provider 能力反向索引链、并发询价、真实 EVM 仿真、同一买入 token 的报价比较与统一授权执行。当前 canonical runtime 是 Rust；**这是 v1 第一阶段实现，不是已审计的主网服务。**

先读 [产品文档](docs/PRODUCT.md)，再读 [架构逻辑](docs/ARCHITECTURE.md)、[Provider 官方接入手册](docs/PROVIDER_INTEGRATION_GUIDE.md)、[技术方案](docs/TECHNICAL.md)、[v1 ADR](docs/decisions/ADR-001-v1-provider-driven-chain-discovery.md)、[证据与来源](docs/SOURCES.md)、[验证记录](docs/VERIFICATION.md)。合约设计及部署前检查见 [contracts/README.md](contracts/README.md)。

## 快速启动（最小运行时）

需要 Rust 1.94.1 和 Cargo。无需数据库、API key、RPC、钱包或 Docker 即可启动健康检查；无 key provider 默认参与，必需 key 缺失的 provider 会明确返回 unavailable。

```sh
cargo run
```

默认监听 127.0.0.1:3000。修改 Rust 源码后可使用 `cargo run` 重启。服务不会生成 mock 报价，也不会访问缺少必需凭据的 provider。

```sh
curl -s http://127.0.0.1:3000/health
curl -s http://127.0.0.1:3000/v1/capabilities
curl -s http://127.0.0.1:3000/v1/competitions \
  -H 'Content-Type: application/json' \
  -d '{"chainId":1,"sellToken":"0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","buyToken":"0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48","sellAmount":"1000000000000000000","slippageBps":30}'
```

创建响应包含 `id` 和 `accessToken`。轮询 `/v1/competitions/{id}`，附 `Authorization: Bearer <accessToken>`，直到 `status` 为 `complete`。最小运行时只提供轮询，不提供 SSE、OpenAPI endpoint 或交易回执代理。

build 请求：`POST /v1/competitions/{id}/quotes/{quoteId}/build`，同样鉴权，JSON 为 `{ "taker": "真实钱包地址", "acceptedMinBuyAmount": "先前接受的最低到账整数" }`。金额不能用小数或 JS Number。若返回 approvals，先逐笔授权并等待回执，再重新 build，最后由钱包签名 swap；不要签署过期报价。API 从不代签或广播。

## Live 配置

Rust 进程读取环境变量；`config.example.json` 为空对象，表示 v1 不使用链/provider/token 业务配置。不要提交真实环境文件。

- 需要 key 的 provider 使用原生环境变量：`ZERO_EX_API_KEY`、`ONE_INCH_API_KEY`、`BARTER_API_KEY`、`ENSO_API_KEY`、`HYPERBLOOM_API_KEY`、`OOGABOOGA_API_KEY`、`OKX_API_KEY`。
- OKX 还必须配置 `OKX_SECRET_KEY`、`OKX_API_PASSPHRASE`；`OKX_PROJECT_ID` 可选。`BEBOP_API_KEY`、`KYBER_CLIENT_ID` 和 `ODOS_API_KEY` 为 optional，配置后只传给对应 adapter，不改变其参与资格。Kyber 公共 legacy gateway、Odos、LiquidSwap、OpenOcean、Velora 当前可免 key 访问；完整接入差异见 [Provider 官方接入手册](docs/PROVIDER_INTEGRATION_GUIDE.md)。
- `RPC_URL_<chainId>`：对应链的可信 HTTP RPC，例如 `RPC_URL_8453`；需要支持 `eth_simulateV1`、`eth_call` state override。Ethereum 也兼容 `ETHEREUM_RPC_URL`。
- 不配置 RPC 时链仍会出现在 capabilities，但仿真会明确返回 unavailable/unsupported。

服务不维护 token 白名单；API 收到的 token 地址经过格式、金额和 route 安全校验后透传给 provider。不得把 provider 返回的任意 target 自动加入 Router allowlist；正式 Router/Holder 仍需按链逐一核对。

`balanceSlots` 和 route rules 不再由 JSON 注入；正式执行配置仍需在后续链/部署 profile 中设计。当前没有 Router 时只有 direct-preview，不会自动切换到逐家授权。

当前 13 个 provider 均已注册真实 HTTP adapter，并通过脱敏 fixture 验证请求参数、响应归一化和 route 安全边界；真实 key、生产 RPC、Router/allowlist 和成功仿真仍需独立 release gate。跨链和非 EVM 不属于 v1。

## 真实 provider smoke test

`tests/providers_live.rs` 为 13 个 provider 各提供一个真实上游 smoke test。它使用生产 adapter 和 `ReqwestClient`，不使用 `MockHttp`，不会签名、广播或提交交易。为避免普通测试意外访问外部服务，这些测试同时需要 `#[ignore]` 和显式环境开关：

```sh
METAMATCH_RUN_LIVE_PROVIDER_TESTS=1 \
  cargo test --test providers_live -- --ignored --nocapture
```

需要 access key 的 provider 从进程环境读取已有的原生 key；缺少必需字段时测试明确 `SKIP`，不会发请求。Bebop、Kyber、LiquidSwap、Odos、OpenOcean、Velora 即使没有 key 也会尝试进入测试。Ethereum、Optimism、Base、Arbitrum 使用公开的 WETH/USDC 默认输入；HyperEVM 使用 WHYPE/USDT0，Berachain 使用 native/HONEY。可通过 `METAMATCH_LIVE_BUY_TOKEN_<chainId>`、`METAMATCH_LIVE_SELL_TOKEN_<chainId>` 和 `METAMATCH_LIVE_SELL_AMOUNT_<chainId>` 覆盖输入，但不会把这些测试默认值引入产品 token registry。`METAMATCH_LIVE_CHAIN_ID` 可让所有 provider 使用同一个替代链，但该链必须在对应 provider 的 `supported_chains()` 中。

每个实际发出请求的 live test 都要求全程 HTTP 2xx，并且生产 adapter 必须返回通过安全校验的归一化 `Route`；只有“服务器返回了错误”不算通过。live 结果会受上游限流、IP 策略、实时流动性和服务变更影响，不属于普通 CI 门禁。OpenOcean 官方公开 API 虽免 key，但 403 表示出口 IP 被安全策略拦截，需要联系 OpenOcean 加白，详见验证文档。

## 测试

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --release
# 另需 Foundry 的 forge 在 PATH 中
forge fmt --root contracts --check
forge build --root contracts --deny-warnings
forge test --root contracts
cargo test --test e2e_local -- --ignored --nocapture
# 显式配置 key/token 后再运行真实 provider smoke；默认不会访问外部服务
# METAMATCH_RUN_LIVE_PROVIDER_TESTS=1 cargo test --test providers_live -- --ignored --nocapture
```

Rust 当前已覆盖 domain/config、HTTP/provider/RPC fixture、ABI 编码、仿真失败分类、轮询 HTTP 生命周期、build 边界和本地 Anvil E2E。生产 `src/` 不包含测试模块；测试统一放在 `tests/`。

## 格式化、Lint 与 AI agent

代码质量入口已经统一到 Cargo 与 Foundry：

```sh
cargo fmt --all       # 格式化 Rust
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --release
forge fmt --root contracts --check
forge build --root contracts --deny-warnings
forge test --root contracts
```

Rust 通过 `rustfmt`、Clippy 和 Cargo 测试保持代码质量；Solidity 不引入第二套 formatter，以 Foundry `forge fmt` 为唯一格式来源。生成的 `target`、Foundry `out/cache` 和本地密钥均被忽略。

仓库根目录的 [AGENTS.md](AGENTS.md) 是 AI coding agent 的项目设置：它定义模块边界、资金安全不变量、允许执行的验证命令、fixture/live 区分、密钥与部署禁区，以及交付前必须报告的未验证边界。任何 agent 修改前都应先读该文件和相关产品/技术文档。

更细的 agent skill 说明见 [docs/AI-AGENT.md](docs/AI-AGENT.md)。仓库内的 `.agents/skills/` 按供应商适配、合约安全、质量门拆分工作流；当前不自动安装外部 plugin，避免在没有明确授权时引入第三方账号、私有数据或部署能力。

GitHub Actions 位于 `.github/workflows/ci.yml`，在每个 push/PR 上执行 Rust 格式、Clippy、测试、release build 和 Foundry 合约质量门。CI 没有生产密钥，也不会部署或广播交易。

`cargo test --test e2e_local -- --ignored --nocapture` 启动并清理独立本地 Anvil，用测试 Token、测试 Provider 与 mock Holder 跑 Rust HTTP → 仿真 → 授权 → 重建 → 实际本地成交；它不是主网 fork，也不证明真实上游供应商可执行。

Dockerfile 已切换为 Rust multi-stage 构建，镜像构建需另行验证；此版本不要求 Docker。任何真实部署前须完成供应商条款/限流评估、真实 AllowanceHolder 主网 fork、RPC 兼容性、合约外部审计、小额人工验收与多签管理。

## 运维边界

当前 Rust runtime 有内存 TTL、竞赛/构建并发上限、请求体上限、超时和敏感错误清洗。不是多租户公网生产配置：没有持久数据库、IP/分布式限流、跨副本事件总线、用户身份系统、历史审计数据库或交易回执代理。反向代理部署需明确可信代理并配置 TLS、访问控制、监控、出站网络策略；不要直接公开收费上游代理。服务器只接受固定供应商域名和服务端配置 RPC，不接受用户传入的目标 URL。
