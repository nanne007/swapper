# MetaMatch Backend v1

类似 Matcha Meta 的非托管多链 EVM 聚合器后端：按 provider 能力反向索引链、并发询价、真实 EVM 仿真、同一买入 token 的报价比较与统一授权执行。当前 canonical runtime 是 Rust；**这是 v1 第一阶段实现，不是已审计的主网服务。**

先读 [产品文档](docs/PRODUCT.md)，再读 [架构逻辑](docs/ARCHITECTURE.md)、[Provider 官方接入手册](docs/PROVIDER_INTEGRATION_GUIDE.md)、[技术方案](docs/TECHNICAL.md)、[v1 ADR](docs/decisions/ADR-001-v1-provider-driven-chain-discovery.md)、[证据与来源](docs/SOURCES.md)、[验证记录](docs/VERIFICATION.md)。合约设计及部署前检查见 [contracts/README.md](contracts/README.md)。

## 快速启动（最小运行时）

需要 Rust 1.94.1 和 Cargo。无需数据库、API key、RPC、钱包或 Docker 即可启动健康检查；无 key provider 默认参与，必需 key 缺失的 provider 会明确返回 unavailable。

```sh
cargo run
```

启动时会从当前目录或父目录读取可选 `.env`，已有的 shell/部署环境变量优先；文件不存在时继续启动，文件存在但不可读或格式错误时直接失败。默认监听 127.0.0.1:3000。修改 Rust 源码后可使用 `cargo run` 重启。服务不会生成 mock 报价，也不会访问缺少必需凭据的 provider。

排障时可以启用内部 backtrace；日志写入 stderr，默认过滤为 `metamatch_backend=info`：

```sh
RUST_LOG=metamatch_backend=info RUST_LIB_BACKTRACE=1 cargo run
```

内部直接使用 `anyhow::Result<T>`，通过 `.context(ErrorKind::…)` 附加分类；原始错误、上下文和 backtrace 交给 anyhow。失败边界直接 `tracing::warn!(error = ?error, "操作失败")`，启动入口返回 `anyhow::Result<()>`，不再封装 `Fault` 或专用日志层。当前不实现日志脱敏，内部日志可能包含上游错误详情；公开 `ApiError` 仍只返回稳定错误码，不包含内部原因链。

```sh
curl -s http://127.0.0.1:3000/health
curl -s http://127.0.0.1:3000/v1/capabilities
curl -s http://127.0.0.1:3000/v1/competitions \
  -H 'Content-Type: application/json' \
  -d '{"chainId":1,"sellToken":"0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","buyToken":"0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48","sellAmount":"1000000000000000000","slippageBps":30,"taker":"0x你的真实钱包地址"}'
```

`POST /v1/competitions` 在竞赛总超时内直接返回 HTTP `200` 和最终结果，不需要轮询、Bearer competition token 或第二次 build。`taker` 必填，且同时是收款人。每个 provider 独立执行 quote → route 校验 → 构建交易 → 完整仿真；所有流程共用一个截止时间，超时只影响未完成的结果。

响应包含 `quotes` 和 `failures` 两个数组。`Quote` 只包含必填的 `route`、`simulation: SimulationSuccess`、`approvals[]`、`transaction` 和 `latencyMs`，按 `simulation.boughtAmount`（完整交易序列在模拟中的钱包买入 token 余额增量）降序排列。provider 和原始报价分别在 `route.provider`、`route.buyAmount`，最低到账在 `route.minBuyAmount`。Quote 和成功 simulation 不再携带 status/error；第一条 quote 即最高模拟到账量。失败 provider 的状态、错误码及仿真失败分类只在 `failures` 中；全部失败时 `quotes: []`。Gas 单独报告，不参与金额排序。

`route.tx` 是归一化的 provider 路由交易；钱包应执行 `quote.approvals` 和 `quote.transaction`，后者才是完成统一 Holder/Router 仿真的最终交易。

用户选择一条结果，按顺序执行必要的 approvals 并等待确认，再执行返回的同一笔 swap。simulation 总是把 taker 的卖出资产和 native gas 资金覆盖到本次请求所需数额，`funding` 固定为 `overridden`；成功结果证明这组交易在该假定资金和固定区块上下文中可执行，不证明钱包当前余额充足。API 不返回 `expiresAt`，也不设置人工报价 TTL。`simulation.blockContext` 包含基础区块 `number`（十六进制字符串）、`hash`、`timestamp`（Unix 秒）；`simulatedTimestamp` 是实际用于模拟执行的时间。调用方发送前应确认钱包余额，并自行决定是否重新仿真；重新竞赛会重新获取 provider 报价，最低到账变化需要调用方重新确认。provider 原生签名期限和 calldata 内的 deadline 仍按链上规则生效。

