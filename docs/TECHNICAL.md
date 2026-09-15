# MetaMatch 技术实现方案 v1

更新日期：2026-09-15。本文与 `PRODUCT.md` v1 同步，是 review 代码时的技术基准。

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
| `src/chains.rs` | 接收 provider 派生的 chain ID；用 `alloy-chains::Chain` 生成规范名称，解析显式或 Alchemy RPC，bootstrap 验证 Router 并读取 Holder |
| `src/domain.rs` | 输入、金额、route、quote 和排序；不持有 provider 身份或能力矩阵 |
| `src/config.rs` | typed JSON 服务参数、RPC/Router、provider 原生 key 和可选 balanceSlots；不解析业务 allowlist |
| `src/providers/mod.rs` | `Provider` trait、provider registry、反向索引、共享 DTO/helper 和 route 归一化 |
| `src/providers/*.rs` | 每家 provider 的独立 endpoint、认证、DTO 使用和 route 适配 |
| `src/competitions.rs` | 由反向索引选 provider，总 deadline/capacity、批量 routes → 公共 context → 并发 simulation、返回结果 |
| `src/rpc.rs` | 共享 Alloy 2.x `DynProvider`、原始 RPC 错误分类、parent block 和 gas context |
| `src/simulation.rs` | 固定 block 顺序仿真、余额/授权/到账/gas/reorg 检查 |
| `src/balance_slots.rs` | 配置优先的 mapping base 解析、假地址只读 trace 探测及有界进程缓存 |
| `src/execution.rs` | route 边界、ABI、AllowanceHolder/MetaRouter transaction 编码 |
| `src/app.rs` | Axum endpoints、typed request/response、请求体限制、错误 envelope、rejection/header 处理、capabilities |
| `src/error.rs` | `ErrorKind` typed context；不持有 error、HTTP status 或诊断字符串副本 |
| `src/api_error.rs` | 公开 `ApiError`、封闭错误码映射、HTTP status、Axum rejection 和 JSON envelope |

依赖方向：`app -> competitions -> {providers, rpc, simulation, execution, domain, config}`；provider 不调用 competition，domain 不依赖网络。

## 3. 配置模型

`load_config_file` 读取一个文件（默认当前目录 `config.json`，CLI 可指定路径），`load_config` 反序列化完整 JSON 并拒绝尾随文档。不读取 `.env` 或应用环境变量。Serde 用 `rename_all / deny_unknown_fields / default / try_from` 与 typed IP/地址/整数约束结构，并在转换阶段检查范围、RPC scheme、凭据空白值和 balanceSlots。

`Config.chains` 仅保存每条链的基础设施参数，不决定链集合。异步 `Services::production` 创建 provider registry 和共享 RpcClients；`bootstrap_chains` 对有 Router 的链检查 RPC chain ID，在同一区块高度读取 Router code、`allowanceHolder()`、Holder code。getter 严格解码，拒绝空/非法/保留/自身地址；每链受 timeout_ms 限制。错误包裹 chain ID 与 INVALID_CONFIG，任何配置 Router 验证失败都不启动 listener。

只有 bootstrap 成功才能形成 `Chain.router: Option<RouterDeployment { address, holder }>`。缺少 Router 的链仍可进入 capabilities；配置 Router 但没有 RPC 或不属于 provider 链并集时失败，不静默忽略。RPC 优先 `chains.<id>.rpcUrl`，其次 `alchemyApiKey` 生成的官方 endpoint。完整 schema、默认值、迁移与运维边界见 [CONFIGURATION.md](CONFIGURATION.md)。

access key 不建通用 credential trait。每个适配器持有自己的原生 credential，并在自己的 `supported_chains` 中决定当前实例可用的链；注册表只消费链集合，不重复判断凭证。OKX 的 key/secret/passphrase 必需、project ID 可选；Bebop Bearer、Kyber client ID、Odos key 是 optional。官方公开 endpoint 的 optional 字段不改变参与资格；取得正式 Pro/Enterprise 接入契约后再在对应 adapter 内实现新的 host/认证。

