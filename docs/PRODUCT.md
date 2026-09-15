# MetaMatch 产品需求 v1

日期：2026-09-15。状态：v1 运行时契约与 13 个 provider adapter 已落库；公开 provider 已执行 live quote，仍缺必需 key、生产 RPC、正式 Router 和主网执行验证。

## 1. 产品目标

MetaMatch 是一个非托管的 EVM 同链 exact-input swap 报价竞赛服务。用户提交链、卖出 token、买入 token、数量和滑点；服务按照 provider 对链的实际支持能力建立 `chain -> provider list` 反向索引，向该链上能参与的 provider 并发询价，统一校验、仿真、排序，并返回由用户钱包自行签名和广播的 unsigned transaction。

服务端不持有私钥，不签名、不广播，不替用户扩大或降低 minimum output。

## 2. v1 范围

- 只处理同一条 EVM 链上的交换；不做跨链桥、非 EVM、intent、gasless 或交易回执代理。
- provider 集合固定来自 [Matcha Meta DEX Aggregation](https://0x-docs.gitbook.io/matcha-meta/core-concepts/dex-aggregation) 页面，不由部署配置增删。
- 链集合是上述 provider 支持链的并集；启动时不配置链 allowlist，也不配置 provider enable/disable 列表。
- 配置不维护 token 白名单、token 列表或 token metadata。API 收到的 token 地址只做地址格式和交易不变量校验，然后透传给 provider。
- provider 自己实现 `supported_chains()`。需要 access key 的 provider 在没有对应环境变量时返回空集合；不需要 access key 的 provider 不因缺少 key 被排除。配置了 optional key 时仍传给对应 adapter。其余 provider 一律参与竞赛，失败只影响自身报价。
- 请求带 `chainId`，服务必须拒绝当前 provider 实例支持链并集之外的链；`buyToken` 暂不允许使用 native sentinel，保持现有资金模型简单。
- `taker` 必填并绑定收款人与返回交易。竞赛仿真始终覆盖本次卖出资产和 native gas 资金，不读取真实钱包余额；缺少 RPC 或执行 Router 返回明确失败。

## 3. provider 与链

provider ID（共 13 个）：`0x`、`1inch`、`barter`、`bebop`、`enso`、`hyperBloom`、`kyber`、`liquidSwap`、`odos`、`oogaBooga`、`okx`、`openOcean`、`velora`。

当前 13 个 provider adapter 的静态能力矩阵并集共有 17 条链：Ethereum `1`、Optimism `10`、BNB Smart Chain `56`、Unichain `130`、Polygon `137`、Monad `143`、Sonic `146`、HyperEVM `999`、Mantle `5000`、Base `8453`、Plasma `9745`、Arbitrum One `42161`、Avalanche `43114`、Linea `59144`、Berachain `80094`、Blast `81457`、Scroll `534352`。这只是当前代码快照，不是第二份 chain catalog；运行时链集合严格由本次实例的 `supported_chains()` 并集生成，因此会随必需 key 是否存在而变化。

运行时代码中的 provider-to-chain 矩阵和 provider path slug 分别位于 [src/providers/](../src/providers/) 的对应 adapter；[src/chains.rs](../src/chains.rs) 只按 provider 给出的 chain ID 构造运行时链并解析 RPC。provider 名单来自 Matcha，链矩阵和接入参数按 2026-09-13 各家当前官方 API 重核；细节见 [PROVIDER_INTEGRATION_GUIDE.md](PROVIDER_INTEGRATION_GUIDE.md)。Monad 已从历史 testnet `10143` 更新为主网 `143`。

## 4. 用户流程

1. `GET /v1/capabilities` 返回本次启动的 provider 支持链并集和每条链真正能参与的 provider ID；不返回 RPC URL。
2. `POST /v1/competitions` 提交：

   ```json
   {
     "chainId": 8453,
     "sellToken": "0x…",
     "buyToken": "0x…",
     "sellAmount": "1000000000000000000",
     "slippageBps": 30,
     "taker": "0x…"
   }
   ```

3. 服务在 `COMPETITION_TIMEOUT_MS` 总预算内完成竞赛并返回 `200`。先并发获取并校验全部 provider route；route 阶段全部结算后获取一次公共 block context，再基于该 context 并发 simulation 所有有效 route。总预算不会按阶段重置；route 阶段若耗尽预算，本轮无法继续完成 context 或 simulation。
4. 仿真先覆盖 taker 的卖出资产和 native gas 资金，再执行完整的「买入 token 余额查询 → 必要 approvals → swap → 余额查询」。仅 route 与完整仿真都成功的结果成为 `Quote`，按模拟余额增量 `simulation.boughtAmount` 降序排序；金额相同按 provider ID 排序。Gas 单独展示，未支持的费用保持 `null`，不做伪造价格换算。
5. 成功结果包含该次仿真对应的 `approvals[]` 和 `transaction`，`simulation.funding` 固定为 `overridden`。用户选择一条并依序执行；发送前自行确认真实余额足够。失败 provider 的诊断信息单独放入 `failures`，不能执行。
6. API 不返回 `expiresAt`，不维护报价 TTL。每条成功 simulation 返回基础区块 `blockContext.number/hash/timestamp` 和 `simulatedTimestamp`。调用方决定是否重做 simulation 或重新竞赛；后者可能产生新的最低到账，需调用方重新确认，不会自动替换用户已接受的交易。
7. provider 返回的真实报价期限及 calldata/签名内的 deadline 仍生效。没有上游期限时，服务不额外增加人工时间限制。链上最低到账保护继续执行，仿真不保证未来成交。

## 5. 最小 API

| 方法 | 路径 | 作用 |
| --- | --- | --- |
| GET | `/health` | 进程健康状态，无竞赛存储 |
| GET | `/v1/capabilities` | 链与 provider 反向索引发现，不返回 token 列表 |
| POST | `/v1/competitions` | 一次请求完成竞赛，返回排序后的仿真与交易 |

响应为 `{id, input, quotes, failures}`，`id` 仅用于诊断关联。`quotes` 只包含成功的 `Quote {route, simulation, approvals, transaction, latencyMs}`，所有字段必填；`simulation` 为仅含成功数据的 `SimulationSuccess`，没有 status/error/reason。完整归一化 route 包含 `provider/buyAmount/minBuyAmount/sellAmount/spender/tx`，以及上游明确提供时才存在的原生 `deadline`（Unix 秒，不是 API TTL）。钱包执行 quote 顶层的 `approvals/transaction`，不直接签署 provider 的 `route.tx`。

`failures` 包含 `provider/status/latencyMs/error`，存在明确仿真失败时包含失败专用 `simulation {status, reason}`，其 status 只能是 reverted/unsupported/error。失败对象没有 route 或交易字段。两个数组始终存在；全成功时 failures 为空，全失败时 quotes 为空。成功项按模拟到账量排序，失败项按 provider ID 排序。

本轮字段迁移：`quote.provider → quote.route.provider`、`quote.quotedAmount → quote.route.buyAmount`、`quote.minBuyAmount → quote.route.minBuyAmount`；移除 quote.status/error 和成功 simulation.status，调用方无需在 quotes 内筛选成功项。

这是接口破坏性变更：移除 GET competition 和 POST build 路由、access token、生命周期状态、quote ID、recommended quote ID 及 `expiresAt`；旧路由返回 404。`taker` 从可选变为必填。重新仿真现有交易由调用方通过钱包/RPC 完成，本次没有新增任意 calldata 仿真接口。

## 6. 配置原则

- `COMPETITION_TIMEOUT_MS` 是一次请求的总工作预算（默认 6000，范围 100–30000）；移除旧 `PROVIDER_TIMEOUT_MS`、`QUOTE_TTL_MS`。并发请求容量保留，完成/失败/取消后自动释放，不存储竞赛快照。

- 不配置链集合，也不维护 `CHAIN_CATALOG`：运行时链集合来自当前 provider 实例的 `supported_chains()` 并集。
- 不配置 provider 集合：代码内 provider 注册表始终创建全部 13 个 provider。
- 没有 token allowlist；未知 token 由 provider 和链上仿真决定是否可交易。可选 `BALANCE_SLOTS` 提供余额 mapping 基础槽位，独立 resolver 在配置和按 chain/token 建立的进程缓存缺失时，对两个固定假地址并行调用 `eth_createAccessList`。所有 simulation 消费 base 并直接覆盖本次卖出余额；它不读取真实余额，也不改变 provider 参与资格。
- RPC 是运行基础设施，不是产品能力开关。对并集中的每个 chain ID，解析优先级为 `RPC_URL_<chainId>`、Ethereum 旧别名 `ETHEREUM_RPC_URL`、`ALCHEMY_API_KEY` 自动生成的官方链 endpoint。没有任何来源时链仍保留在 capabilities，但不能仿真；capabilities 只返回是否已配置 RPC，不返回 URL。
- provider access key 使用各 provider 原生环境变量，不抽象成 credential trait。需要 key 的 provider 缺少任一必需字段时不进入索引；完整名称见 [src/config.rs](../src/config.rs)。

## 7. MetaRouter 路由与管理权限

- Router 维护管理员登记的 target/spender/selector 白名单，默认拒绝。Rust provider rules 预检与链上登记均须通过，不得因上游返回某个目标而自动加白；provider 原生响应校验、完整仿真和金额/到账保护继续执行。白名单只约束外层元组，嵌套 Holder 的内层身份不因此得到验证。
- 为保护历史暂存资产，Router 拒绝把 ERC20 合约直接作为路由目标，保留目标代码、禁止自调用、Holder 调用形状等结构检查。带 `balanceOf` 接口的 vault/其他入口也可能被拒绝，具体判定见合约文档。
- 管理员使用现有 `owner` 名称。`recoverToken(token, recipient, amount)` 可提取 Router 持有的 ERC20/native 指定数量，在暂停期间可用；只有当前 owner 有权限，且不能在 swap/recover 回调中重入资金操作。
- 管理员转移需要两步：原 owner 提名、新 pendingOwner 接受。接受前原 owner 继续负责 pause/recover/白名单管理，接受后权限转交，旧 owner 失权。
- 管理员可以提取历史余额及误转资产，但 recover 不会从用户钱包拉款。Router 不作为存款保管地址；管理操作由管理员账户直接调用合约，HTTP 后端不新增管理员接口或签名能力。

接口、事件、错误和迁移说明见 [contracts/README.md](../contracts/README.md)。生产运行时的 Router 地址接入、provider 的经审核 rules 与链上登记仍待完成；恢复白名单不代表已完成正式部署。

## 8. 明确不在 v1

数据库、多副本状态、分布式限流、跨链、非 EVM、token registry、价格/decimals 目录、provider 熔断、智能路由重写、平台抽成、用户身份系统、SSE、交易广播和回执存储。

## 9. 当前实现状态

v1 的 provider 派生链集合、13 个字符串 provider ID、`supported_chains` 规则、access-key 过滤、反向索引和 13 家真实 HTTP adapter 已实现。每家 adapter 都把 provider 原生 quote 转为统一 `Route`，并拒绝金额、上游原生 deadline、target、spender、calldata 或 native value 不一致的响应。当前仍不应宣称真实 key、生产 RPC、正式 Router 或主网执行已经验证。

实现和验证状态以 [VERIFICATION.md](VERIFICATION.md) 和 [RUST_REWRITE_LOG.md](RUST_REWRITE_LOG.md) 为准。
