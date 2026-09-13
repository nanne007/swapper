# Provider 官方接入与实现核对 v1

核对日期：2026-09-13。

本文回答三件事：每家 provider 能做什么、接入前需要什么、当前 Rust adapter 如何把官方协议收敛成 MetaMatch 的统一 `Route`。它既是接入手册，也是 review `src/providers/*.rs` 的契约清单。

## 1. 范围和证据规则

- v1 的 13 家 provider 名单来自 [Matcha Meta DEX aggregation](https://0x-docs.gitbook.io/matcha-meta/core-concepts/dex-aggregation)。Matcha 页面用于决定“收录谁”，不继续充当每家 API 参数和当前链支持的唯一依据。
- 每家能力、认证、endpoint、chain、金额单位、native token 和响应字段优先采用该 provider 当前官方文档、官方 SDK 或官方 GitHub。代码里的链矩阵是 2026-09-13 官方支持链与本项目 17 条 EVM catalog 的交集。
- provider 官网可能随时变更。`supported_chains()` 是代码快照，不是远端动态发现；上线前和每次升级 adapter 时必须重新核对本文链接并运行 live smoke。
- “API 返回交易”不等于“可安全成交”。所有响应仍要经过金额、token、taker、spender、target、calldata、native value、expiry 和链上仿真检查。
- v1 只做同链、EVM、exact-input、自托管交易。跨链、intent/gasless、Solana、平台收费和 provider token registry 不进入核心实现。

## 2. 总览

链 ID 使用本项目 catalog；逗号列表表示当前 adapter 会返回的静态能力交集。

| Provider | v1 API/模式 | 认证与申请 | 当前 catalog 链 | Native 表示 | 主要注意事项 |
| --- | --- | --- | --- | --- | --- |
| 0x | Swap API v2 AllowanceHolder firm quote | 必需 `ZERO_EX_API_KEY`，0x Dashboard | 1, 10, 56, 130, 137, 143, 146, 999, 5000, 8453, 9745, 42161, 43114, 59144, 80094, 534352 | `0xeeee…` | approval spender 与 `transaction.to` 是两个字段；绝不能 approve Settler |
| 1inch | Classic Swap v6.1 | 必需 `ONE_INCH_API_KEY`，1inch Business Portal | 1, 10, 56, 130, 137, 143, 146, 999, 8453, 42161, 43114, 59144 | `0xeeee…` | Bearer；slippage 是百分数；当前使用官方 Router v6 地址 |
| KyberSwap | Aggregator legacy public route/build | 无 key 可用；`KYBER_CLIENT_ID` 可选 | 1, 10, 56, 130, 137, 143, 146, 999, 5000, 8453, 9745, 42161, 43114, 59144, 80094 | `0xeeee…` | route/build 必须连续完成；route 最多缓存约 5–10 秒；公开网关限流较严 |
| Barter | Router API route → swap | 必需 `BARTER_API_KEY`，向 Barter 获取 | 1, 8453, 42161 | 未形成可靠公开契约 | 每次调用唯一 `X-Request-Id`；deadline 是毫秒；`minReturn` 至少为 quote 的 98% |
| Bebop | PMM/RFQ v3 self-execution | demo 可免 key；`BEBOP_API_KEY` 可选、生产建议申请 | 1, 10, 56, 137, 999, 8453, 42161 | `0xeeee…` | 地址需 EIP-55 checksum；使用 firm `minimumAmount` 和秒级 expiry |
| Enso | Shortcuts Route，`router` strategy | 必需 `ENSO_API_KEY`，Enso Dashboard | 1, 10, 56, 130, 137, 143, 146, 999, 8453, 9745, 42161, 43114, 59144, 80094 | `0xeeee…` | POST body 中 token/amount 是数组；v1 主动拒绝跨链 route |
| HyperBloom | HyperEVM Swap `/quote` | 必需 `HYPERBLOOM_API_KEY`，联系团队 | 999 | `0xeeee…` | `api-key` header；slippage 是小数比例；仅 HyperEVM |
| LiquidSwap | Route Finding v2 exact-input | 无认证 | 999 | API 要求 token 合约地址，v1 用 WHYPE 而非 native | `amountIn` 是 human-readable；响应 details 才是 base-unit；需 RPC 读 decimals |
| Odos | SOR quote v2 → assemble | 官方公开 API 无认证；`ODOS_API_KEY` 仅作为可选透传 | 1, 10, 56, 130, 137, 146, 5000, 8453, 42161, 43114, 59144, 534352 | 零地址 | quote 的 `pathId` 再 assemble；approval target 取组装交易 Router |
| OogaBooga | Swap API | 必需 `OOGABOOGA_API_KEY`，联系团队 | 999, 80094 | 零地址 | 只支持 `/tokens` 白名单；router/executor 动态返回，不硬编码 |
| OKX | Classic Swap API v6 | 必需 API key/secret/passphrase；project ID 可选 | 1, 10, 56, 130, 137, 143, 146, 999, 5000, 8453, 9745, 42161, 43114, 59144, 81457, 534352 | `0xeeee…` | HMAC 签名覆盖 path+query；ERC-20 请求 approval data；使用上游 `minReceiveAmount` |
| OpenOcean | Swap API v4 | Public 无 key，默认 2 RPS | 全部 17 条 catalog 链 | 按链分别为 `eeee`、零地址或 Polygon `…1010` | swap 前读取 gasPrice；403 是 IP/安全策略，不应伪装成缺 key |
| Velora | Market API `/swap` v6.2 | Public 无 key | 1, 10, 56, 130, 137, 146, 8453, 9745, 42161, 43114 | `0xeeee…` | `/swap` 低限流、无 RFQ、无 allowance/balance check；必须自行仿真 |

## 3. 统一接入边界

每家 adapter 最终只输出：provider 字符串 ID、sell/buy/min amount、spender、`to/data/value` 和短 expiry。共享层不猜 provider schema，也不引入通用 credential abstraction。

认证规则如下：

1. 必需凭证缺失时，provider 的 `supported_chains()` 返回空列表，启动期反向索引不会调度它。
2. 免 key provider 始终返回自身链矩阵。若已有明确的 optional credential 字段，配置后只传给该 adapter，不改变参与资格。
3. key 只存在服务端环境变量，不返回 API、不写日志、不进 fixture。
4. 供应商的 Pro/Enterprise host 或新认证方式只有在拿到正式文档后才能实现；不能靠猜 header 或 query 参数接入。

## 4. 逐家接入说明

### 4.1 0x

官方入口：[Swap API v2 migration](https://docs.0x.org/docs/upgrading/upgrading-to-swap-v2)、[supported chains](https://docs.0x.org/docs/introduction/supported-chains)、[Dashboard](https://dashboard.0x.org/)。

- 能力：聚合 AMM/RFQ 流动性，firm `/swap/allowance-holder/quote` 返回可执行交易；v1 不采用需要用户 EIP-712 签名拼接的 Permit2 模式。
- 前提：创建 0x application 并配置 `ZERO_EX_API_KEY`；请求带 `0x-api-key` 和强制的 `0x-version: v2`。
- 请求：统一 host `https://api.0x.org`，链由 `chainId` 指定；金额是 base-unit 十进制整数，滑点是 bps。
- 响应：ERC-20 spender 只能取 `issues.allowance.spender` 或 `allowanceTarget`；执行入口只取 `transaction.to`。两者允许不同且都不能硬编码。native sell 不需要 ERC-20 approval。
- 注意：官方明确禁止向 Settler 授权。任何把 spender 和执行 target 强行比较为同一固定地址的实现都是错误的。

代码：[zero_ex.rs](../src/providers/zero_ex.rs)。测试覆盖 native 动态 target 和 ERC-20 spender/target 分离。

### 4.2 1inch

官方入口：[Classic Swap introduction](https://business.1inch.com/portal/documentation/apis/swap/classic-swap/introduction)、[quick start](https://business.1inch.com/portal/documentation/apis/swap/classic-swap/quick-start)、[Business Portal getting started](https://business.1inch.com/portal/documentation/overview/getting-started)。

- 能力：Pathfinder v6.1 的同链 exact-input 交易；1inch 另有 Intent 和 Cross-chain 产品，但不属于 v1。
- 前提：在 1inch Business Portal 创建 application/key，配置 `ONE_INCH_API_KEY`，使用 `Authorization: Bearer`。
- 请求：`GET /swap/v6.1/{chain}/swap`；`amount` 是 base units；`slippage=0.30` 表示 0.30%；v1 禁止 partial fill，并让本项目负责链上仿真。
- 响应：`dstAmount` 是预期输出，`tx` 是交易；当前官方 quickstart 使用 Aggregation Router v6 `0x111111125421ca6dc452d289314280a0f8842a65` 作为 approval/执行目标。
- 注意：如果上游返回非空 `stateOverrides`，v1 拒绝，避免在没有重放同一状态假设时把条件性 quote 当成可执行。

代码：[one_inch.rs](../src/providers/one_inch.rs)。

### 4.3 KyberSwap

官方入口：[EVM Aggregator API](https://docs.kyberswap.com/developer-guide/aggregator-api/aggregator-api-specification/evm-swaps)。

- 能力：先 `/routes` 找最优 route，再 `/route/build` 生成 calldata。
- 前提：当前代码保留可匿名访问的 `aggregator-api.kyberswap.com/{chain}/api/v1`。`KYBER_CLIENT_ID` 是可选 client identity；缺失时仍参与，但公开网关配额更低。Kyber 新商业 gateway/API key 不在未确认完整 endpoint 契约前混入此 adapter。
- 请求：native 为 `0xeeee…`；金额为 base units；build 的 `slippageTolerance` 是 bps，deadline 是 Unix 秒。
- 响应：route 和 build 的 `routerAddress` 必须一致；交易 value 对 native 和 ERC-20 分别校验。
- 注意：官方要求 route summary 短期使用，通常只应缓存 5–10 秒。adapter expiry 因而设为 10 秒，禁止跨请求复用 `routeSummary`。当前网关还会拒绝空 User-Agent；共享 Reqwest client 已设置 `metamatch-backend/<version>`，真实测试证明该 403 与 access key 无关。

代码：[kyber.rs](../src/providers/kyber.rs)。

### 4.4 Barter

官方入口：[Barter Router Swap API](https://barterdefi.gitbook.io/barter-docs/barter-router-system/swap-api)。

- 能力：Ethereum/Base/Arbitrum 上先获取 route，再把 route 参数交给 `/swap` 生成交易。
- 前提：配置 `BARTER_API_KEY`，使用 Bearer；每个 HTTP 调用都生成新的 UUID `X-Request-Id`。
- 请求：`sellAmount` 是 base units；`deadline` 是未来 Unix 毫秒，不是秒；`recipient`/`origin` 绑定用户地址。
- 响应：两阶段 input/output 必须一致；当前把 swap `to` 作为 execution target 和 spender。
- 注意：官方要求 `minReturn` 不低于预期输出的 98%。产品允许最高 5% 滑点时，adapter 会收紧为最多 2%，不会向下放宽用户保护。公开文档没有形成稳定 native sell marker 契约，因此 v1 明确拒绝 native sell。

代码：[barter.rs](../src/providers/barter.rs)。

### 4.5 Bebop

官方入口：[RFQ API quickstart](https://docs.bebop.xyz/rfq-api/quickstart)、[authentication](https://docs.bebop.xyz/core-concepts/authentication)。

- 能力：PMM/RFQ firm quote，self-execution 返回 approval target、交易和保证最小输出。
- 前提：demo 可不带 key，但配额和市场覆盖受限；生产应申请 key 并配置 `BEBOP_API_KEY`，adapter 会用 Bearer 透传。
- 请求：`/pmm/{chain}/v3/quote`，必须 `gasless=false`；sell/buy/taker/receiver 地址使用 EIP-55 checksum。
- 响应：精确匹配 `chainId`、taker、sell/buy token 和 amount；`minimumAmount` 必须存在；`expiry` 是 Unix 秒，转换为毫秒。
- 注意：不再本地推导缺失的 minimum，因为那不是 provider 的 firm guarantee。

代码：[bebop.rs](../src/providers/bebop.rs)。

### 4.6 Enso

官方入口：[Route guide](https://docs.enso.build/pages/build/get-started/route)、[API reference](https://docs.enso.build/api-reference/defi-shortcuts/optimal-route-between-two-tokens)、[routing strategies](https://docs.enso.build/pages/build/reference/routing-strategies)、[authentication](https://docs.enso.build/pages/build/get-started/authentication)。

- 能力：token swap、zap、position migration 和跨链 route。v1 只接受返回路径全部位于请求链的普通 token route。
- 前提：Enso Dashboard 申请 `ENSO_API_KEY`，Bearer 认证；官方默认限流需按账户核对。
- 请求：`POST /api/v1/shortcuts/route` JSON；`tokenIn`、`tokenOut`、`amountIn` 都是数组，金额为 base units，slippage 为 bps。v1 使用适合 EOA/普通 Router 的 `routingStrategy=router`。
- 响应：`tx.to` 必须按响应使用，不能硬编码；ERC-20 spender 优先从对应 token 的 `preTransactions` 读取，否则使用 tx target。
- 注意：官方 API 支持 `destinationChainId`，但产品没有跨链状态机，任何跨链 leg 都返回 `CROSS_CHAIN_ROUTE_UNSUPPORTED`。若未来把 Enso calldata 嵌入 MetaRouter，还需主网 fork 证明 sender/receiver/spender 语义兼容。

代码：[enso.rs](../src/providers/enso.rs)。

### 4.7 HyperBloom

官方入口：[API overview](https://docs.hyperbloom.xyz/api-reference/overview)、[endpoints](https://docs.hyperbloom.xyz/api-reference/endpoints)。

- 能力：HyperEVM 聚合 quote；`/quote` 返回可执行数据，`/price` 仅 indicative，v1 使用前者。
- 前提：联系 HyperBloom 获取 key，配置 `HYPERBLOOM_API_KEY`，header 名为 `api-key`。
- 请求：金额 base units；native 为 `0xeeee…`；`slippagePercentage=0.003` 表示 0.3%。
- 响应：校验 chainId、token、sellAmount、allowanceTarget、to/data/value。
- 注意：接口可能返回 protocol fee。当前共享资金不变量要求 ERC-20 swap 的 native `value=0`；需要额外 native fee 的 route 会被保守拒绝，直到费用模型被显式实现。

代码：[hyperbloom.rs](../src/providers/hyperbloom.rs)。

### 4.8 LiquidSwap

官方入口：[Route Finding](https://docs.liqd.ag/liquidswap-integration/route-finding)。

- 能力：HyperEVM/Robinhood 上聚合多家 DEX；当前 catalog 只交集到 HyperEVM 999。
- 前提：无认证；但本项目需要该链 RPC 读取 sell token `decimals()`，例如 `RPC_URL_999`。
- 请求：tokenIn/tokenOut 是合约地址；`amountIn` 是 human-readable 十进制数，不是 base units；slippage 是百分数。
- 响应：必须 `success=true`，token 地址必须匹配；`execution.details.amountIn/amountOut/minAmountOut` 转回统一 base-unit 金额，`execution.to/calldata` 生成交易。
- 注意：官方参数要求合约地址，所以 native HYPE 不能用零地址或 `eeee` 冒充 token；调用方应传 WHYPE。adapter 不配置 feeBps/feeRecipient，避免在 v1 引入收费逻辑。

代码：[liquid_swap.rs](../src/providers/liquid_swap.rs)。

### 4.9 Odos

官方入口：[Odos API](https://docs.odos.xyz/build/quickstart/sor)、[官方 odos-mcp](https://github.com/odos-xyz/odos-mcp)。

- 能力：`/sor/quote/v2` 路径发现，随后 `/sor/assemble` 生成交易。
- 前提：官方公共 `api.odos.xyz` 路径不要求 access key，因此无 key 也参与。为兼容已有部署，若配置 `ODOS_API_KEY`，adapter 只以 `x-api-key` 可选透传；它不是参与门槛。
- 请求：金额是 base units，slippageLimitPercent 是百分数；native 映射为零地址；用户地址进入 quote 和 assemble。
- 响应：只接受单输入/单输出的第一个 amount；assemble 的 `transaction.to` 同时作为当前 Router spender/target。
- 注意：`pathId` 是临时状态，不能跨 quote 重用。当前官方站点偶发边缘网关 530 时属于上游/出口问题，不应改成伪 credential 逻辑。

代码：[odos.rs](../src/providers/odos.rs)。

### 4.10 OogaBooga

官方入口：[Swap API guide](https://docs.oogabooga.io/developers/swap-api/guide)、[reference](https://docs.oogabooga.io/developers/swap-api/reference)、[router notes](https://docs.oogabooga.io/developers/swap-api)。

- 能力：Berachain/HyperEVM host 上的 smart-order-routing quote 和 calldata。
- 前提：联系团队申请 key，配置 `OOGABOOGA_API_KEY`，Bearer 认证。
- 请求：金额 base units；native 为零地址；slippage 是小数比例；只有提供 `to` 才返回 execution data。
- 响应：使用 `routerAddr` 作为 approval/交易 Router，验证 amount/min/value/calldata。
- 注意：API 当前只交易 `/tokens` 白名单中的 token。官方明确 executor 是临时地址，不能硬编码，也不能直接调用；adapter 只消费 API 返回的 Router 数据。虽然官方部署页可能出现更多链，未确认 chain-specific API host 前不会扩大 `supported_chains()`。

代码：[ooga_booga.rs](../src/providers/ooga_booga.rs)。

### 4.11 OKX

官方入口：[Classic Swap v6](https://web3.okx.com/onchainos/dev-docs/trade/dex-swap)、[API authentication](https://web3.okx.com/onchainos/dev-docs/build/dev-portal/api-access-and-usage)。

- 能力：同链 Classic Swap 返回 OKX DEX Router 交易。v1 已从旧 v5 升到 v6。
- 前提：在 OKX Developer Portal 创建项目和 API credentials；必需 `OKX_API_KEY`、`OKX_SECRET_KEY`、`OKX_API_PASSPHRASE`。`OKX_PROJECT_ID` 若存在则附加，但官方 swap request 示例不把它作为必需签名头。
- 请求：GET `/api/v6/dex/aggregator/swap`；HMAC-SHA256 prehash 是 `timestamp + GET + path + ?query`；slippagePercent 是百分数。ERC-20 请求添加 `approveTransaction=true` 和 `approveAmount`。
- 响应：`tx.minReceiveAmount` 是上游保护下限；开启 approval 后，`tx.signatureData` 中 JSON 字符串的 `approveContract` 是 spender，不能把 `tx.to` 当成 approval target。
- 注意：签名必须使用实际编码后的 query 且时间同步；不要记录 secret、signature 或完整上游错误 body。OKX Router 可退回未消耗 token，MetaRouter 的余额增量/退款语义必须继续保留。

代码：[okx.rs](../src/providers/okx.rs)。

### 4.12 OpenOcean

官方入口：[Swap API v4](https://docs.openocean.finance/docs/swap-api/v4)、[supported chains](https://docs.openocean.finance/docs/overview/supported-chains)、[pricing/access](https://docs.openocean.finance/docs/swap-api/api-pricing-and-access)、[errors](https://docs.openocean.finance/docs/developer-resources/errors)。

- 能力：公开 v4 quote/swap；v1 使用 `/swap` 直接获取交易。
- 前提：Public API 无 key，默认 2 RPS；更高配额或商业 SLA 需单独申请 Pro/Enterprise。
- 请求：先 `GET /v4/{chain}/gasPrice`，把 `data.standard` 作为 `gasPriceDecimals`；再调用 `/swap`。amountDecimals 是 base units，slippage 是百分数。
- Native：Ethereum/Optimism/BNB/Unichain/Base/Arbitrum/Linea/Blast/Scroll 用 `0xeeee…`；Polygon 用 `0x000…1010`；当前其余 catalog 链用零地址。该映射必须按官方链表更新，不能全局统一。
- 响应：使用上游 in/out/min amount、chainId、from、to/data/value；当前 swap target 也作为 spender。
- 注意：本机实测 gasPrice 为 200、swap 为 403。官方错误表把 401/402 归为 Pro key 问题，403 归为 IP 白名单/安全策略，因此保持免 key 语义并让 live test 失败；需要 OpenOcean 加白或提供正式商业 host，而不是猜 API key。

代码：[open_ocean.rs](../src/providers/open_ocean.rs)。

### 4.13 Velora

官方入口：[Market API rate/swap](https://developers.velora.xyz/api/velora-api/velora-market-api/get-rate-for-a-token-pair-1)、[API v6.2](https://developers.velora.xyz/api/velora-api/velora-market-api/master/api-v6.2)。

- 能力：原 ParaSwap/现 Velora Market API v6.2 聚合路由并返回 `priceRoute + txParams`。
- 前提：公开 endpoint，无 access key；生产流量和商业支持仍应向 Velora 核对。
- 请求：`GET https://api.paraswap.io/swap`，SELL side，base-unit amount，native 为 `0xeeee…`，slippage 是 bps。v1 传 `ignoreBadUsdPrice=true`，表示不让上游 USD 价格保护替代本项目仿真。
- 响应：校验 network/srcAmount/destAmount；spender 优先 `tokenTransferProxy`，兼容 `contractAddress`；交易取 `txParams`。
- 注意：官方说明 `/swap` 相比分步 `/prices` + `/transactions` 有更低 rate limit，不做 allowance/balance 检查且不包含 RFQ。当前最小实现接受这些取舍，但绝不能跳过本地余额、授权和成交仿真。

代码：[velora.rs](../src/providers/velora.rs)。

## 5. Review 检查表

每次升级 provider 时逐项回答：

1. endpoint/version/method 是否仍为官方当前版本？
2. key 是必需、optional 还是完全不支持？缺 key 时 `supported_chains()` 是否正确？
3. 支持链是当前官方能力与 catalog 的交集吗？测试网是否误混入主网？
4. amount、slippage、deadline/expiry 的单位是什么？有无浮点精度风险？
5. native token 是否按该 provider、该链映射，而不是全局猜一个 sentinel？
6. approval spender、execution target 和临时 executor 是否被正确区分？
7. 是否使用上游 firm minimum/expiry；若本地推导，文档是否明确它只是保守显示值？
8. 两阶段 API 是否验证 path/router/amount 一致并限制缓存时间？
9. response 是否校验 chain/token/taker/from；是否拒绝额外 native value、空 calldata、零 target？
10. fixture 是否只测协议；live smoke 是否真实访问服务、严格要求 2xx+可归一化 quote、且不签名广播？

## 6. 当前仍需上线前确认

- 逐 provider 商业条款、限流、归因/展示、费用和 production SLA。
- 0x/1inch/Enso 等动态交易目标嵌入 MetaRouter 后的主网 fork 行为；不得因“API 返回”就自动进入链上 allowlist。
- Barter native、Odos 商业 key、Kyber 新 gateway、OogaBooga 新链 host 等尚无完整公开契约的能力。
- OpenOcean 出口 IP 加白；所有 key-gated provider 的真实账户权限和成功 quote。
- 每条链的可信 `eth_simulateV1` RPC、正式 Router 部署、外部审计和小额人工成交验收。