## 4. Provider 接口和注册表

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;
    fn requires_access_key(&self) -> bool;
    fn supported_chains(&self) -> Vec<u64>;
    async fn quote(&self, input: &Input, chain: &Chain, sender: Address)
        -> anyhow::Result<Route>;
}
```

`supported_chains()` 表示 provider 实例当前实际可参与的链：必须凭证缺失时返回空集合，免 key 或 optional key provider 返回自身支持矩阵。`ProviderRegistry::new` 在启动时直接对 provider 返回的链集合建立索引，并从索引 key 派生有序 chain ID 列表：

```rust
HashMap<u64, Vec<&'static str>>
```

竞赛只从 `registry.for_chain(input.chain_id)` 取得 provider。不能在 `competitions.rs` 再遍历“全部 provider”或重新判断 key，否则会破坏单一索引语义。provider ID 是固定字符串，不引入 `ProviderId` enum。

每个 `src/providers/*.rs` 是自身 ID、链矩阵、凭证要求和原生交易校验的代码事实来源；`domain.rs` 不再列举 provider。Rust 与合约均采用 permissionless 策略，不维护 `Rule`、`Provider::rules()` 或路由登记。新增 provider 仍需独立 adapter、原生凭据、DTO fixture、capability/index 测试和真实执行验收。

## 5. 单请求竞赛

1. typed `Json<Input>` 解码；请求与响应复用同一份 Input 字段定义，不再维护 CreateCompetitionRequest 或逐字段转换。chainId 在反序列化时通过 NonZeroU64 校验后保存为 u64，地址必须为带 0x 前缀的字符串，sellAmount/slippage 在 Serde 解码时校验，slippage 默认 30。随后显式调用 `Input::validate()` 检查 token 组合和保留 taker，保持 `INVALID_INPUT` 与 `INVALID_TAKER` 分类独立；运行时 Router/Holder 地址在 competition 入口另行检查。收款人为同一 taker。直接在 Rust 中构造 Input 不会触发 Serde 校验，内部调用者须维持这些字段不变量。
2. `Competitions::create` 取得 `Semaphore` permit，超额立即返回 429；使用单个 Tokio `Instant` 截止时间覆盖 provider quote、交易构建、context 和 simulation。
3. 确认 Router 已配置，再对 registry 中各 provider 并发执行 quote，并校验 provider ID、金额、原生 deadline、value/calldata 和基本 target/spender 结构。`validate_route` 消费 Route 并返回不可变的 `ValidatedRoute`，随后编码与仿真不重复整套校验。每条 route future 使用同一个绝对 deadline；失败项立即归入该 provider 的 failure。
4. 等全部 route future 成功、失败或超时结算后才获取一次公共 parent context。这样 context 的 block 不会早于本批次 route 的生成时点；公共准备失败时，已取得的 routes 分别返回明确失败，不构造虚假报价或交易。
5. 在同一个 context 调用一次 `SimulationProvider::prepare`：检查 parent hash，再并行读取 Router code 与资金准备信息（ERC20 mapping base、taker 对 Holder 的 allowance）。产生本轮共享的 `SimulationPreparation {router, approvals, sell_balance_slot}`；native 无 token slot/approval。准备错误分发给有效 routes，不跨请求保存失败。随后并发 simulation 所有有效 route。每家互不继承其他 provider 的模拟状态；达到绝对 deadline 时丢弃未完成 future，已完成的 Quote 保留。deadline 不按阶段重置，因此 route 阶段耗尽预算时，context/simulation 也无法完成。
6. 仅 route 和完整仿真成功且余额增量达到 route minimum 时构造成功专用 `Quote`，保存完整 route 和原始 `SimResult` 的 simulation、approvals、transaction。Quote 无 status/error/Option 成功字段，排序无需筛选错误状态。排序按模拟到账整数降序，平局按 provider ID；失败项单独存入 failures。无有效 output 的结果不能成为可执行候选。

不再有 HashMap 竞赛存储、token 鉴权、polling、单独 build 或 TTL 清理。permit 随请求 future 完成、失败或取消释放；HTTP 连接结束是否立即 drop handler 取决于服务器行为，未取消的 handler 仍受总 deadline 限制。`id` 仅用于日志关联。

配置项 `competitionTimeoutMs` 默认 6000，范围 100–30000。provider HTTP/RPC 的单次超时以该值作为上限，竞赛总 deadline 会进一步限制实际剩余时间；不会每完成一个阶段就重置预算。旧 `PROVIDER_TIMEOUT_MS`、`QUOTE_TTL_MS` 和 `max_competitions` 移除。

## 6. 仿真与资金边界

每次 public competition 从一开始绑定真实 taker，但 simulation 不读取 taker 的真实 native 或卖出 token 余额。native sell 时直接覆盖足以支付 transaction value 和声明 gas budget 的 native balance；ERC20 sell 时解析 mapping base，把 `storage_key(taker, base)` 直接覆盖为 `sellAmount`，并同时覆盖 native gas balance。allowance 在同一区块每轮读取一次，以决定共享的 reset/approve 列表。各 route 独立构造 payload/state overrides，模拟后的 block hash 检查仍逐 route 执行。随后顺序执行买入 token 余额查询、必要的 reset/approve、swap、余额查询。approval 返回 false、到账低于 minimum、RPC 不支持、完整交易 revert 或区块重组均明确失败，不返回 Quote。

每条成功 simulation 包含：

- `blockContext: {number, hash, timestamp}`：基础区块，number 为十六进制 quantity 字符串，timestamp 为 Unix 秒。
- `simulatedTimestamp`：传入 `eth_simulateV1` 的执行时间，目前为 parent timestamp + 1；不能把它误当作基础区块 timestamp。
- `boughtAmount`：完整调用序列中真实收款地址的买入 token 模拟余额增量；`gasUsed` 为 approval/swap 合计，`gasFeeWei` 未支持时为 null；`funding: overridden`。该字段明确表示卖出资产和 gas 资金是 state override 假设，不证明钱包当前有足够余额。

API 不返回 expiresAt，不按区块年龄淘汰结果。重新 simulate 是对相同交易的状态复核；重新 competition 会重新获取报价。调用方自行选择，并自行确认新报价的 minimum。最低到账写入交易，后端不默默替换已经返回的交易。

`Route.deadline: Option<u64>` 仅保存上游明确提供的 Unix 秒期限（例如 Bebop expiry），归一化时拒绝已经失效的上游报价。移除各 adapter 在 route 元数据中人工添加的 10/20 秒期限。原有 provider API 请求字段保持不变，包括 Kyber build 的 60 秒 deadline 和 Barter swap 的 20 秒 deadline；这些仍可限制最终 calldata 的执行时间。没有上游期限时 MetaRouter deadline 编码为 `U256::MAX`，provider calldata 内的其他执行约束原样保留；这不增加业务层 TTL。

正式 transaction 保持：

```text
user wallet -> AllowanceHolder.exec -> MetaRouter.execute -> provider target
```

缺少 Router 的 public competition 返回 unavailable。配置的 Holder 来自启动时的 `allowanceHolder()`，用于钱包 allowance 查询、approvals 和最外层交易入口；Router 地址作为 operator、内层 target 和 provider sender。

Solidity 已无 admin/owner、pause、白名单、recover、其他 ERC20 target 探测和 Holder 固定嵌套形状检查。本次未修改合约行为。最新 ABI 为 `execute(sellToken, buyToken, receiver, sellAmount, minBuyAmount, deadline, spender, target, value, data)`（selector `0xf509c3a5`），Rust 同步编码，API taker 映射为 sender/receiver。精确授权与清零、最低实际到账、退款增量、同交易资产边界和重入保护继续执行。无关暂存资产不保证安全，详见 [合约说明](../contracts/README.md)。

Rust 不再使用路由白名单；`ROUTE_NOT_ALLOWLISTED` 错误分类和映射也已删除。permissionless 不取消 provider 原生 target/spender 约束、交易不变量或完整仿真。配置成功只证明部署 getter/代码检查通过，不等于生产执行已验收。

`Services::production` 为 context 与 simulator 注入同一个 `Arc<RpcClients>`，按配置 URL 复用 `DynProvider` 和 reqwest 连接池。业务代码直接调用 Alloy，不再维护 `RpcFactory`/`EvmRpc`/`AlloyRpc` 转发层或 `BlockInfo` 副本；保留 `ContextProvider`、`SimulationProvider` 业务测试边界。客户端复用不缓存报价或链状态，也不增加 RPC fallback。

### 6.1 独立 BalanceSlots

`Chain` 不再携带 `balance_slots` map。启动时 `balanceSlots` 解析为 `chain ID -> Address -> U256`（槽位使用 `0x0` 等十六进制字符串），注入服务端共享的 `Arc<BalanceSlots>`；它独立管理配置和自动探测缓存，不维护 token allowlist。非法 JSON/地址/槽位、零 chain ID 和保留 token 地址在配置边界失败。

解析顺序：

1. `resolve(rpc, chainId, token, block) -> U256` 可独立调用，不接收真实 owner/amount，不构造 StateOverride，也不等待余额不足。配置命中直接返回 mapping base，不发 RPC，也不自动覆盖配置。
2. 无配置则查询 `(chainId, token) -> U256` 进程缓存；命中直接返回 base，不依赖当前用户或金额，也不发 RPC。
3. 缓存未命中：从固定标签派生两个假的 owner，用 `tokio::try_join!` 并发执行两次 Alloy typed `create_access_list(...).block_id(block)`，生成同一区块 `balanceOf(fakeOwner)` 的 access list，全程不使用 state override。一侧报错时返回错误并丢弃另一侧 future，不启动后台任务。对 base `0..1023` 本地计算 `keccak256(abi.encode(fakeOwner, base))`，匹配 token 地址对应的 `storageKeys`。取两个地址候选的交集，仅唯一 base 可缓存；找不到、歧义或 token storage key 超过上限时明确失败。每个 call gas limit 为 100000，最多接受 32 个 token storage key。任意 uint256/namespaced base 可通过配置提供，不承诺自动识别，也不逐槽发 RPC。
4. `storage_key(owner, base)` 是纯 hash 计算。每轮竞赛只解析一次 base 并派生 taker 的 slot，各条 ERC20 simulation 消费同一个解析结果，以 `sellAmount` 构造 token stateDiff；每次 simulation 都把 taker 的 native balance 覆盖为 transaction value 与声明 gas budget 之和。不执行真实余额查询、反值探测、另一地址检查或 cache invalidation。配置错误或不兼容布局由后续完整调用序列的失败暴露；配置始终保留。RPC 错误继续保留原始 source chain。

缓存采用 `Mutex<HashMap<(u64, Address), SharedBase>>`，其中 `SharedBase = Arc<tokio::sync::Mutex<Option<U256>>>`。外层锁只保护索引，不跨 await；内层锁只合并同一 key 的探测。省去独立 CacheKey/CacheEntry 和 FIFO 队列，常规查找使用 HashMap。上限保持 1024 项，满时淘汰任意空闲项，不保证淘汰顺序；全部正在使用时返回明确 busy。其他 key 的 RPC 互不等待；取消后释放锁，失败不保存可复用结果，下次可重试。缓存不持久化、不跨副本共享、不设置报价 TTL。RPC 继续使用服务端客户端超时，调用者的整体 deadline 可以取消解析过程。

只读探测识别的是被 `balanceOf` 访问的 mapping base。simulation 不再用额外 `eth_call` 验证 override，也不因真实余额不足而拒绝；它直接写入卖出余额，再由完整 approval/swap 仿真验证整条执行路径。错误配置或不兼容布局可能让完整仿真失败，配置项不会被自动覆盖，缓存也不会因单次仿真失败而失效。`BalanceSlotError` 使用 BALANCE_SLOT 分类，在 simulation 边界映射为原有稳定 unsupported reason；Quote 成功结构不变。

协议依据：[Geth eth_createAccessList](https://geth.ethereum.org/docs/interacting-with-geth/rpc/ns-eth#eth_createaccesslist)、[Geth state override](https://geth.ethereum.org/docs/interacting-with-geth/rpc/objects)。生产 RPC 的 access-list generation 与 `eth_simulateV1` state override 能力需部署环境单独验证。

## 7. Provider adapter 分层

公共层只做 HTTP timeout/响应上限/JSON 解码，adapter 自己负责：

- endpoint、chain path、认证 header 和 provider 参数命名；
- provider 原始 amount/output/transaction 字段解析；
- spender/target/value/transaction data 的 provider-specific 约束；
- 转换为统一 `Route`。

禁止把所有 provider 压成一个“万能 quote 请求”：各家对 key、chain slug、slippage、spender、RFQ、gasless、Permit 和 calldata 语义不同；公共抽象只保留稳定的 `Provider` 接口，避免错误复用。

13 家 adapter（包括 Kyber、1inch）统一经过 `normalize_route`，保留 provider-specific target/spender/from/amount/value/期限和 state override 限制。竞争入口将 provider 返回值检查为不可变 `ValidatedRoute`；`swap_transaction` 只接受该类型和显式 `RouterDeployment`，不再接收 Rules、可选 minimum 或 direct-provider fallback。编码时只重查可能随时间失效的 deadline；minimum 始终来自 route。adapter 的上游协议校验与竞争入口对 Provider trait 结果的校验是不同边界，保留相应错误分类。

`Tx.data` 和 ABI helper 使用 `Bytes`，上游 hex 只解码一次；公开 JSON 仍为 hex 字符串，金额仍为十进制字符串。Alloy RPC transaction 仅发送 `input`，不再同时复制 `data`；不改变返回钱包交易的 `data` 字段。HTTP 直接从 response bytes 解码 typed DTO，用 `serde_path_to_error` 保留路径，不再先构造整棵 Value。动态字段仍可使用 Value。拒绝尾随 JSON；typed DTO 重复的已知字段现在报错，而非先经 Value 合并。语法错误保持 UPSTREAM_INVALID_JSON，合法 JSON 的类型/结构错误保持 UPSTREAM_INVALID_RESPONSE；仅在 DTO 提前报结构错时用不分配树的 IgnoredAny 复核剩余语法。

`minimum` 用商/余数分解精确计算 floor(amount × (10000-bps) / 10000)，避免 U256 中间乘法溢出；helper 拒绝 bps > 10000，API 的 1..500 范围不变。LiquidSwap 十进制单位转换用字符串定位/补零，支持完整 u8 decimals 范围，不计算可能溢出的 10^decimals。

当前 13 个 provider 均已完成 adapter。每家 adapter 位于独立的 `src/providers/*.rs` 文件；测试统一位于 `tests/`。逐家 endpoint、认证、单位、native marker、spender/target 和已知限制见 [PROVIDER_INTEGRATION_GUIDE.md](PROVIDER_INTEGRATION_GUIDE.md)。各 adapter 必须以官方 API 文档和脱敏 fixture 固化 response schema，不能用任意字段猜测 target 或 spender；真实供应商行为不由 fixture 证明。

## 8. Axum API 边界

`src/app.rs` 的 create_app/create_app_with_services 直接返回 Axum Router，使用 `State<Arc<Competitions>>` 注入服务，不再增加单字段 App/AppState 包装。请求只浅克隆 Arc；health 不提取 state。POST competition 使用 typed `Json<Input>` 和显式 `Input::validate()`；`WithRejection` 保留 JSON rejection 的安全错误 envelope。成功请求返回 200 和 `CompetitionResponse {id, input, quotes, failures}`。测试仍通过 create_app_with_services 注入 provider/RPC 服务。

移除 GET competition、POST build、Bearer header/token 和 path extraction。旧路由返回 404。Router 保留 body limit、404/405 fallback、`Cache-Control: no-store`。`Quote` 仅包含必填 route、SimulationSuccess、approvals、transaction、latencyMs；provider/原始金额从 route 读取。失败返回到独立的 `ProviderFailure` 数组，没有 route/交易字段。全部失败时 quotes 为空数组；failures 保留失败原因。

## 9. 内部错误与公开错误

- 内部统一返回 `anyhow::Result<T>`，不保留 `Fault` struct、别名或自定义 Result。`?`、`Context::context`/`with_context` 保留 reqwest、Alloy、serde、Tokio 的原始错误；`bail!(ErrorKind::…)` 用于无底层 cause 的明确失败。`error.chain()`/downcast 可访问内部根因，`RUST_LIB_BACKTRACE=1` 在错误创建时捕获栈。
- `ErrorKind` 是仅有分类的 enum，由 thiserror 提供 Error/Display，可作为 `.context(ErrorKind::…)` 附加到原始错误。通过 `error.downcast_ref::<ErrorKind>()` 取最外层分类；外层分类不丢失内层 source/backtrace。不使用字符串匹配判断错误，不为普通诊断文本创建分类。
- `ApiError` 保持独立，在 API/quote 投影边界将已知分类映射为原有公开 code/status。未分类的库错误或字符串 context 一律 `INTERNAL_ERROR`，即使文本恰好等于 `INVALID_INPUT`；JSON 仍为 `{"error":"CODE"}`，原始原因和 backtrace 不进入 HTTP response。仿真层只判断内部 `ErrorKind`，不依赖 `ApiError`。
- HTTP 保留 status、响应 body（沿用现有大小上限）和 endpoint context；JSON DTO 使用 `serde_path_to_error` 保留原始反序列化错误和字段路径。RPC 保留 Alloy 错误，直接 `.context(method)`；逐笔 revert 保留 Alloy call result、错误和返回 bytes。
- 仿真仅以 `Err(anyhow::Error)` 传播失败，`SimResult` 只承载成功数据，不再包含 `diagnostic`。`SimulationFailure` 是保留 `reverted / unsupported / error` 和稳定 reason 的 typed context，不包装 source/report；竞赛在输出边界只将安全的 SimulationFailure 分类序列化到 ProviderFailure.simulation；SimResult.simulation 为成功专用 SimulationSuccess，不能表示失败。准备阶段的既有输入/route 错误保持原分类；不再使用 `Ok(失败结果)` 作为另一条失败通道。
- 失败边界直接使用 `tracing::warn!(error = ?error, "操作失败")` 输出 anyhow 原因链和已捕获的 backtrace，competition/provider span 提供关联。按当前简化要求，不增加日志白名单、文本原因分类器、专用元数据类型或自定义脱敏 Debug；内部日志可能含上游错误详情，但这些详情不进入 API/quote。
- `main` 返回 `anyhow::Result<()>`，由标准退出行为报告启动错误并返回非零退出码。JSON 文件和 Router bootstrap 在 HTTP 监听前完成；文件缺失、格式/校验/RPC 错误保留上下文并失败。
- provider deadline 必须生成该 provider 的 ProviderFailure，不能提前 `?` 导致该家从竞赛结果消失；不增加自动重试、降级 mock 或隐藏失败的 fallback。

公开 API 保留错误 envelope；旧 build/auth 错误码已随路由删除。内部通过 typed downcast 或 `error::kind` 获取分类。`tests/errors.rs` 覆盖 source/backtrace、分类覆盖、HTTP/JSON/RPC 原因链、API 详情隔离、revert 与 timeout；`tests/startup.rs` 用隔离子进程验证默认/显式 JSON 路径、忽略应用环境变量与配置错误非零退出；真实重放入口仍为 `tests/replay_live.rs`。

## 10. 验证顺序

```sh
sh scripts/check.sh
```

精确命令只维护在 [scripts/check.sh](../scripts/check.sh)，CI 共用该入口，包含 Cargo、Foundry 和隔离 Anvil E2E；任一步失败立即停止。迭代时仍可单独运行相关测试。

默认质量门不访问生产 provider/RPC。2026-09-14 在用户明确授权下另行完成真实 provider + RPC 的 direct-preview 重放；正式 Holder/Router、主网 fork 和钱包广播仍未验证，不能把 preview 称为实际主网成交。详见 `VERIFICATION.md`。
