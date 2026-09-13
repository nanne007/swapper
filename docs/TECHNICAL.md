# MetaMatch 技术实现方案 v1

日期：2026-09-13。本文与 `PRODUCT.md` v1 同步，是 review 代码时的技术基准。

## 1. 设计结论

核心不是“配置有哪些链”，而是“provider 当前返回哪些可参与链”。启动时创建固定 provider 集合，用每个 provider 的 `supported_chains` 返回值生成一次反向索引：

```text
provider -> supported chain IDs
                    │
                    ▼
chain ID -> eligible provider IDs
                    │
                    ▼
competition(chain ID) -> only these providers
```

没有 access key 的必需凭据 provider 不出现在索引里，因此不会发起请求；免 key provider 直接进入索引。provider 请求失败是局部失败，不改变其他 provider 的结果。

## 2. 模块职责

| 模块 | v1 职责 |
| --- | --- |
| `src/chains.rs` | Matcha 支持链并集、chain ID/name/slug、按 chain ID 读取 RPC 环境变量 |
| `src/domain.rs` | 输入、金额、route、quote 和排序；不持有 provider 身份或能力矩阵 |
| `src/config.rs` | 服务参数、RPC 环境变量和各 provider 原生 key；不解析链/token/provider policy JSON |
| `src/providers/mod.rs` | `Provider` trait、provider registry、反向索引、共享 DTO/helper 和 route 归一化 |
| `src/providers/*.rs` | 每家 provider 的独立 endpoint、认证、DTO 使用和 route 适配 |
| `src/competitions.rs` | 由反向索引选 provider，并发 quote、TTL/capacity、polling、重新 quote/build |
| `src/rpc.rs` | Alloy 2.x typed EVM RPC、parent block 和 gas context |
| `src/simulation.rs` | 固定 block 顺序仿真、余额/授权/到账/gas/reorg 检查 |
| `src/execution.rs` | route 边界、ABI、AllowanceHolder/MetaRouter transaction 编码 |
| `src/app.rs` | Axum endpoints、typed request/response、请求体限制、错误 envelope、rejection/header 处理、capabilities |

依赖方向：`app -> competitions -> {providers, rpc, simulation, execution, domain, config}`；provider 不调用 competition，domain 不依赖网络。

## 3. 配置模型

`Config` 只有服务运行参数、catalog 生成的 `chains` 和 `provider_keys`。`chains` 不是用户配置结果，而是静态能力矩阵的运行时视图；每个 chain 的 RPC 由 `RPC_URL_<chainId>` 注入。默认没有 RPC、Router 或 balance slot，所以服务可以启动但不能声称真实仿真/可执行。

access key 不建通用 credential trait。每个适配器持有自己的原生 credential，并在自己的 `supported_chains` 中决定当前实例可用的链；注册表只消费链集合，不重复判断凭证。OKX 的 key/secret/passphrase 必需、project ID 可选；Bebop Bearer、Kyber client ID、Odos key 是 optional。官方公开 endpoint 的 optional 字段不改变参与资格；取得正式 Pro/Enterprise 接入契约后再在对应 adapter 内实现新的 host/认证。

