# 来源和已知边界

核对日期：2026-09-09。历史产品观察日期：2026-08-26。

- [Matcha Meta aggregation](https://0x-docs.gitbook.io/matcha-meta/core-concepts/meta-aggregation)：多供应商加仿真。
- [Matcha One-Time Approval](https://0x-docs.gitbook.io/matcha-meta/core-concepts/one-time-approval)：统一授权思想。
- [0x contracts](https://docs.0x.org/docs/core-concepts/contracts)：AllowanceHolder 地址、禁止 approve Settler。
- [0x v2 migration](https://docs.0x.org/docs/upgrading/upgrading-to-swap-v2)：transaction.to/data/value 以及新参数。
- [AllowanceHolderBase](https://github.com/0xProject/0x-settler/blob/master/src/allowanceholder/AllowanceHolderBase.sol)：forward sender 和临时额度真实语义。
- [1inch Classic v6.1](https://business.1inch.com/portal/documentation/apis/swap/classic-swap/methods/v6.1/1/swap/method/get)：from、slippage、disableEstimate、tx。
- [KyberSwap EVM API](https://docs.kyberswap.com/kyberswap-solutions/kyberswap-aggregator/aggregator-api-specification/evm-swaps)：routes 和 route/build 两步。
- [Geth eth_simulateV1](https://geth.ethereum.org/docs/interacting-with-geth/rpc/ns-eth)：隔离状态、多调用和虚拟块。
- [Geth state overrides](https://geth.ethereum.org/docs/interacting-with-geth/rpc/objects)：balance 与 stateDiff。
- [OP Stack fee estimation](https://docs.optimism.io/app-developers/guides/transactions/estimates)：L1 data fee 需另外估算。

实现的 build/MetaRouter 是独立设计，不是声称得到了 Matcha 后端源码。Matcha 的 /api/trade 未经钱包流程验证，不作为实现契约。
