# MetaMatch 项目架构与逻辑 v1

本文是当前 Rust 代码的 review 导航。代码、fixture 和测试是事实来源；本文记录模块职责、状态变化和必须保持的不变量。

最后核对：2026-09-15。

## 1. 一句话概览

MetaMatch 是一个单进程、有总超时、非托管的多链 EVM exact-input 报价竞赛服务：当前 provider 实例的能力矩阵直接生成链集合和 `chain -> provider` 反向索引，收到请求后只调度对应链的 provider，首次请求绑定真实 taker → 并发获取并校验 routes → 获取公共 parent block → 并发仿真 → 按模拟到账量排序 → 返回 unsigned transactions。

服务端不持有私钥，不签名、不广播；交易由用户钱包执行。

## 2. 组件关系

```text
调用方 / 钱包
 competition → approve → swap
                    │ HTTP/JSON
                    ▼
              Axum app
                    │
                    ▼
          Competitions + deadline
             │                 │
             │                 ├─ ContextSource ── Alloy 2.x RPC
             │                 └─ Simulator ────── eth_call/simulateV1
             │
             └─ ProviderRegistry
                  chainId -> eligible providers
                    │
                    └─ fixed provider HTTP APIs

wallet → AllowanceHolder.exec → MetaRouter.execute → provider target
                                      │
                                      └─ minimum/output/refund/reentrancy
```

## 3. 文件和职责

| 文件 | 职责 | 不应放入 |
| --- | --- | --- |
| `src/main.rs` | 加载配置、启动 Axum、优雅退出 | 业务规则、签名、provider 逻辑 |
| `src/chains.rs` | 将 provider 派生的 chain ID 转成运行时链、Alchemy base URL 和最终 RPC | provider 能力矩阵、用户 token |
| `src/config.rs` | 服务参数、RPC URL、各 provider 原生 key | chain/provider policy、token 列表 |
| `src/domain.rs` | 输入、金额、route、quote、排序 | HTTP、RPC、provider 身份/能力、异步状态 |
| `src/providers/mod.rs` | Provider trait、registry/反向索引、共享 DTO/helper 和 route | 签名、广播、竞争状态 |
| `src/providers/*.rs` | 每家 provider 的独立 endpoint、认证、chain matrix 和 DTO 归一化 | registry、竞争状态、签名 |
| `docs/PROVIDER_INTEGRATION_GUIDE.md` | 13 家官方接入契约、申请前提、单位、native/spender/target 差异 | 运行时业务逻辑 |
| `src/competitions.rs` | capacity/总 deadline、按索引调度、构建与仿真 | provider JSON 字段、ABI 细节 |
| `src/rpc.rs` | Alloy typed EVM RPC、block/gas/context | 合约资金不变量、管理员权限 |
| `src/simulation.rs` | 固定 block 顺序仿真、余额/授权/到账/gas/reorg | provider schema、HTTP API |
| `src/balance_slots.rs` | 配置优先的 mapping base 解析、假地址只读探测与有界进程缓存 | 真实 owner/amount、StateOverride、资金验证 |
| `src/execution.rs` | route 验证、ERC-20 ABI、Holder/Router calldata | 网络请求、状态生命周期 |
| `src/app.rs` | 路由、JSON 解析、body limit、错误 envelope、capabilities | 供应商协议、仿真细节 |
| `src/error.rs` | 仅分类的 `ErrorKind` typed context | error 容器、HTTP status、重复 context 字符串 |
| `src/api_error.rs` | 内部 code 到安全公开 code/status 的封闭映射、Axum rejection | 原始响应、key、错误堆栈 |
| `contracts/src/MetaRouter.sol` | 链上最终执行和资金边界 | 信任用户 calldata 或配置 |

## 4. 启动和能力发现

### 4.1 启动顺序

