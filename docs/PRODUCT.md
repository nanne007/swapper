# MetaMatch 产品需求 v0.3

日期：2026-09-11。状态：最小核心版本。

## 产品目标

让调用方对同一笔 Ethereum exact-input 兑换获得多家供应商报价、固定区块仿真结果和排序结果，并返回由用户钱包执行的 unsigned transactions。服务不保管私钥、不签名、不广播，也不替用户降低 minimum output。

## 核心范围

- 只支持 Ethereum 主网配置（chainId 1）。资产是服务端白名单中的 ETH/WETH/USDC；只支持买入 ERC-20。
- 并发请求 0x、1inch Classic、KyberSwap，统一成内部 Route 后校验目标、spender、selector、金额和 value。
- 从同一个 parent block 获取 context，使用 `eth_call` 和 `eth_simulateV1` 验证余额增量、审批返回值、gas 和 minimum output。
- 只有有效期内、仿真成功且有净到账值的报价才能成为推荐报价。
- build 时绑定真实 taker，重新报价、重新仿真，并拒绝低于用户已接受底价的结果。
- Router 与路由白名单未配置时可以做 direct preview，但 build 明确返回配置错误。

不在本版本：demo/mock 报价、Base、其他链、跨链、原生币 buy、intent、平台抽成、SSE、OpenAPI endpoint、交易回执代理、数据库和多副本。

## 用户流程

1. `GET /v1/capabilities` 查询唯一支持的链、资产和供应商配置状态。
2. `POST /v1/competitions` 提交 `chainId`、`sellToken`、`buyToken`、十进制整数 `sellAmount`、`slippageBps`，可选 taker。
3. 服务返回 `202`、短期 `accessToken` 和 `competitionId`。调用方带 Bearer token 轮询 `GET /v1/competitions/{id}`，直到 `status=complete`。
4. 调用方只把 `simulation.status=success` 且 `netOutput` 非空的未过期报价视为可比较结果。
5. 调用方提交真实 taker 与先前接受的 `acceptedMinBuyAmount` 到 build。服务重新报价和仿真后返回独立 approval 交易和 swap 交易；调用方自行签名、广播，必要审批完成后重新 build。

## 最小 API

- `GET /health`：进程健康状态。
- `GET /v1/capabilities`：链、资产和供应商配置发现。
- `POST /v1/competitions`：创建报价竞赛。
- `GET /v1/competitions/{id}`：带 token 读取快照和排序报价。
- `POST /v1/competitions/{id}/quotes/{quoteId}/build`：重新报价、仿真并返回 unsigned transactions。

## 验收条件

| 编号 | 验收行为 |
| --- | --- |
| P1 | 三个适配器并发询价，一家失败不丢弃其他结果；无凭据不发起该供应商请求。 |
| P2 | 竞赛异步完成，轮询可读取逐家结果和终态；竞赛数量、请求体、上游响应和 build 并发有界。 |
| P3 | 金额、gas、费用和比较全部使用整数或定点数，不使用浮点金额。 |
| P4 | 仿真严格区分 success、reverted、unsupported、error；仿真和报价不能被 mock 标为真实成功。 |
| P5 | build 校验 taker、有效期、底价、目标白名单，并对最终统一执行 calldata 重新仿真。 |
| P6 | Router 保持精确花费、临时授权、实际到账 minimum、退款增量、pause、reentrancy 等资金不变量。 |

## 非功能与证据边界

服务是单进程、短 TTL、有界内存实现。没有真实供应商 key、生产 simulate RPC、正式 Router/Holder 部署或主网 fork 时，只能声称 fixture、单测和本地 Anvil 已验证；不能将其称为 live end-to-end 或审计完成。

仿真是特定区块和假设下的结果，不保证稍后成交价格；最终 minimum output 由链上 Router 执行。服务端不接受用户提供的 RPC URL、目标地址、calldata、state slot 或 provider endpoint。
