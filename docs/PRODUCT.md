# MetaMatch 产品需求 v1

日期：2026-09-13。状态：v1 运行时契约与 13 个 provider adapter 已落库；公开 provider 已执行 live quote，仍缺必需 key、生产 RPC、正式 Router 和主网执行验证。

## 1. 产品目标

MetaMatch 是一个非托管的 EVM 同链 exact-input swap 报价竞赛服务。用户提交链、卖出 token、买入 token、数量和滑点；服务按照 provider 对链的实际支持能力建立 `chain -> provider list` 反向索引，向该链上能参与的 provider 并发询价，统一校验、仿真、排序，并返回由用户钱包自行签名和广播的 unsigned transaction。

服务端不持有私钥，不签名、不广播，不替用户扩大或降低 minimum output。

## 2. v1 范围

- 只处理同一条 EVM 链上的交换；不做跨链桥、非 EVM、intent、gasless 或交易回执代理。
- provider 集合固定来自 [Matcha Meta DEX Aggregation](https://0x-docs.gitbook.io/matcha-meta/core-concepts/dex-aggregation) 页面，不由部署配置增删。
- 链集合是上述 provider 支持链的并集；启动时不配置链 allowlist，也不配置 provider enable/disable 列表。
- 配置不维护 token 白名单、token 列表或 token metadata。API 收到的 token 地址只做地址格式和交易不变量校验，然后透传给 provider。
- provider 自己实现 `supported_chains()`。需要 access key 的 provider 在没有对应环境变量时返回空集合；不需要 access key 的 provider 不因缺少 key 被排除。配置了 optional key 时仍传给对应 adapter。其余 provider 一律参与竞赛，失败只影响自身报价。
- 请求带 `chainId`，服务必须拒绝 catalog 之外的链；`buyToken` 暂不允许使用 native sentinel，保持现有资金模型简单。
- quote 阶段可因缺少 RPC 或执行 Router 而成为 preview/unavailable；build 必须重新报价和仿真，不把 preview 当作可执行成功。

## 3. provider 与链

provider ID（共 13 个）：`0x`、`1inch`、`barter`、`bebop`、`enso`、`hyperBloom`、`kyber`、`liquidSwap`、`odos`、`oogaBooga`、`okx`、`openOcean`、`velora`。

链 catalog（共 17 个）：Ethereum `1`、Optimism `10`、BNB Smart Chain `56`、Unichain `130`、Polygon `137`、Monad `143`、Sonic `146`、HyperEVM `999`、Mantle `5000`、Base `8453`、Plasma `9745`、Arbitrum One `42161`、Avalanche `43114`、Linea `59144`、Berachain `80094`、Blast `81457`、Scroll `534352`。

运行时代码中的 provider-to-chain 矩阵分别位于 [src/providers/](../src/providers/) 的对应 adapter；链名称和上游 path slug 在 [src/chains.rs](../src/chains.rs)。provider 名单来自 Matcha，链矩阵和接入参数按 2026-09-13 各家当前官方 API 重核；细节见 [PROVIDER_INTEGRATION_GUIDE.md](PROVIDER_INTEGRATION_GUIDE.md)。Monad 已从历史 testnet `10143` 更新为主网 `143`。

## 4. 用户流程

1. `GET /v1/capabilities` 返回 catalog 中每条链和本次启动真正能参与的 provider ID。
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

3. 服务返回 `202`、短期 `accessToken` 和 competition ID；调用方带 Bearer token 轮询结果。
4. 只有未过期、统一 route 校验通过、仿真成功且有有效 `quotedAmount` 的 quote 才能推荐；同一请求的买入 token 和链一致，因此按整数报价输出排序，不引入 token registry 或固定 decimals。
5. build 接收真实 taker 和调用方此前接受的 `acceptedMinBuyAmount`，重新 quote、重新仿真，低于底价或 route 改变即拒绝。
6. 调用方完成 approval（如果需要），重新 build，最后自行签名和广播。

## 5. 最小 API

| 方法 | 路径 | 作用 |
| --- | --- | --- |
| GET | `/health` | 进程健康状态 |
| GET | `/v1/capabilities` | 链与 provider 反向索引发现，不返回 token 列表 |
| POST | `/v1/competitions` | 创建异步报价竞赛 |
| GET | `/v1/competitions/{id}` | 读取带鉴权的结果快照 |
| POST | `/v1/competitions/{id}/quotes/{quoteId}/build` | 重新报价、仿真并生成 unsigned 交易 |

## 6. 配置原则

- 不配置链集合：代码内 catalog 始终存在。
- 不配置 provider 集合：代码内 provider 注册表始终创建全部 13 个 provider。
- 不配置 token：没有 token allowlist；未知 token 由 provider 和链上仿真决定是否可交易。
- RPC 是运行基础设施，不是产品能力开关。按 `RPC_URL_<chainId>` 提供，例如 `RPC_URL_8453`；Ethereum 兼容别名 `ETHEREUM_RPC_URL` 仅为迁移便利保留。
- provider access key 使用各 provider 原生环境变量，不抽象成 credential trait。需要 key 的 provider 缺少任一必需字段时不进入索引；完整名称见 [src/config.rs](../src/config.rs)。

## 7. 明确不在 v1

数据库、多副本状态、分布式限流、跨链、非 EVM、token registry、价格/decimals 目录、provider 熔断、智能路由重写、平台抽成、用户身份系统、SSE、交易广播和回执存储。

## 8. 当前实现状态

v1 的多链 catalog、13 个字符串 provider ID、`supported_chains` 规则、access-key 过滤、反向索引和 13 家真实 HTTP adapter 已实现。每家 adapter 都把 provider 原生 quote 转为统一 `Route`，并拒绝金额、expiry、target、spender、calldata 或 native value 不一致的响应。当前仍不应宣称真实 key、生产 RPC、正式 Router 或主网执行已经验证。

实现和验证状态以 [VERIFICATION.md](VERIFICATION.md) 和 [RUST_REWRITE_LOG.md](RUST_REWRITE_LOG.md) 为准。
