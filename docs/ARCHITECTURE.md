# MetaMatch 项目架构与逻辑

本文是当前代码的 review 导航，不是另一个产品规格。代码和测试是事实来源；本文只解释模块之间的关系、关键状态变化和必须保持的不变量。

最后核对：2026-09-11。

## 1. 一句话概览

MetaMatch 是一个单进程、短 TTL、非托管的 Ethereum exact-input 报价竞赛服务：接收一次兑换请求，并发询价 → 在同一 parent block 上仿真 → 只按可验证的净到账排序 → 在 build 时绑定真实 taker、重新询价和仿真 → 返回 unsigned approval/swap 交易。

服务端不持有用户私钥，不签名、不广播；真正的链上交易由调用方钱包发送到 AllowanceHolder，再进入 MetaRouter 和已配置的供应商目标。

## 2. 总体组件关系

```text
                         ┌──────────────────────────┐
                         │        调用方 / 钱包       │
                         │ create → poll → build     │
                         │ sign → broadcast          │
                         └────────────┬─────────────┘
                                      │ HTTP / JSON
                                      ▼
┌──────────────────────────────────────────────────────────────────┐
│ Axum app                                                         │
│  health / capabilities / competition create / polling / build   │
└────────────────────────────┬─────────────────────────────────────┘
                             │
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│ Competitions                                                     │
│ bounded in-memory state · TTL · bearer token · async lifecycle  │
└───────────────┬──────────────────────┬───────────────────────────┘
                │                      │
                ▼                      ▼
       ┌────────────────┐     ┌─────────────────────┐
       │ Provider list  │     │ Context + Simulator │
       │ 0x / 1inch /   │     │ fixed block         │
       │ KyberSwap      │     │ Alloy typed RPC     │
       └───────┬────────┘     └──────────┬──────────┘
               │                        │
               ▼                        ▼
        固定上游 HTTP              server-configured EVM RPC
        reqwest + serde             eth_call / eth_simulateV1

build 返回 unsigned transaction：

调用方 → AllowanceHolder.exec → MetaRouter.execute → provider target
                                      │
                                      └─ 实际余额差、授权清理、退款、minimum output
```

