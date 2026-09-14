# MetaMatch 技术实现方案 v1

更新日期：2026-09-14。本文与 `PRODUCT.md` v1 同步，是 review 代码时的技术基准。

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
| `src/chains.rs` | 接收 provider 派生的 chain ID；用 `alloy-chains::Chain` 生成规范名称，并解析显式或 Alchemy RPC |
| `src/domain.rs` | 输入、金额、route、quote 和排序；不持有 provider 身份或能力矩阵 |
| `src/config.rs` | 服务参数、RPC 环境变量和各 provider 原生 key；不解析链/token/provider policy JSON |
| `src/providers/mod.rs` | `Provider` trait、provider registry、反向索引、共享 DTO/helper 和 route 归一化 |
| `src/providers/*.rs` | 每家 provider 的独立 endpoint、认证、DTO 使用和 route 适配 |
| `src/competitions.rs` | 由反向索引选 provider，并发 quote、TTL/capacity、polling、重新 quote/build |
| `src/rpc.rs` | 共享 Alloy 2.x `DynProvider`、原始 RPC 错误分类、parent block 和 gas context |
| `src/simulation.rs` | 固定 block 顺序仿真、余额/授权/到账/gas/reorg 检查 |
| `src/execution.rs` | route 边界、ABI、AllowanceHolder/MetaRouter transaction 编码 |
| `src/app.rs` | Axum endpoints、typed request/response、请求体限制、错误 envelope、rejection/header 处理、capabilities |
| `src/error.rs` | `ErrorKind` typed context；不持有 error、HTTP status 或诊断字符串副本 |
| `src/api_error.rs` | 公开 `ApiError`、封闭错误码映射、HTTP status、Axum rejection 和 JSON envelope |

依赖方向：`app -> competitions -> {providers, rpc, simulation, execution, domain, config}`；provider 不调用 competition，domain 不依赖网络。

## 3. 配置模型

`Config` 只有服务运行参数、按 chain ID 索引的显式 RPC、Alchemy key 和各 provider 原生凭据；不保存链集合。`Services::production` 先创建当前 provider 实例，`ProviderRegistry` 对它们的 `supported_chains()` 求并集并排序，然后 `configured_chains` 才为这些 ID 建立运行时 `Chain`。因此没有静态 `CHAIN_CATALOG` 或第二份链 allowlist。