1. `main` 先加载可选 `.env`，初始化 tracing，再创建 Tokio runtime 并调用 `load_config_from_env`；已有进程变量优先。
2. `Services::production` 创建全部 13 个当前 provider 实例，并调用 `ProviderRegistry::new`。
3. registry 遍历每个 provider 的 `supported_chains()`，直接写入 `by_chain`，再从索引 key 排序生成运行时 chain ID 列表；没有静态 catalog 或交集步骤。
4. `configured_chains` 按 `RPC_URL_<chainId>` → Ethereum `ETHEREUM_RPC_URL` → `ALCHEMY_API_KEY` 的顺序为这些 chain ID 解析 RPC；无 RPC 的链仍进入运行时集合，后续 context 明确返回 `RPC_NOT_CONFIGURED`。
5. `Competitions` 同时持有该运行时链集合和 registry。
6. Axum 暴露 health、capabilities 和 competition API。

### 4.2 配置语义

| 配置 | 语义 |
| --- | --- |
| `HOST` / `PORT` | 监听地址 |
| `COMPETITION_TIMEOUT_MS` | 整个竞赛预算，默认 6 s，100 ms 至 30 s |
| `RPC_URL_<chainId>` | 最高优先级的 server-side EVM RPC；不是链开关 |
| `ETHEREUM_RPC_URL` | Ethereum 显式 RPC 的兼容别名；优先于 Alchemy |
| `BALANCE_SLOTS` | 可选 chain ID → token → uint256 mapping 基础槽位；所有 ERC20 simulation 在配置缺失时自动探测 |
| `ALCHEMY_API_KEY` | 为 provider 并集中、没有显式 RPC 且存在官方映射的链生成 Alchemy HTTPS endpoint；不是链开关 |
| provider 原生环境变量 | 只传给对应 adapter；必需 key 缺失时 `supported_chains()` 返回空集合，免 key provider 不受影响 |

当前没有 `CONFIG_PATH` 业务配置文件，也没有链/provider/token 集合配置；执行路由另有 target/spender/selector 白名单。`BALANCE_SLOTS` 仅用于 simulation 的卖出 token 存储布局，配置缺失时通过 `eth_createAccessList` 自动探测。服务参数见 `.env.example`。

## 5. Provider 反向索引

每个 provider adapter 保存该家当前官方 API 支持链；Matcha 页面只负责定义 provider 名单。`supported_chains()` 再叠加本实例的认证条件。例如 0x 的逻辑等价于：

```text
supported_chains() = has_0x_key ? matrix(0x) : []
```

免 key provider 则不需要认证条件；optional key 只透传，不改变能力集合。最终 `ProviderRegistry.by_chain` 是竞赛唯一调度来源：

```text
by_chain[8453] = [0x, 1inch, barter, bebop, enso, kyber, odos, ...]
```

具体内容取决于本次进程实际配置的 key。`capabilities` 直接暴露这个结果，不暴露静态 token 列表，也不暴露未参与本次运行的 provider。

## 6. HTTP 和竞赛

`POST /v1/competitions` 要求真实 taker，取得并发名额后在一次 HTTP 请求中分两阶段执行：先并发获取并校验该链全部 provider route；全部 route 请求成功、失败或超时结算后，再获取一次公共 parent context，并基于它并发 simulation 所有有效 route。route 不会使用在其产生之前取得的旧 context。两个阶段共用同一绝对截止时间，不会在阶段切换时重置预算；若 route 阶段耗尽预算，context 和 simulation 也会超时。

返回 200 和 `{id, input, quotes, failures}`；成功项按 `simulation.boughtAmount` 的 U256 数值降序排列，平局按 provider ID。Quote 只包含完整 route、成功专用 SimulationSuccess、同轮仿真的 approvals/transaction 和 latencyMs，均为必填；没有 status/error。失败 ProviderFailure 单独放入 failures 数组，没有 route 或交易。全失败时 quotes 为空。没有存储、轮询、token 或独立 build，旧路由返回 404。取消请求 future 自动释放 permit，不留下后台竞赛任务。

## 7. 仿真与直接执行

仿真从同一个 parent block 独立执行每条完整路线：买入余额查询 → 必要 reset/approve → swap → 买入余额查询。真实 taker 不补余额；校验 Router、target/spender、sell amount、minimum、native value 和 calldata；模拟前后复核 block hash。Rust 校验 provider rules，链上独立校验管理员登记的路由白名单；结构检查和资金约束由完整合约调用执行。未配置 Router/RPC、unsupported/revert/error 都不能生成可执行结果。