竞赛总预算用 `COMPETITION_TIMEOUT_MS` 配置（默认 6000，允许 100–30000）。旧 `PROVIDER_TIMEOUT_MS` 和 `QUOTE_TTL_MS` 已移除；请更新部署环境中的配置名称。金额使用十进制字符串，不能用小数或 JS Number。API 从不代签或广播。

## Live 配置

Rust 进程读取环境变量和本地 `.env`，配置项见 `.env.example`；v1 不使用链/provider/token 业务配置文件。不要提交真实环境文件。

- 需要 key 的 provider 使用原生环境变量：`ZERO_EX_API_KEY`、`ONE_INCH_API_KEY`、`BARTER_API_KEY`、`ENSO_API_KEY`、`HYPERBLOOM_API_KEY`、`OOGABOOGA_API_KEY`、`OKX_API_KEY`。
- OKX 还必须配置 `OKX_SECRET_KEY`、`OKX_API_PASSPHRASE`；`OKX_PROJECT_ID` 可选。`BEBOP_API_KEY`、`KYBER_CLIENT_ID` 和 `ODOS_API_KEY` 为 optional，配置后只传给对应 adapter，不改变其参与资格。Kyber 公共 legacy gateway、Odos、LiquidSwap、OpenOcean、Velora 当前可免 key 访问；完整接入差异见 [Provider 官方接入手册](docs/PROVIDER_INTEGRATION_GUIDE.md)。
- `RPC_URL_<chainId>`：对应链的可信 HTTP RPC，例如 `RPC_URL_8453`；需要支持带 state override 的 `eth_simulateV1`。未配置 ERC20 mapping base 时还需要 `eth_createAccessList`。Ethereum 也兼容 `ETHEREUM_RPC_URL`。
- `ALCHEMY_API_KEY`：当某条链没有显式 `RPC_URL_<chainId>`（Ethereum 也没有旧别名）时，按官方 Alchemy network endpoint 自动补齐 RPC。显式 RPC 始终优先；key 只在服务端使用，不得写入日志或提交到仓库。
- 不配置 RPC 时链仍会出现在 capabilities，但仿真会明确返回 unavailable/unsupported。

Alchemy 官方列出 endpoint 不代表每条链都支持本服务依赖的全部仿真方法；`eth_simulateV1`、state override 和按需 `eth_createAccessList` 仍由运行时 RPC 检查，方法缺失不会被当成成功。

服务不维护 token 白名单；API 收到的 token 地址经过格式、金额和 route 安全校验后透传给 provider。MetaRouter 要求管理员登记 `(target, spender, selector)` 白名单；不得把 provider 返回的任意目标自动加白。合约结构检查、精确授权、最低到账和仿真继续生效，正式 Router/Holder 仍需按链逐一核对。