每条运行时链的 RPC 依次取第一个非空值：`RPC_URL_<chainId>`、仅 Ethereum 的兼容别名 `ETHEREUM_RPC_URL`、由 `ALCHEMY_API_KEY` 生成的官方 HTTPS endpoint。`alchemy_rpc_url(chain_id)` 只返回不含 key 的 base URL，随后用 `url` crate 把 key 编码为 path segment。显式 URL 在配置边界要求 HTTP(S) host。默认没有 RPC、Router 或 balance slot，所以服务仍可启动但不能声称真实仿真/可执行。

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
        -> anyhow::Result<Route>;
}
```

`supported_chains()` 表示 provider 实例当前实际可参与的链：必须凭证缺失时返回空集合，免 key 或 optional key provider 返回自身支持矩阵。`ProviderRegistry::new` 在启动时直接对 provider 返回的链集合建立索引，并从索引 key 派生有序 chain ID 列表：

```rust
HashMap<u64, Vec<&'static str>>
```

竞赛只从 `registry.for_chain(input.chain_id)` 取得 provider；build 通过字符串 ID 回查同一个 registry。不能在 `competitions.rs` 再遍历“全部 provider”或重新判断 key，否则会破坏单一索引语义。provider ID 是固定字符串，不引入 `ProviderId` enum。

每个 `src/providers/*.rs` 是自身 ID、链矩阵、凭证要求和 rules 的代码事实来源；`domain.rs` 不再列举 provider。后续新增 provider 的最小步骤是：新增独立 adapter、实现上述 provider 元数据、注册原生 key、增加 DTO fixture 和 capability/index 测试。

## 5. Quote 生命周期

1. HTTP 层由 typed `Json<CreateCompetitionRequest>` 解码，再由 `validate_input` 验证地址格式、正整数金额、滑点、taker 保留地址；不查 token registry。
2. `Competitions::create` 只按 chain ID 查当前 provider 并集对应的运行时链，并建立带容量和 TTL 的内存状态。
3. 异步任务读取 context，然后从反向索引获取 eligible providers，并发请求。
4. 每个 route 经过 provider rules、sell amount、min amount、transaction value、calldata、expiry 和统一执行边界校验。
5. 有 context 时进入 simulation；provider 失败、超时、无 RPC 或待实现 adapter 都只写入自身 quote 状态。
6. polling 使用 `rank`，只把未过期、仿真成功且有 `quotedAmount` 的 quote 当作推荐候选；同一请求中的 buy token 已固定，直接比较整数输出。

token 地址透传意味着 v1 不维护 decimals/price registry，也不调用额外价格源；报价排序不扣除无法可靠归一化的 gas/token 价格。

竞赛任务与 build 分别持有 Tokio `Semaphore` permit，容量仍使用 `max_active`；`try_acquire_owned` 保留立即拒绝超额请求的行为，不新增排队。permit 随 future 完成、失败、取消或 unwind 自动释放。内存条目仍受 `max_competitions` 和 TTL 限制；清理/close 只移除快照，不承诺中断上游任务，进行中的请求由已有 timeout 收束。

## 6. Build 与资金边界

build 重新获取 context 和 route，不复用 quote 阶段 calldata；检查真实 taker、quote expiry、用户接受底价、provider ID、目标地址、spender、selector、sell amount 和 native value。若没有 Router，返回 `ROUTER_NOT_CONFIGURED`，不自动退化成逐 provider 的未保护交易。

仿真继续保持以下不变量：固定 parent block 和 block hash 二次确认；ERC-20 preview 余额不足时只有经过验证的 balance slot override 才能继续；approval 返回值必须为 true；实际到账必须达到 minimum；gas 余额必须足够；`eth_simulateV1` 不支持时是 `unsupported`，不是成功。

无 taker 时使用保留 preview 账户（`keccak256("MetaMatch preview account")` 的低 20 bytes），不持有对应签名 key。原生币 preview 余额按实际顺序 payload 中的 `value + Σ(gasLimit × gasPrice)` 补足，包含 reset/approval/swap；前后到账 probe 显式 gasPrice=0。不再按固定 2M gas 注资却提交 8M gas-limit 交易。真实 taker 路径不增加原生币或 token 余额，余额不足明确失败。

正式 transaction 仍是：

```text
user wallet -> AllowanceHolder.exec -> MetaRouter.execute -> provider target
```

Router 的 route allowlist、临时授权、余额增量、退款、pause 和 reentrancy 由 `contracts/src/MetaRouter.sol` 保证；Rust 只编码并验证边界，不替代链上保护。

`Services::production` 为 context 与 simulator 注入同一个 `Arc<RpcClients>`，按配置 URL 复用 `DynProvider` 和 reqwest 连接池。业务代码直接调用 Alloy，不再维护 `RpcFactory`/`EvmRpc`/`AlloyRpc` 转发层或 `BlockInfo` 副本；保留 `ContextProvider`、`SimulationProvider` 业务测试边界。客户端复用不缓存报价或链状态，也不增加 RPC fallback。

## 7. Provider adapter 分层

公共层只做 HTTP timeout/响应上限/JSON 解码，adapter 自己负责：

- endpoint、chain path、认证 header 和 provider 参数命名；
- provider 原始 amount/output/transaction 字段解析；
- spender/target/value/transaction data 的 provider-specific 约束；
- 转换为统一 `Route`。

禁止把所有 provider 压成一个“万能 quote 请求”：各家对 key、chain slug、slippage、spender、RFQ、gasless、Permit 和 calldata 语义不同；公共抽象只保留稳定的 `Provider` 接口，避免错误复用。

归一化与执行校验共用 `execution::validate_transaction` 检查 expiry、native value 和 calldata。provider 仍负责正数解析、上游金额匹配及非零 target/spender，执行层仍负责 route 金额不变量和 allowlist；不同边界的错误语义不强行合并。`swap_transaction` 保留入口校验，build 重新报价后仍校验。camelCase DTO 使用 Serde `rename_all`，保留特殊字段名、默认值、未知字段拒绝和可选字段省略规则。

当前 13 个 provider 均已完成 adapter。每家 adapter 位于独立的 `src/providers/*.rs` 文件；测试统一位于 `tests/`。逐家 endpoint、认证、单位、native marker、spender/target 和已知限制见 [PROVIDER_INTEGRATION_GUIDE.md](PROVIDER_INTEGRATION_GUIDE.md)。各 adapter 必须以官方 API 文档和脱敏 fixture 固化 response schema，不能用任意字段猜测 target 或 spender；真实供应商行为不由 fixture 证明。

## 8. Axum API 边界

`src/app.rs` 使用 Axum `State` 注入应用状态；写请求使用 typed `Json<CreateCompetitionRequest>`/`Json<BuildRequest>`，路径使用 typed `Path`，不在 handler 中接收 `serde_json::Value` 作为请求协议。`axum-extra` 的 `TypedHeader` 读取 Bearer Authorization，`WithRejection` 将 JSON、path 和 header rejection 统一转换为当前 API error envelope。

Router 同时配置 body limit、404/405 fallback 和 `Cache-Control: no-store` response layer。无效凭据返回 401 和 `WWW-Authenticate: Bearer`；未知字段和格式错误不会把 serde 内部错误文本暴露给调用方。`tests/api.rs` 固化这些 HTTP 可观察行为。

## 9. 内部错误与公开错误

- 内部统一返回 `anyhow::Result<T>`，不保留 `Fault` struct、别名或自定义 Result。`?`、`Context::context`/`with_context` 保留 reqwest、Alloy、serde、Tokio 的原始错误；`bail!(ErrorKind::…)` 用于无底层 cause 的明确失败。`error.chain()`/downcast 可访问内部根因，`RUST_LIB_BACKTRACE=1` 在错误创建时捕获栈。
- `ErrorKind` 是仅有分类的 enum，由 thiserror 提供 Error/Display，可作为 `.context(ErrorKind::…)` 附加到原始错误。通过 `error.downcast_ref::<ErrorKind>()` 取最外层分类；build 可覆盖分类但不丢失内层 source/backtrace。不使用字符串匹配判断错误，不为普通诊断文本创建分类。
- `ApiError` 保持独立，在 API/quote 投影边界将已知分类映射为原有公开 code/status。未分类的库错误或字符串 context 一律 `INTERNAL_ERROR`，即使文本恰好等于 `INVALID_INPUT`；JSON 仍为 `{"error":"CODE"}`，原始原因和 backtrace 不进入 HTTP response。仿真层只判断内部 `ErrorKind`，不依赖 `ApiError`。
- HTTP 保留 status、响应 body（沿用现有大小上限）和 endpoint context；JSON DTO 使用 `serde_path_to_error` 保留原始反序列化错误和字段路径。RPC 保留 Alloy 错误，直接 `.context(method)`；逐笔 revert 保留 Alloy call result、错误和返回 bytes。
- 仿真仅以 `Err(anyhow::Error)` 传播失败，`SimResult` 只承载成功数据，不再包含 `diagnostic`。`SimulationFailure` 是保留 `reverted / unsupported / error` 和稳定 reason 的 typed context，不包装 source/report；竞赛在输出边界投影为 `Simulation`，build 添加 `BUILD_REVERTED / BUILD_UNSUPPORTED / BUILD_ERROR` 分类而保留原始 cause。准备阶段的既有输入/route 错误保持原分类；不再使用 `Ok(失败结果)` 作为另一条失败通道。
- 失败边界直接使用 `tracing::warn!(error = ?error, "操作失败")` 输出 anyhow 原因链和已捕获的 backtrace，competition/provider span 提供关联。按当前简化要求，不增加日志白名单、文本原因分类器、专用元数据类型或自定义脱敏 Debug；内部日志可能含上游错误详情，但这些详情不进入 API/quote。
- `main` 返回 `anyhow::Result<()>`，由标准退出行为报告启动错误并返回非零退出码。dotenv 在线程创建前加载；缺少文件允许启动，格式错误保留上下文并失败。
- provider deadline 必须生成该 provider 的失败 quote，不能提前 `?` 导致该家从竞赛结果消失；不增加自动重试、降级 mock 或隐藏失败的 fallback。

公开 API 兼容现有 envelope、401 Bearer header 和 quote 状态；内部接口的 Rust 返回类型改为 anyhow，需要旧调用方从 `.code` 改用 typed downcast 或 `error::kind`。`tests/errors.rs` 覆盖 source/backtrace、分类覆盖、HTTP/JSON/RPC 原因链、API 详情隔离、revert 与 timeout；`tests/startup.rs` 用隔离子进程验证 dotenv 错误上下文与非零退出；真实重放入口仍为 `tests/replay_live.rs`。

## 10. 验证顺序

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --release
forge fmt --root contracts --check
forge build --root contracts --deny-warnings
forge test --root contracts
```

默认质量门不访问生产 provider/RPC。2026-09-14 在用户明确授权下另行完成真实 provider + RPC 的 direct-preview 重放；正式 Holder/Router、主网 fork 和钱包广播仍未验证，不能把 preview 称为实际主网成交。详见 `VERIFICATION.md`。
