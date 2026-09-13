# 来源和已知边界

核对日期：2026-09-13。历史产品观察日期：2026-08-26。

- [Matcha Meta aggregation](https://0x-docs.gitbook.io/matcha-meta/core-concepts/dex-aggregation)：v1 的 13 家 provider 名单与聚合概念；不再把该页的历史链矩阵当成各家当前 API 契约。
- [Matcha One-Time Approval](https://0x-docs.gitbook.io/matcha-meta/core-concepts/one-time-approval)：统一授权思想。
- [0x contracts](https://docs.0x.org/docs/core-concepts/contracts)：AllowanceHolder、Settler 和 approval 安全边界。
- [0x v2 migration](https://docs.0x.org/docs/upgrading/upgrading-to-swap-v2)：spender 与 `transaction.to` 分离、动态 target、v2 header/参数。
- [0x supported chains](https://docs.0x.org/docs/introduction/supported-chains)：当前 Swap API chain matrix。
- [AllowanceHolderBase](https://github.com/0xProject/0x-settler/blob/master/src/allowanceholder/AllowanceHolderBase.sol)：forward sender 和临时额度真实语义。
- [1inch Classic v6.1](https://business.1inch.com/portal/documentation/apis/swap/classic-swap/introduction)：Pathfinder v6.1、认证与当前支持链。
- [1inch Classic quickstart](https://business.1inch.com/portal/documentation/apis/swap/classic-swap/quick-start)：Bearer、Router、allowance 和 swap 流程。
- [KyberSwap EVM API](https://docs.kyberswap.com/developer-guide/aggregator-api/aggregator-api-specification/evm-swaps)：公开 legacy gateway、routes 和 route/build 两步、短 route 有效期。
- [Geth eth_simulateV1](https://geth.ethereum.org/docs/interacting-with-geth/rpc/ns-eth)：隔离状态、多调用和虚拟块。
- [Geth state overrides](https://geth.ethereum.org/docs/interacting-with-geth/rpc/objects)：balance 与 stateDiff。
- [OP Stack fee estimation](https://docs.optimism.io/app-developers/guides/transactions/estimates)：L1 data fee 需另外估算。
- [Monad developer portal](https://developers.monad.xyz/)：当前 Monad mainnet chain ID `143`；历史 testnet `10143` 不再进入 v1 catalog。
- [Enso route guide](https://docs.enso.build/pages/build/get-started/route)：Route API 的 token/amount 数组、slippage、响应和跨链能力。
- [Enso route API reference](https://docs.enso.build/api-reference/defi-shortcuts/optimal-route-between-two-tokens)：`POST /shortcuts/route` 参数与响应结构。
- [Enso routing strategies](https://docs.enso.build/pages/build/reference/routing-strategies)：EOA `router`、smart-wallet `delegate` 和动态 `tx.to`。
- [Enso authentication](https://docs.enso.build/pages/build/get-started/authentication)：Enso `api.enso.build` 和 Bearer API key。
- [Bebop RFQ API quickstart](https://docs.bebop.xyz/rfq-api/quickstart)：Bebop chain/API 形态、expiry、approvalTarget 和 self-execution 边界。
- [LiquidSwap route finding](https://docs.liqd.ag/liquidswap-integration/route-finding)：LiquidSwap endpoint 无需认证，tokenIn/tokenOut 必须是合约地址；WHYPE/USDT0 官方示例、human-readable amount、execution calldata 和 base-unit details。
- [Odos SOR quickstart](https://docs.odos.xyz/build/quickstart/sor)：Odos quote/assemble 流程。
- [Odos official MCP](https://github.com/odos-xyz/odos-mcp)：公开 `api.odos.xyz`、无认证请求和 endpoint/DTO 的官方代码交叉证据。
- [OpenOcean API v4](https://docs.openocean.finance/docs/swap-api/v4)：OpenOcean v4 swap endpoint、amountDecimals、slippage、minOutAmount 和 transaction 形态。
- [OpenOcean API access](https://docs.openocean.finance/docs/swap-api/api-pricing-and-access)：公开 Swap API 对所有 DeFi builders 开放，默认 2 RPS；Pro/Enterprise 需单独接入。
- [OpenOcean error codes](https://docs.openocean.finance/docs/developer-resources/errors)：401/402 对应 Pro API key，403 对应 IP 白名单或安全策略。
- [Velora Market API](https://developers.velora.xyz/api/velora-api/velora-market-api/get-rate-for-a-token-pair-1)：Velora `/swap` 的 priceRoute/txParams 形态。
- [HyperBloom API endpoints](https://docs.hyperbloom.xyz/api-reference/endpoints)：HyperBloom quote 参数、`api-key` 和 slippage 单位。
- [OogaBooga swap API](https://docs.oogabooga.io/developers/swap-api.md)：OogaBooga chain host、Bearer key、native zero address 和动态 router。
- [OKX DEX Swap API v6](https://web3.okx.com/onchainos/dev-docs/trade/dex-swap)：v6 endpoint、`slippagePercent`、approval `signatureData`、`minReceiveAmount` 和交易边界。
- [Barter swap API](https://barterdefi.gitbook.io/barter-docs/barter-router-system/swap-api)：Barter route/swap 两阶段请求、Bearer、X-Request-Id 和 deadline。

逐家分析、配置字段、native marker、chain matrix 和代码映射汇总在 [PROVIDER_INTEGRATION_GUIDE.md](PROVIDER_INTEGRATION_GUIDE.md)。实现的 build/MetaRouter 是独立设计，不是声称得到了 Matcha 后端源码。Matcha 的 `/api/trade` 未经钱包流程验证，不作为实现契约。