成功 simulation 返回 `blockContext {number, hash, timestamp}`、`simulatedTimestamp`、余额增量及 gas 信息。number 为十六进制区块高度，基础 timestamp 和模拟执行 timestamp 分别报告。没有 API expiresAt 或人工报价 TTL，调用方决定是否对同一交易重新 simulate，或重新竞赛并确认新 minimum。

`Route.deadline` 仅保留上游明确给出的时间期限，未提供时 Router deadline 为 U256::MAX；provider calldata/签名的原生期限仍生效。API 不把仿真成功当作未来成交保证。

context 与 simulator 共用 `RpcClients`，按配置 endpoint 复用 Alloy 客户端，不缓存链状态。详细 wire contract、deadline 和失败分类见 [技术方案](TECHNICAL.md)。

## 8. 不变量

- 金额内部使用 Alloy `U256`，JSON 使用十进制字符串；不使用浮点金额。
- token 是不可信输入；provider 返回的 route 经过参数校验和完整仿真。Router 路由必须命中管理员登记的三元组白名单；直接 ERC20 target、自调用和非法 Holder 形状仍被拒绝。
- 服务端不签名、不广播；provider response 不能把服务状态伪装成链上成功。
- Router 使用精确 spend、临时授权、实际到账 minimum、退款增量、pause 和 reentrancy 保护。
- owner 可 recover Router 持有的 ERC20/native，recover 在暂停期间可用并与 swap 共用锁。owner 转移采用提名/接受两步，历史暂存资产可由管理员提取；接口与事件见 [合约说明](../contracts/README.md)。
- 每条链的 provider 集合只能来自 registry；不能在竞赛中遍历未过滤全集。
- provider 部分失败不能吞掉其他结果，也不能用 mock/fallback 冒充 live 报价。

## 9. 当前实现状态与 review 入口

已实现：provider 派生的多链集合、13 个字符串 provider ID、各 adapter 自有支持矩阵、key-aware `supported_chains()`、反向索引、无 token 白名单输入路径和 13 家 provider route adapter。

已有 13 个独立 ignored live HTTP 测试，以及要求真实 taker 的竞赛重放 `tests/replay_live.rs`。旧版无 taker direct-preview 的历史 live 记录不能证明新的一次请求执行路径；当前快照与未验证边界见 [VERIFICATION.md](VERIFICATION.md)，逐家接入契约见 [PROVIDER_INTEGRATION_GUIDE.md](PROVIDER_INTEGRATION_GUIDE.md)。

排错 review 顺序：`http/rpc` 保留原始 cause → anyhow 添加 `ErrorKind`/操作 context → 仿真失败通过 `Err` 和 `SimulationFailure` typed context 传播 → 失败边界用 tracing 输出完整原因链，并投影为公开 ProviderFailure 或 `ApiError`。没有通用 `Fault` 包装、`SimResult.diagnostic` 或专用日志层；仿真层只判断内部类型，不依赖 API。当前不做内部日志脱敏，但详情仍不得进入 quote 或 API，见 [技术方案第 9 节](TECHNICAL.md#9-内部错误与公开错误)。仿真重点检查卖出资产与 native gas 资金总是直接 override、没有真实余额读取、最低到账/reorg 保护没有为提升成功率而放松。

review 顺序建议：先看各 `src/providers/*.rs` 的 ID、supported chains、凭证要求和原生交易校验，再看 `providers/mod.rs` 的 `ProviderRegistry` 与共享边界，然后按 provider 文件检查各自 endpoint，接着看 `Competitions::create` / `compete` 的调度，最后看 `validate_route`、`simulation`、Solidity 资金不变量及 recover/ownership 权限。`Rule` 和 `Provider::rules()` 用于链下预检，必须与审核后的链上登记同步；外层 Holder 元组不验证其内层身份。测试从 `tests/core.rs`、`tests/providers.rs`、`tests/e2e_local.rs` 和 `contracts/test/MetaRouter.t.sol` 进入。