组件装配从 [src/main.rs#L4-L14](../src/main.rs#L4-L14) 开始，HTTP 路由和服务状态在 [src/app.rs#L18-L52](../src/app.rs#L18-L52)，生产依赖在 [src/competitions.rs#L43-L62](../src/competitions.rs#L43-L62) 创建。

## 3. 运行时与依赖方向

依赖方向应保持单向：

```text
app
 └─ competitions
     ├─ domain
     ├─ providers ── http
     ├─ rpc
     ├─ simulation ── execution ── domain
     └─ config ── domain

contracts/ 是独立的链上边界，由 execution 生成 calldata，由钱包发送后执行。
```

| 模块 | 主要职责 | review 时不应放入的内容 |
| --- | --- | --- |
| `src/main.rs` | 读取配置、绑定 listener、优雅退出 | 业务规则、供应商逻辑、签名 |
| `src/app.rs` | Axum 路由、请求解析、错误 envelope、body limit | 竞赛状态细节、上游协议细节 |
| `src/config.rs` | 环境变量/JSON 配置、Ethereum 资产与 route allowlist | 请求级用户配置、用户 URL、任意 calldata |
| `src/domain.rs` | 输入/领域类型、整数金额、价格、排序 | HTTP 连接、RPC 调用、线程管理 |
| `src/competitions.rs` | 竞赛生命周期、并发询价、认证、build 编排 | ABI offset、供应商字段解析、通用 RPC 细节 |
| `src/providers.rs` | 三家上游适配、响应归一化和 route 边界校验 | 签名、广播、持久化 |
| `src/http.rs` | 出站 HTTP、超时、响应大小、JSON 解码 | 供应商业务判断、交易执行判断 |
| `src/rpc.rs` | Alloy provider、EVM RPC 抽象、block/price context | route allowlist、最终余额不变量 |
| `src/simulation.rs` | 顺序调用仿真、余额差、授权、gas、reorg 检查 | 供应商响应 schema、HTTP 路由 |
| `src/execution.rs` | route 校验、ERC-20 ABI、Holder/MetaRouter calldata | RPC 请求、异步状态管理 |
| `contracts/src/MetaRouter.sol` | 链上最终资金与调用边界 | 把服务端配置或用户输入当作信任来源 |

## 4. 启动与配置逻辑

### 4.1 启动顺序

1. `main` 调用 `load_config_from_env`。
2. 配置由环境变量和可选 `CONFIG_PATH` JSON 合并得到。
3. 当前只创建一个 chain：Ethereum `chainId=1`。
4. `create_app` 创建 `Competitions`；没有注入测试 service 时，使用 `Services::production`。
5. 绑定 `HOST:PORT`，以 Axum 启动 HTTP server。
6. 收到 Ctrl-C 后调用 `App::close`，设置所有竞赛的取消标记，再结束服务。

代码位置：[src/config.rs#L44-L95](../src/config.rs#L44-L95)、[src/main.rs#L5-L20](../src/main.rs#L5-L20)。

### 4.2 配置边界

默认配置和可调范围：

| 配置 | 默认值 | 作用 |
| --- | ---: | --- |
| `HOST` | `127.0.0.1` | 监听地址 |
| `PORT` | `3000` | 监听端口 |
| `QUOTE_TTL_MS` | `60000` | 竞赛有效期，10 秒至 120 秒 |
| `PROVIDER_TIMEOUT_MS` | `6000` | 上游/RPC 操作超时，100 毫秒至 30 秒 |
| `ETHEREUM_RPC_URL` | 空 | Ethereum server-side RPC；需要 state override 和 `eth_simulateV1` |
| `CONFIG_PATH` | 空 | 可选 JSON，设置 router、rules、balanceSlots |
| provider key | 空 | 缺失时该供应商不触网，报价为 `unavailable` |

服务端容量在 `Config` 中固定为 `max_competitions=500`、`max_active=20`。链配置只接受 JSON 顶层键 `"1"`；路由规则按 provider 存储 `(target, spender, selector)`。

资产白名单是代码内的 ETH、WETH、USDC，定义在 [src/config.rs#L113-L144](../src/config.rs#L113-L144)。配置中的 router 不能是 AllowanceHolder；非法 RPC scheme、地址、selector 或未知 provider 会在加载时失败，而不是运行时静默降级。

## 5. HTTP API 与请求生命周期

路由装配在 [src/app.rs#L34-L52](../src/app.rs#L34-L52)，body 上限为 16 KiB。

| 方法 | 路径 | 逻辑 | 认证 |
| --- | --- | --- | --- |
| GET | `/health` | 进程与存储类型探针 | 否 |
| GET | `/v1/capabilities` | 返回 chain、资产、provider 配置状态 | 否 |
| POST | `/v1/competitions` | 校验输入并异步创建竞赛 | 否 |
| GET | `/v1/competitions/{id}` | 读取快照并排序报价 | Bearer token |
| POST | `/v1/competitions/{id}/quotes/{quote_id}/build` | 重新报价、仿真并生成 unsigned tx | Bearer token |

所有业务错误最终由 `AppError` 转成 `{"error":"CODE"}`；HTTP status 来自 `Fault.http_status`，不会把上游原始 body 返回给调用方。实现见 [src/app.rs#L55-L134](../src/app.rs#L55-L134)。

### 5.1 创建竞赛

```text
POST /v1/competitions
        │
        ▼
parse_input
  ├─ deny unknown fields
  ├─ chainId == 1
  ├─ sellToken/buyToken 是地址、不同，buyToken 不是 native sentinel
  ├─ sellAmount 是正的十进制 U256 字符串
  ├─ slippageBps ∈ [1, 500]
  └─ taker 不能是保留地址或低地址
        │
        ▼
Competitions::create
  ├─ 清理过期项
  ├─ 检查 active / total capacity
  ├─ 检查 chain asset whitelist
  ├─ 检查 taker 与 router 冲突
  ├─ 保存 Competition + token + expiry
  ├─ active += 1
  └─ spawn(run)
        │
        └─ 立即返回 202 + id + accessToken + expiresAt
```

输入解析在 [src/domain.rs#L41-L128](../src/domain.rs#L41-L128)，创建和异步启动在 [src/competitions.rs#L85-L140](../src/competitions.rs#L85-L140)。

### 5.2 轮询与排序

调用方持有创建响应中的 access token，轮询快照直到 `status=complete`。读取流程是：

1. `access` 先 sweep 过期竞赛。
2. 按 id 读取 state。
3. 用固定时间比较 token，避免普通字符串比较泄露早停差异。
4. `rank` 复制报价并排序。
5. `recommendedQuoteId` 只指向仍未过期、仿真成功、`netOutput` 非空的报价。

排序优先级：

```text
verified 且未过期、有 netOutput
    ↓ 按 netOutput 降序
    ↓ provider id 作为稳定 tie-breaker
非 verified / 过期 / fee unknown
    ↓ provider id
```

读取、token 校验和排序入口在 [src/competitions.rs#L142-L159](../src/competitions.rs#L142-L159)、[src/competitions.rs#L293-L309](../src/competitions.rs#L293-L309)、[src/domain.rs#L328-L375](../src/domain.rs#L328-L375) 和 [src/domain.rs#L383-L416](../src/domain.rs#L383-L416)。

### 5.3 Build 生命周期

build 不是把第一次 quote 原样返回，而是一次新的验证流程：

```text
带 token 的 build 请求
        │
        ▼
读取竞赛与 quote
        │
├─ router 必须存在
├─ taker 非保留地址、不能等于 router
├─ 若创建时绑定 taker，build taker 必须一致
├─ quote 必须 ready 且未过期
├─ acceptedMinBuyAmount >= 首轮 quote.minBuyAmount
└─ build 并发计数未超过上限
        │
        ▼
并发重新获取 context + provider route
        │
├─ require_unified route 校验和 route allowlist
├─ route.buyAmount >= accepted minimum
├─ 最终 minimum = max(accepted, 新 route.minBuyAmount)
└─ actual=true 重新仿真
        │
├─ 仿真必须 success 且 funding=actual
├─ route 和竞赛 expiry 至少还剩约 2 秒
└─ 返回 approvals + transaction + context + simulation
```

实现分为入口校验 [src/competitions.rs#L161-L228](../src/competitions.rs#L161-L228) 和重新报价/仿真 [src/competitions.rs#L230-L290](../src/competitions.rs#L230-L290)。

## 6. 领域模型

### 6.1 主要类型

| 类型 | 含义 | 生命周期 |
| --- | --- | --- |
| `Input` | 已通过边界校验的用户兑换请求 | 竞赛创建时固定 |
| `Chain` | Ethereum 资产、RPC、router、allowlist、balance slot | 配置加载时固定 |
| `Context` | parent block number/hash、timestamp、gas price、价格 | 一次竞赛/一次 build 固定 |
| `Route` | 某 provider 的可执行 route 和执行元数据 | 每次询价生成，短期有效 |
| `Simulation` | success/reverted/unsupported/error 结果 | 跟随 quote 或 build 返回 |
| `Quote` | route 的公开快照、仿真、净到账和状态 | 竞赛内存状态 |
| `Tx` | `to/data/value` unsigned transaction | 只作为钱包输入 |
| `Fault` | 内部错误码和 HTTP status | 不携带原始上游 body |

定义集中在 [src/domain.rs#L19-L328](../src/domain.rs#L19-L328)。金额和 gas 在内部使用 `U256`；JSON 的金额保持十进制字符串。`parse_uint` 限制无前导零、纯数字和最大 78 位，防止把小数、科学计数法或 JS Number 语义带入交易。

### 6.2 报价、仿真和推荐的区别

这三个概念不能混用：

```text
provider response
    └─ Route：只代表上游返回了一个候选执行
          └─ Simulation：代表固定区块下通过了仿真检查
                └─ netOutput：在 gas fee 和价格均已知时才可计算
                      └─ recommendedQuoteId：只有以上条件同时成立才出现
```

`Simulation::Success` 还区分 `funding=overridden` 和 `funding=actual`。preview 可以使用受控 state override；build 必须使用真实 taker 余额，因此 build 只接受 `is_actual_success()`。

## 7. 上游 HTTP 与 Provider 适配

### 7.1 公共 HTTP 边界

`HttpClient` 是可注入的出站 HTTP 抽象；生产实现是 reqwest，测试使用 fixture。生产 client：

- 禁止重定向；
- 每个请求有 timeout；
- 响应 body 超过 2 MiB 立即失败；
- 非 2xx 只暴露 `UPSTREAM_RATE_LIMITED` 或 `UPSTREAM_HTTP_ERROR`；
- 空 body、非法 JSON、schema 不匹配映射为明确错误；
- 不把原始响应 body 写入 `Fault`。

代码见 [src/http.rs#L8-L135](../src/http.rs#L8-L135)。

### 7.2 三家 Provider 的责任

所有实现满足 `Provider` trait：`id()`、`enabled()`、`quote(input, chain, sender)`，定义在 [src/providers.rs#L14-L39](../src/providers.rs#L14-L39)。

| Provider | 请求 | 关键归一化/校验 |
| --- | --- | --- |
| 0x | allowance-holder quote endpoint | liquidity、sell/buy/min 数量、allowance target/Holder、transaction.to/data/value |
| 1inch | Classic v6.1 swap endpoint | dst amount、禁止非空 state override、固定 1inch spender、transaction.to |
| KyberSwap | 先 `/routes`，再 `/route/build` | route summary amountIn、router 地址前后一致、amountOut、native/ERC20 transaction value |

Provider 层只返回统一 `Route`。任何金额不一致、非正数量、unexpected target/spender、非法 calldata、非法 value、过期或不支持的 override 都在进入 simulation 前失败。适配器实现见 [src/providers.rs#L45-L298](../src/providers.rs#L45-L298)。

缺少 key 时 `enabled=false`，`Competitions::run_live` 不调用上游，直接留下 `unavailable` quote；一个 provider 的失败不会取消其他 provider，见 [src/competitions.rs#L374-L481](../src/competitions.rs#L374-L481)。

## 8. Block Context、价格与 Alloy RPC

### 8.1 Context 获取

`ContextSource::get` 要求配置了 RPC，然后通过 `RpcFactory` 创建 Alloy provider，并行获取：

1. `eth_chainId`
2. latest block（number/hash/timestamp）
3. gas price
4. CoinGecko ETH/USDC USD 价格

chain id 不匹配时返回 `RPC_CHAIN_MISMATCH`。价格缓存 30 秒；价格源失败不会伪造价格，结果是 `nativeUsd/buyUsd` 缺失，进而 `netOutput=null`。

代码见 [src/rpc.rs#L174-L289](../src/rpc.rs#L174-L289)。

### 8.2 Alloy 边界

生产 RPC 已通过 Alloy typed provider：

- `ProviderBuilder` + reqwest transport；
- typed block、chain id、gas price、code、balance；
- typed `TransactionRequest`；
- typed `StateOverride`；
- typed `SimulatePayload` / `SimulatedBlock`。

`EvmRpc` trait 是很薄的测试替换边界，不是第二个 RPC 协议。标准 RPC 错误在 [src/rpc.rs#L150-L162](../src/rpc.rs#L150-L162) 统一成 `RPC_METHOD_UNSUPPORTED`、`RPC_INVALID_RESPONSE` 或 `RPC_CALL_FAILED`。

## 9. Simulation 逻辑

仿真入口是 [src/simulation.rs#L56-L328](../src/simulation.rs#L56-L328)。它按以下顺序工作：

```text
1. execution::swap_transaction 先校验 route 并构造最终 transaction
2. RPC 未配置 → unsupported
3. 检查 context 的 parent block hash
4. 若有 router，检查 router 在该 block 有 code
5. 读取 taker native balance，检查 value + gas 资金
6. ERC20 sell：读取余额；preview 必要时构造 balance-slot override
7. 用 eth_call 验证 override 后的 balanceOf
8. 读取 allowance；不足时生成 reset-to-zero + exact approval
9. 构造单个 eth_simulateV1 block：
   buy balance before → approvals → swap → buy balance after
10. 检查返回 block/call 数量和每个 call status
11. 检查 approval 返回值
12. 用买币余额 after-before 得到 bought amount
13. 检查 bought >= minimum
14. 汇总 approval + swap gas，并对 actual taker 做 gas 余额保护
15. 再次检查 parent block hash，发现 reorg 则拒绝验证
16. 返回 Simulation::Success
```

关键不变量：

- preview 的 override 只用于受控测试资金，不等于真实 taker 有钱；
- build 使用真实 taker，且不足卖币余额或 gas 余额时失败；
- 买币结果是余额增量，不是 provider 的宣传 `buyAmount`；
- call 数量必须是 `approvals + 3`；
- approval 的非空返回值必须是 `true`；
- `after < before`、低于 minimum、reorg、RPC unsupported 都不能被标为 success；
- gas 只从 approval 和 swap call 汇总，不把 balanceOf 读调用计入 gas。

state override 和余额 slot 逻辑在 [src/simulation.rs#L99-L202](../src/simulation.rs#L99-L202)，顺序仿真与结果检查在 [src/simulation.rs#L203-L328](../src/simulation.rs#L203-L328)。

## 10. Route 校验与 unsigned transaction

### 10.1 Route 校验

`validate_route` 是 provider route 进入仿真和 build 的共同门槛：[src/execution.rs#L13-L57](../src/execution.rs#L13-L57)。它检查：

- sell amount 必须与用户 input 完全相等；
- buy amount 不低于 min buy amount，且 minimum 为正；
- route 未过期；
- native sell 时 transaction value 必须等于 sell amount；ERC20 sell 时必须为 0；
- calldata 是合法 hex、至少 4 bytes、最大约 256 KiB；
- 配置 router 时，`(tx.to, spender, selector)` 必须命中该 provider 的 allowlist。

### 10.2 Direct preview 与 unified build

| 模式 | `chain.router` | transaction 结果 | 适用阶段 |
| --- | --- | --- | --- |
| direct preview | 空 | 原始 provider tx | 只能预览，build 明确拒绝 |
| unified | 已配置 | `Holder.exec(router, sellToken, sellAmount, router, MetaRouter.execute(...))` | build 和最终执行 |

`swap_transaction` 在 [src/execution.rs#L59-L100](../src/execution.rs#L59-L100) 使用 `alloy-sol-types::sol!` 编码 ABI，避免手写 offset。统一交易的外层 `to` 是固定 `HOLDER`；内层把 provider target、spender、value、data 和 deadline 交给 MetaRouter。

## 11. 链上 MetaRouter 边界

服务端校验不是最终安全边界，最终边界在 [contracts/src/MetaRouter.sol#L40-L176](../contracts/src/MetaRouter.sol#L40-L176)：

1. 构造时 owner 非零、AllowanceHolder 必须有 code。
2. route permission 由 owner 设置，按 `keccak256(target, spender, selector)` 存储。
3. nested AllowanceHolder 只能使用经过特殊限制的 `exec` tuple，不能把 Settler 当作普通 spender。
4. `execute` 只能由 AllowanceHolder 调用，支持 pause 和 nonReentrant。
5. 检查 deadline、canonical calldata 长度和末尾 forwarded taker。
6. 检查 target/spender/selector allowlist、sell/buy token、native/ERC20 value 规则。
7. ERC20 通过 Holder 精确拉取 sellAmount；Router 先清零再设置临时 allowance。
8. 外部调用结束后清零 allowance，只退还本次新增余额。
9. 买币以 taker 的余额增量检查 minimum，历史余额不能满足本次 minimum。
10. 最后处理 native refund，并再次检查最终 taker balance。

服务端 `execution.rs` 和 Solidity 合约必须共同保持这条边界：服务端不能通过扩大 allowlist、修改 calldata 或降低 minimum 来绕过合约检查。

## 12. 状态、并发与生命周期

### 12.1 内存状态

`Competitions` 持有：

- `items: Arc<Mutex<HashMap<Uuid, Arc<CompetitionHandle>>>>`：竞赛索引；
- `active: AtomicUsize`：正在运行的竞赛数；
- `builds: AtomicUsize`：正在执行的 build 数；
- 每个 `CompetitionHandle` 内有 `Mutex<Competition>` 和 `AtomicBool cancel`。

状态模型是有界单进程内存，不提供跨进程共享、持久化或恢复。过期清理由 create/get/build 前的 `sweep` 触发；关闭时 drain 全部 item 并设置 cancel，见 [src/competitions.rs#L311-L350](../src/competitions.rs#L311-L350)。

### 12.2 并发原则

- 供应商询价通过 `join_all` 并发执行；单个 provider 失败只影响自己的 quote。
- context 的 chain/block/gas/price 查询并发执行。
- build 的 context 和 provider re-quote 并发执行，但最终 simulation 在 route/context 准备好后执行。
- 每个出站请求和主要阶段有 timeout。
- 锁只保护内存状态，网络 await 不在 Competition state mutex 内进行。

review 时重点检查：新增 await 是否持有 `items` 或 `Competition` mutex；新增 provider 是否能够取消整场竞赛；新增容量计数是否在所有 early return 路径释放。

## 13. 错误和证据语义

错误状态不是装饰字段，调用方会据此决定是否继续：

| 状态/错误 | 含义 | 是否可推荐/执行 |
| --- | --- | --- |
| `ready` + simulation success | route 已归一化且仿真通过 | 只有 `netOutput` 非空才可推荐 |
| `unavailable` | provider 未配置或未提供服务 | 否 |
| `error` | provider/RPC/解析/timeout 等失败 | 否 |
| `reverted` | 仿真执行或资金不变量失败 | 否 |
| `unsupported` | RPC method、slot、router 或能力缺失 | 否 |
| `netOutput=null` | fee 或价格缺失 | 否，不能用 quotedAmount 替代 |
| `ROUTER_NOT_CONFIGURED` | 只有 direct preview，没有统一执行 router | build 否 |

`safe_error` 当前只返回内部错误码，不返回上游原文：[src/competitions.rs#L540-L564](../src/competitions.rs#L540-L564)。任何新错误都应明确选择 HTTP status，并确认不会让失败报价进入推荐或 build。

## 14. 测试与 review 证据

### 14.1 单元/fixture

单元测试与实现位于同一模块，当前覆盖：

- `domain`：整数金额、输入拒绝、保留 taker；
- `config`：单链默认值、非法 RPC/config；
- `http`：URL 参数编码、非法 JSON、上游错误清洗；
- `providers`：响应归一化、地址/value、数量格式；
- `rpc`：typed provider context、RPC error mapping；
- `simulation`：余额差、approval、unsupported、reorg；
- `execution`：route target/value、Holder/MetaRouter calldata；
- `competitions`：provider failure isolation、竞赛完成；
- `app`：HTTP polling 生命周期与鉴权。

### 14.2 本地 Anvil E2E

`tests/e2e_local.rs` 是独立 ignored 测试：

1. 启动本地 Anvil chainId 1；
2. 写入 mock Holder code，部署测试 token、provider、router；
3. 设置 router route permission 和测试余额；
4. 通过 HTTP 创建竞赛并 polling；
5. build 得到 approval，发 approval 后再次 build；
6. 发送 swap transaction；
7. 检查 taker 收到 200 buy token，router 对 provider 的 allowance 为 0。

测试入口：[tests/e2e_local.rs#L310-L354](../tests/e2e_local.rs#L310-L354)、主断言：[tests/e2e_local.rs#L507-L629](../tests/e2e_local.rs#L507-L629)。它证明本地 mock EVM 闭环，不证明正式 Holder、正式 Router、真实供应商或主网执行。

### 14.3 Solidity

`contracts/test/MetaRouter.t.sol` 覆盖 caller authorization、allowlist、exact spend、approval cleanup、minimum output、历史余额、refund、nested Holder 和 reentrancy，并包含 fuzz。合约验证命令以 Foundry 为准。

## 15. Review 建议阅读顺序

按下面顺序阅读，能最快建立“输入如何变成可执行交易”的心智模型：

1. [docs/PRODUCT.md](PRODUCT.md)：确认当前只支持的产品范围和非目标。
2. [src/app.rs](../src/app.rs)：确认外部接口、认证和错误 envelope。
3. [src/domain.rs](../src/domain.rs)：确认金额、状态和排序定义。
4. [src/competitions.rs](../src/competitions.rs)：跟踪 create → poll → build 编排。
5. [src/providers.rs](../src/providers.rs)：逐家核对上游字段如何归一化为 `Route`。
6. [src/execution.rs](../src/execution.rs)：核对 route allowlist 和 ABI calldata。
7. [src/rpc.rs](../src/rpc.rs)：确认 block context 和 Alloy RPC 映射。
8. [src/simulation.rs](../src/simulation.rs)：核对顺序调用、余额差和 reorg。
9. [contracts/src/MetaRouter.sol](../contracts/src/MetaRouter.sol)：核对链上最终不变量。
10. [tests/e2e_local.rs](../tests/e2e_local.rs) 与 Foundry tests：确认 review 结论有哪一层证据支持。

## 16. Review Checklist

### API 与领域

- [ ] 新字段是否同时更新 serde/domain、测试和产品文档？
- [ ] 是否仍使用十进制字符串和 `U256`，没有浮点金额？
- [ ] 无效/过期/fee unknown quote 是否绝不会成为推荐？
- [ ] accepted minimum 是否单调不降低？

### Provider 与 RPC

- [ ] 是否只调用固定服务端 URL？
- [ ] provider 返回的 target、spender、selector、value、sell amount 是否全部重新校验？
- [ ] 是否保持响应大小、timeout、错误清洗和 provider failure isolation？
- [ ] simulation 是否始终使用同一个 block number/hash，并在前后检查 reorg？
- [ ] 是否区分 actual funding 与 override funding？

### 执行与资金

- [ ] route allowlist 是否仍是显式配置，而非从用户或上游响应自动学习？
- [ ] ERC20 是否 exact spend、临时授权清零、只退本次增量？
- [ ] minimum 是否基于 taker 的实际买币余额增量？
- [ ] 是否没有增加任意 target、delegatecall、rescue 或 Settler approval？
- [ ] 服务端是否仍不持有 key、不签名、不广播？

### 生命周期与证据

- [ ] 新增 await 是否持有 mutex？
- [ ] 所有计数器和 TTL early return 是否可释放？
- [ ] fixture、本地 Anvil、fork、live provider 是否被清楚区分？
- [ ] 是否运行 Cargo、Foundry 和必要的 ignored E2E 质量门？

## 17. 当前明确不属于架构的能力

当前不要从代码中推断以下能力已经存在：数据库或 Redis、多副本、分布式限流、持久审计、provider 熔断、RPC 容灾、其他链、跨链、intent、平台抽成、真实主网 fork、正式 Holder/Router 部署、外部审计和生产交易广播。这些如果未来加入，应先扩大产品范围，再新增架构决策和对应证据，不要用空 trait 或配置字段提前占位。

