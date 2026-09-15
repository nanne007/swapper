# MetaMatch Backend v1

类似 Matcha Meta 的非托管多链 EVM 聚合器后端：按 provider 能力反向索引链、并发询价、真实 EVM 仿真、同一买入 token 的报价比较与统一授权执行。当前 canonical runtime 是 Rust；**这是 v1 第一阶段实现，不是已审计的主网服务。**

先读 [产品文档](docs/PRODUCT.md)，再读 [架构逻辑](docs/ARCHITECTURE.md)、[Provider 官方接入手册](docs/PROVIDER_INTEGRATION_GUIDE.md)、[技术方案](docs/TECHNICAL.md)、[v1 ADR](docs/decisions/ADR-001-v1-provider-driven-chain-discovery.md)、[证据与来源](docs/SOURCES.md)、[验证记录](docs/VERIFICATION.md)。合约设计及部署前检查见 [contracts/README.md](contracts/README.md)。

## 快速启动（最小运行时）

需要 Rust 1.94.1 和 Cargo。无需数据库、API key、RPC、钱包或 Docker 即可启动健康检查；无 key provider 默认参与，必需 key 缺失的 provider 会明确返回 unavailable。

```sh
cp config.example.json config.json
cargo run
# 可选：cargo run -- /absolute/path/config.json
```

启动默认读取当前工作目录的 `config.json`，也可传入单个文件路径参数。不再读取 `.env` 或应用环境变量；文件缺失、非法配置或 Router bootstrap 失败时退出。默认监听 127.0.0.1:3000。配置示例不含 RPC、凭据或 Router，可用于 health/capabilities；配置变更后重启。完整字段、迁移表和校验规则见 [JSON 配置说明](docs/CONFIGURATION.md)。

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

`POST /v1/competitions` 在竞赛总超时内直接返回 HTTP `200` 和最终结果，不需要轮询、Bearer competition token 或第二次 build。`taker` 必填，且同时是收款人。每个 provider 都独立执行 `route → validate → simulate`，某家 route 一完成即可在 `latest` 上开始完整仿真，无需等待其他 provider。所有 provider pipeline 共用本次竞赛的一个绝对截止时间；单家失败或慢 route 不阻塞其他家的 simulation。

响应包含 `quotes` 和 `failures` 两个数组。`Quote` 只包含必填的 `route`、`simulation: SimulationSuccess`、`approvals[]`、`transaction` 和 `latencyMs`，按 `simulation.boughtAmount`（完整交易序列在模拟中的钱包买入 token 余额增量）降序排列。provider 和原始报价分别在 `route.provider`、`route.buyAmount`，最低到账在 `route.minBuyAmount`。Quote 和成功 simulation 不再携带 status/error；第一条 quote 即最高模拟到账量。失败 provider 的状态、错误码及仿真失败分类只在 `failures` 中；全部失败时 `quotes: []`。Gas 单独报告，不参与金额排序。

`route.tx` 是归一化的 provider 路由交易；钱包应执行 `quote.approvals` 和 `quote.transaction`，后者才是完成统一 Holder/Router 仿真的最终交易。

用户选择一条结果，按顺序执行 approvals 并等待确认，再执行返回的同一笔 swap。ERC20 卖出总是返回一笔 `approve(Holder, sellAmount)`，native 卖出没有 token approval；服务不预读钱包当前 allowance。simulation 总是覆盖 taker 的卖出资产，并把 native 余额设为仅供仿真的最大值，`funding` 固定为 `overridden`；成功结果证明这组交易在该假定资金和该 provider 的 latest 模拟区块中可执行，不证明钱包当前余额充足。simulation 不预先查询 block context 或 gas price，调用 `eth_simulateV1` 时也不发送 `gasPrice`；因此 `gasFeeWei` 固定为 `null`。`simulation.blockContext` 直接来自模拟响应，包含 `number`（JSON u64）、`hash`、`timestamp`（Unix 秒），`simulatedTimestamp` 与该模拟区块时间一致。API 不返回 `expiresAt`，也不设置人工报价 TTL；调用方发送前应确认钱包余额并自行决定是否重新竞赛。

竞赛总预算用 `competitionTimeoutMs` 配置（默认 6000，允许 100–30000）。旧 `PROVIDER_TIMEOUT_MS` 和 `QUOTE_TTL_MS` 已移除；请迁移为 JSON 配置字段。金额使用十进制字符串，不能用小数或 JS Number。API 从不代签或广播。

