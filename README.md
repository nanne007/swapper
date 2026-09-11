# MetaMatch Backend v0.3

类似 Matcha Meta 的非托管多聚合器后端：并发询价、真实 EVM 仿真、净到账比较与统一授权执行。当前 canonical runtime 是 Rust；**这是可本地运行和验收的迁移版，不是已审计的主网服务。**

先读 [产品文档](docs/PRODUCT.md)，再读 [架构逻辑](docs/ARCHITECTURE.md)、[技术方案](docs/TECHNICAL.md)、[证据与来源](docs/SOURCES.md)、[验证记录](docs/VERIFICATION.md)。合约设计及部署前检查见 [contracts/README.md](contracts/README.md)。

## 快速启动（最小运行时）

需要 Rust 1.94.1 和 Cargo。无需数据库、API key、RPC、钱包或 Docker 即可启动健康检查；没有服务端配置时，报价会明确返回 unavailable。

```sh
cargo run
```

默认监听 127.0.0.1:3000。修改 Rust 源码后可使用 `cargo run` 重启。服务不会生成 mock 报价，也不会在未配置凭据时访问供应商。

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

从 `.env.example` 创建自己的环境文件；Rust 进程读取环境变量，配置也可由进程环境注入。不要提交真实环境文件。

- `ZERO_EX_API_KEY`、`ONE_INCH_API_KEY`：对应官方服务的凭据和使用额度；Kyber 使用 `KYBER_CLIENT_ID`。
- `ETHEREUM_RPC_URL`：支持 `eth_simulateV1`、`eth_call` state override 的可信 HTTP RPC。普通公共 RPC 可能不支持。
- `CONFIG_PATH`：服务端 JSON 配置。每链可配置 `router`、`rules`、`balanceSlots`；示例默认全部为空，故默认不能执行。

`rules` 格式为 provider 名（`0x`、`1inch`、`kyber`）到数组，每项 `{ "target": "0x…", "spender": "0x…", "selector": "0x12345678" }`。所有地址须先核对相应链上的正式部署和源代码，并在自己的 Router 中配置相同元组；不能把第一次 API 返回的任意地址自动加入白名单。0x 嵌套 Holder 只允许 target=spender=正式 Holder、selector=`0x2213bc0b`。不要授权 Settler。

`balanceSlots` 为经过 fork 验证的 ERC20 balance mapping slot，不是 allowance slot；未配置且匿名地址没有卖币余额时，ERC20 preview 仿真显示 unsupported。用真实 taker 且余额足够不需要该覆盖。资产名单在 `src/config.rs`，新增资产需要代码、仿真和风控验证，不是随意添加 JSON token。

当前只实现 Ethereum 报价、仿真和 build；必须具备已部署 Router、明确路由白名单、真实资金与成功仿真。没有 Router 时只有 direct-preview，不会自动切换到逐家授权。Base、跨链和其他链不属于当前核心范围。

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
```

Rust 当前已覆盖 domain/config、HTTP/provider/RPC fixture、ABI 编码、仿真失败分类、轮询 HTTP 生命周期、build 边界和本地 Anvil E2E。

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