`BALANCE_SLOTS` 是可选的服务端 JSON 环境变量，格式为 `{"1":{"0x1111111111111111111111111111111111111111":"0x0"}}`（示例地址，非真实 token 配置）：chain ID → token 地址 → uint256 十六进制 mapping 基础槽位。`BalanceSlots::resolve(rpc, chainId, token, block)` 只返回 base，优先读配置和进程缓存；缺失时对两个固定假地址并行调用 `eth_createAccessList`，交叉匹配 token 实际访问的 storage key 后缓存。缓存键只有 chain ID 和 token，与用户地址无关。simulation 使用 base 计算 taker 的 storage key，并直接把卖出 token 和 native balance 写入 state override；不读取真实余额，也不额外验证 override。自动识别范围为 base `0..1023`，详细边界见 [技术方案](docs/TECHNICAL.md#61-独立-balanceslots)。

`Rule`、`Provider::rules()` 和 `ROUTE_NOT_ALLOWLISTED` 已恢复；生产 provider 默认 rules 仍为空，经审核的链专属规则与链上登记尚需补齐。生产链的 `router` 目前仍固定为 `None`，正式地址接入尚需实现；当前返回 `unavailable / ROUTER_NOT_CONFIGURED`，不会自动切换到逐家授权。

当前 13 个 provider 均已注册真实 HTTP adapter，并通过脱敏 fixture 验证请求参数、响应归一化和 route 安全边界；真实 key、生产 RPC、Router/allowlist 和成功仿真仍需独立 release gate。跨链和非 EVM 不属于 v1。

## MetaRouter 管理

- 路由通过 owner-only `setAllowed(target, spender, selector, enabled)` 登记或撤销，默认拒绝；直接 ERC20 target 仍被禁止，防止通过 transfer/approve 移走第三种暂存 token。
- 当前 `owner` 可以调用 `recoverToken(token, recipient, amount)` 提取 Router 中的 ERC20；native 使用 `router.NATIVE()`，amount 为 base units/wei。暂停期间可提取，recover 与 swap 共用重入锁。
- 管理员转移采用 `transferOwnership(newOwner)` → 新管理员调用 `acceptOwnership()`。接受前旧管理员保留权限，接受后立即失权。
- recover 仅转出 Router 自有余额，不拉取钱包资产。暂存资产可被管理员提取，Router 不应作为存款地址。

完整参数、错误条件、事件和 ABI/部署迁移说明见 [MetaRouter 合约文档](contracts/README.md)。本次没有部署；非代理旧合约不能原地更新。

## 真实 provider smoke test

`tests/providers_live.rs` 为 13 个 provider 各提供一个真实上游 smoke test。它使用生产 adapter 和 `ReqwestClient`，不使用 `MockHttp`，不会签名、广播或提交交易。为避免普通测试意外访问外部服务，这些测试同时需要 `#[ignore]` 和显式环境开关：

```sh
METAMATCH_RUN_LIVE_PROVIDER_TESTS=1 \
  cargo test --test providers_live -- --ignored --nocapture
```

需要 access key 的 provider 从进程环境读取已有的原生 key；缺少必需字段时测试明确 `SKIP`，不会发请求。Bebop、Kyber、LiquidSwap、Odos、OpenOcean、Velora 即使没有 key 也会尝试进入测试。Ethereum、Optimism、Base、Arbitrum 使用公开的 WETH/USDC 默认输入；HyperEVM 使用 WHYPE/USDT0，Berachain 使用 native/HONEY。可通过 `METAMATCH_LIVE_BUY_TOKEN_<chainId>`、`METAMATCH_LIVE_SELL_TOKEN_<chainId>` 和 `METAMATCH_LIVE_SELL_AMOUNT_<chainId>` 覆盖输入，但不会把这些测试默认值引入产品 token registry。`METAMATCH_LIVE_CHAIN_ID` 可让所有 provider 使用同一个替代链，但该链必须在对应 provider 的 `supported_chains()` 中。

每个实际发出请求的 live test 都要求全程 HTTP 2xx，并且生产 adapter 必须返回通过安全校验的归一化 `Route`；只有“服务器返回了错误”不算通过。live 结果会受上游限流、IP 策略、实时流动性和服务变更影响，不属于普通 CI 门禁。OpenOcean 官方公开 API 虽免 key，但 403 表示出口 IP 被安全策略拦截，需要联系 OpenOcean 加白，详见验证文档。

### 重放完整 ETH → USDC 竞赛

`tests/replay_live.rs` 使用上面同一个无 taker 的 1 ETH → USDC 请求，经过真实 Axum 路由、生产 provider adapter 和配置的 RPC。它自动将可选 `.env` 解析到局部配置 map，进程环境变量优先，不修改测试进程的全局环境。需要显式启用，运行会消耗上游配额：

```sh
METAMATCH_RUN_LIVE_REPLAY=1 cargo test --test replay_live -- --ignored --nocapture
```

测试要求真实 block context 且至少一家仿真成功；这只证明竞赛整体可用，不表示每家 provider 都成功。逐家失败仍输出并记录，单家接入验收使用上面的严格 provider smoke。2026-09-14 重放有 5 家真实 preview 仿真成功，Odos 530、OpenOcean 403；完整原因和瞬时报价见 [验证记录](docs/VERIFICATION.md#真实请求重放与错误系统2026-09-14)。未签名或广播，也不证明真实 taker 余额足够。

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

Rust 当前已覆盖 domain/config、HTTP/provider/RPC fixture、ABI 编码、仿真失败分类、单请求 HTTP 竞赛、总超时/取消、真实 taker 执行边界和本地 Anvil E2E。生产 `src/` 不包含测试模块；测试统一放在 `tests/`。

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

当前 Rust runtime 不保存竞赛结果，有竞赛并发上限、请求体上限、总超时和公开错误投影。不是多租户公网生产配置：没有持久数据库、IP/分布式限流、跨副本事件总线、用户身份系统、历史审计数据库或交易回执代理。反向代理部署需明确可信代理并配置 TLS、访问控制、监控、出站网络策略；不要直接公开收费上游代理。服务器只接受固定供应商域名和服务端配置 RPC，不接受用户传入的目标 URL。