## 配置、Router 与 Holder

应用配置以 [config.example.json](config.example.json) 为模板，完整字段、旧环境变量迁移表、启动校验与 Docker 使用方法见 [CONFIGURATION.md](docs/CONFIGURATION.md)。真实 `config.json` 已排除出 Git 和 Docker context；本次不会读取、转换或删除已有 `.env`。

- 每链使用 `chains.<chainId>.rpcUrl` 和 `chains.<chainId>.router`；显式 RPC 优先于 `alchemyApiKey` endpoint fallback。
- provider 凭据使用 `providerKeys`，OKX 补充字段使用 `okxSecretKey / okxPassphrase / okxProjectId`。必需/可选凭据的参与规则不变。
- `balanceSlots` 是直接嵌套的 JSON 对象，不再是 JSON 环境变量；缺失时沿用假 owner 的 `eth_createAccessList` 探测。
- 配置 Router 后，服务在监听前检查 RPC chain ID、Router 代码、`allowanceHolder()` 返回值和 Holder 代码。Holder 只从 Router 读取，不可配置，也没有硬编码 fallback；钱包 approval 和最外层交易使用该 Holder。
- 未配置 Router 的链保留在 capabilities，竞赛返回 `ROUTER_NOT_CONFIGURED`。已配置但无法验证的 Router 导致启动失败，不降级运行。

Rust 已匹配当前 `execute(sellToken, buyToken, receiver, sellAmount, minBuyAmount, deadline, spender, target, value, data)` ABI；API 保持 `taker`，映射为合约 sender/receiver。配置中不提供 chain/provider/token 白名单。

当前 Solidity 是无管理员、无暂停、无路由白名单和无 recover 的轻量 Router；只保护本次 sell/buy/native 交换边界，不保证无关暂存 token 的安全。详细行为见 [合约文档](contracts/README.md)。

Rust 与合约均采用 permissionless 路由策略：不维护 target/spender/selector 白名单，也不再返回 `ROUTE_NOT_ALLOWLISTED`。adapter 原生约束、金额/value/calldata 校验和完整 Holder/Router 仿真仍为必要条件；API 不接受任意 calldata。真实 provider/RPC、正式部署和主网执行仍需独立验收。

Router/Holder code 只在启动 bootstrap 验证，请求阶段不重复读取 code 或 Holder allowance。每个 provider 独立解析/复用 ERC20 卖出 token mapping base 并构造自己的 simulation；同一 token 的自动探测仍由 `BalanceSlots` 缓存合并。已配置 mapping base 时，每家只需一次 `eth_simulateV1` RPC。内部 calldata 使用 Bytes，公开交易仍为 hex data 字符串。性能回归与边界见 [验证记录](docs/VERIFICATION.md)。

## 真实 provider smoke test

`tests/providers_live.rs` 为 13 个 provider 各提供一个真实上游 smoke test。它使用生产 adapter 和 `ReqwestClient`，不使用 `MockHttp`，不会签名、广播或提交交易。为避免普通测试意外访问外部服务，这些测试同时需要 `#[ignore]` 和显式环境开关：

```sh
METAMATCH_RUN_LIVE_PROVIDER_TESTS=1 \
  cargo test --test providers_live -- --ignored --nocapture
```

需要 access key 的 provider 从 `config.json` 读取凭据；缺少必需字段时测试明确 `SKIP`，不会发请求。Bebop、Kyber、LiquidSwap、Odos、OpenOcean、Velora 即使没有 key 也会尝试进入测试。Ethereum、Optimism、Base、Arbitrum 使用公开的 WETH/USDC 默认输入；HyperEVM 使用 WHYPE/USDT0，Berachain 使用 native/HONEY。可通过 `METAMATCH_LIVE_BUY_TOKEN_<chainId>`、`METAMATCH_LIVE_SELL_TOKEN_<chainId>` 和 `METAMATCH_LIVE_SELL_AMOUNT_<chainId>` 覆盖输入，但不会把这些测试默认值引入产品 token registry。`METAMATCH_LIVE_CHAIN_ID` 可让所有 provider 使用同一个替代链，但该链必须在对应 provider 的 `supported_chains()` 中。

每个实际发出请求的 live test 都要求全程 HTTP 2xx，并且生产 adapter 必须返回通过安全校验的归一化 `Route`；只有“服务器返回了错误”不算通过。live 结果会受上游限流、IP 策略、实时流动性和服务变更影响，不属于普通 CI 门禁。OpenOcean 官方公开 API 虽免 key，但 403 表示出口 IP 被安全策略拦截，需要联系 OpenOcean 加白，详见验证文档。