## 4. Provider 接口和注册表

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;
    fn requires_access_key(&self) -> bool;
    fn supported_chains(&self) -> Vec<u64>;
    fn rules(&self, chain_id: u64) -> Vec<Rule>;
    async fn quote(&self, input: &Input, chain: &Chain, sender: Address)
        -> Result<Route, Fault>;
}
```

`supported_chains()` 表示 provider 实例当前实际可参与的链：必须凭证缺失时返回空集合，免 key 或 optional key provider 返回自身支持矩阵。`ProviderRegistry::new` 在启动时遍历 provider 返回的链集合，并与 catalog 求交集，保存：

```rust
HashMap<u64, Vec<&'static str>>
```

竞赛只从 `registry.for_chain(input.chain_id)` 取得 provider；build 通过字符串 ID 回查同一个 registry。不能在 `competitions.rs` 再遍历“全部 provider”或重新判断 key，否则会破坏单一索引语义。provider ID 是固定字符串，不引入 `ProviderId` enum。

每个 `src/providers/*.rs` 是自身 ID、链矩阵、凭证要求和 rules 的代码事实来源；`domain.rs` 不再列举 provider。后续新增 provider 的最小步骤是：新增独立 adapter、实现上述 provider 元数据、注册原生 key、增加 DTO fixture 和 capability/index 测试。

## 5. Quote 生命周期

1. HTTP 层 `parse_input` 验证 JSON 字段、地址格式、正整数金额、滑点、taker 保留地址；不查 token registry。
2. `Competitions::create` 只按 chain ID 查 catalog，并建立带容量和 TTL 的内存状态。
3. 异步任务读取 context，然后从反向索引获取 eligible providers，并发请求。
4. 每个 route 经过 provider rules、sell amount、min amount、transaction value、calldata、expiry 和统一执行边界校验。
5. 有 context 时进入 simulation；provider 失败、超时、无 RPC 或待实现 adapter 都只写入自身 quote 状态。
6. polling 使用 `rank`，只把未过期、仿真成功且有 `quotedAmount` 的 quote 当作推荐候选；同一请求中的 buy token 已固定，直接比较整数输出。

token 地址透传意味着 v1 不维护 decimals/price registry，也不调用额外价格源；报价排序不扣除无法可靠归一化的 gas/token 价格。

## 6. Build 与资金边界

build 重新获取 context 和 route，不复用 quote 阶段 calldata；检查真实 taker、quote expiry、用户接受底价、provider ID、目标地址、spender、selector、sell amount 和 native value。若没有 Router，返回 `ROUTER_NOT_CONFIGURED`，不自动退化成逐 provider 的未保护交易。

仿真继续保持以下不变量：固定 parent block 和 block hash 二次确认；ERC-20 preview 余额不足时只有经过验证的 balance slot override 才能继续；approval 返回值必须为 true；实际到账必须达到 minimum；gas 余额必须足够；`eth_simulateV1` 不支持时是 `unsupported`，不是成功。

正式 transaction 仍是：

```text
user wallet -> AllowanceHolder.exec -> MetaRouter.execute -> provider target
```

Router 的 route allowlist、临时授权、余额增量、退款、pause 和 reentrancy 由 `contracts/src/MetaRouter.sol` 保证；Rust 只编码并验证边界，不替代链上保护。

## 7. Provider adapter 分层

公共层只做 HTTP timeout/响应上限/JSON 解码，adapter 自己负责：

- endpoint、chain path、认证 header 和 provider 参数命名；
- provider 原始 amount/output/transaction 字段解析；
- spender/target/value/transaction data 的 provider-specific 约束；
- 转换为统一 `Route`。

禁止把所有 provider 压成一个“万能 quote 请求”：各家对 key、chain slug、slippage、spender、RFQ、gasless、Permit 和 calldata 语义不同；公共抽象只保留稳定的 `Provider` 接口，避免错误复用。

当前 13 个 provider 均已完成 adapter。每家 adapter 位于独立的 `src/providers/*.rs` 文件；测试统一位于 `tests/`。逐家 endpoint、认证、单位、native marker、spender/target 和已知限制见 [PROVIDER_INTEGRATION_GUIDE.md](PROVIDER_INTEGRATION_GUIDE.md)。各 adapter 必须以官方 API 文档和脱敏 fixture 固化 response schema，不能用任意字段猜测 target 或 spender；真实供应商行为不由 fixture 证明。

## 8. Axum API 边界

`src/app.rs` 使用 Axum `State` 注入应用状态；写请求使用 typed `Json<CreateCompetitionRequest>`/`Json<BuildRequest>`，路径使用 typed `Path`，不在 handler 中接收 `serde_json::Value` 作为请求协议。`axum-extra` 的 `TypedHeader` 读取 Bearer Authorization，`WithRejection` 将 JSON、path 和 header rejection 统一转换为当前 API error envelope。

Router 同时配置 body limit、404/405 fallback 和 `Cache-Control: no-store` response layer。无效凭据返回 401 和 `WWW-Authenticate: Bearer`；未知字段和格式错误不会把 serde 内部错误文本暴露给调用方。`tests/api.rs` 固化这些 HTTP 可观察行为。

## 9. 验证顺序

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --release
forge fmt --root contracts --check
forge build --root contracts --deny-warnings
forge test --root contracts
```

真实 provider key、生产 RPC、正式 Holder/Router、主网 fork 和钱包广播不属于本次本地验证；必须单独完成后才可称为 live E2E。