### 重放完整 ETH → USDC 竞赛

`tests/replay_live.rs` 从 `config.json` 加载应用设置，要求通过 `METAMATCH_LIVE_TAKER` 显式提供真实 taker，经过 Axum、生产 adapter 和配置的 RPC。仅在显式启用时运行，会消耗上游配额，不签名、不广播：

```sh
METAMATCH_RUN_LIVE_REPLAY=1 cargo test --test replay_live -- --ignored --nocapture
```

测试要求真实 block context 且至少一家仿真成功；这只证明竞赛整体可用，不表示每家 provider 都成功。逐家失败仍输出并记录，单家接入验收使用上面的严格 provider smoke。2026-09-14 重放有 5 家真实 preview 仿真成功，Odos 530、OpenOcean 403；完整原因和瞬时报价见 [验证记录](docs/VERIFICATION.md#真实请求重放与错误系统2026-09-14)。未签名或广播，也不证明真实 taker 余额足够。

## 测试

```sh
# 需要 Rust 工具链及 Foundry 的 forge/anvil 在 PATH 中
sh scripts/check.sh
```

完整命令顺序只维护在 [scripts/check.sh](scripts/check.sh)，本地与 CI 共用：Rust fmt、Clippy、测试、release build、Foundry fmt/build/test、隔离 Anvil E2E。任一步失败即退出；不会开启生产 provider/RPC 测试。依赖已缓存时可使用 `CARGO_NET_OFFLINE=true sh scripts/check.sh`，这只是 Cargo 的离线选项，不是应用配置。

Rust 当前已覆盖 domain/config、HTTP/provider/RPC fixture、ABI 编码、仿真失败分类、单请求 HTTP 竞赛、总超时/取消、真实 taker 执行边界和本地 Anvil E2E。生产 `src/` 不包含测试模块；测试统一放在 `tests/`。

## 格式化、Lint 与 AI agent

迭代时可单独格式化，交付前运行上述统一质量门：

```sh
cargo fmt --all       # 格式化 Rust
forge fmt --root contracts
```

Rust 通过 `rustfmt`、Clippy 和 Cargo 测试保持代码质量；Solidity 不引入第二套 formatter，以 Foundry `forge fmt` 为唯一格式来源。生成的 `target`、Foundry `out/cache` 和本地密钥均被忽略。

仓库根目录的 [AGENTS.md](AGENTS.md) 是 AI coding agent 的项目设置：它定义模块边界、资金安全不变量、允许执行的验证命令、fixture/live 区分、密钥与部署禁区，以及交付前必须报告的未验证边界。任何 agent 修改前都应先读该文件和相关产品/技术文档。

更细的 agent skill 说明见 [docs/AI-AGENT.md](docs/AI-AGENT.md)。仓库内的 `.agents/skills/` 按供应商适配、合约安全、质量门拆分工作流；当前不自动安装外部 plugin，避免在没有明确授权时引入第三方账号、私有数据或部署能力。

GitHub Actions 位于 `.github/workflows/ci.yml`，对 main 分支的 push/PR 调用同一个 `scripts/check.sh`。CI 没有生产密钥，不会部署或广播真实网络交易。

`cargo test --test e2e_local -- --ignored --nocapture` 可单独启动并清理独立本地 Anvil，用测试 Token、测试 Provider 与 mock Holder 跑 Rust HTTP → 仿真 → 执行返回的同一组授权/swap → 实际本地成交；没有二次 build，不是主网 fork，也不证明真实上游供应商可执行。

Dockerfile 已切换为 Rust multi-stage 构建，镜像构建需另行验证；此版本不要求 Docker。任何真实部署前须完成供应商条款/限流评估、真实 AllowanceHolder 主网 fork、RPC 兼容性、合约外部审计、小额人工验收与部署权限核验。

## 运维边界

当前 Rust runtime 不保存竞赛结果，有竞赛并发上限、请求体上限、总超时和公开错误投影。不是多租户公网生产配置：没有持久数据库、IP/分布式限流、跨副本事件总线、用户身份系统、历史审计数据库或交易回执代理。反向代理部署需明确可信代理并配置 TLS、访问控制、监控、出站网络策略；不要直接公开收费上游代理。服务器只接受固定供应商域名和服务端配置 RPC，不接受用户传入的目标 URL。
