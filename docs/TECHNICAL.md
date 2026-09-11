# MetaMatch 技术实现方案

日期：2026-09-11。对应 `PRODUCT.md` v0.3 的最小核心范围。

## 技术栈与边界

Rust 1.94.1、Tokio、Axum、serde、reqwest 0.13、Alloy Provider/RPC types 2.4、alloy-primitives 1.7、alloy-sol-types 1.7；Solidity 0.8.25、Foundry。服务单进程运行，竞赛状态存于带容量和 TTL 的内存中。

直接依赖按职责保持最小：`alloy-provider` 负责标准 EVM provider，`alloy-rpc-types-eth` 负责 `TransactionRequest`、state override 和 `eth_simulateV1` 类型，`alloy-sol-types` 负责 ABI 编码；reqwest 只负责三个固定供应商和价格源 HTTP。没有引入 ORM、数据库、通用 RPC 客户端、DashMap 或额外 middleware。

## 核心模块

- `domain.rs`：输入、金额、价格、净到账和排序；所有金额内部为 `U256`，JSON 使用十进制字符串。
- `config.rs`：单一 Ethereum chain、ETH/WETH/USDC 白名单、服务端 RPC、Router 和 route allowlist。
- `providers.rs`：0x、1inch Classic、KyberSwap 固定 endpoint；serde DTO 只保留执行所需字段。
- `rpc.rs`：Alloy 2.x provider wrapper、固定 parent block 和价格 context。
- `simulation.rs`：先读余额，再按需 approve，再执行 swap，最后读余额；整个序列经 `eth_simulateV1` 固定在同一 block context。
- `execution.rs`：检查 route 元组并编码 AllowanceHolder/MetaRouter transaction。
- `competitions.rs`：并发报价、provider failure isolation、TTL/capacity、token 鉴权和 build 时重新报价。
- `app.rs`：五个最小 HTTP endpoint、请求体上限和错误 envelope。

## API

```text
GET  /health
GET  /v1/capabilities
POST /v1/competitions
GET  /v1/competitions/{id}
POST /v1/competitions/{id}/quotes/{quoteId}/build
```

创建接口返回 `202`、短期访问 token 和 id。结果只通过带 Bearer token 的 polling endpoint 读取；没有 SSE、OpenAPI endpoint 或 receipt proxy。未配置的供应商返回 `unavailable`，不会用 mock/fallback 补齐。

## 报价与 build

匿名 taker 只用于 preview；若配置真实 taker，则供应商从第一轮开始收到该 taker。Router 配置存在时，上游 route 从第一轮以 Router 为 sender，并在 build 前再次检查 target、spender、selector、sell amount 和 value。没有 Router 只能 direct preview，build 返回 `ROUTER_NOT_CONFIGURED`。

build 重新获取 context 和 route，验证 route 输出不低于调用方接受的 minimum，再用真实 taker执行最终仿真。只有 `Simulation::Success` 且 funding 为 `actual` 时才返回 unsigned approvals 和 swap；服务端不签名、不广播。

## 仿真与资金安全

固定 parent block number/hash；仿真块 timestamp 为 parent+1。`eth_call` 只用于 balance/allowance 和 balance-slot 覆盖前置检查，`eth_simulateV1` 用于顺序交易。parent hash 二次检查失败即要求重新报价。余额增量、approve 返回值、gas 余额和 minimum output 都必须通过，RPC 不支持 simulate 时返回 `unsupported`。

统一执行保持 AllowanceHolder → MetaRouter 边界：精确 spend、exact temporary allowance、route tuple allowlist、实际到账 minimum、仅退本次增量、pause 和 reentrancy。正式 Router/Holder、真实 RPC 和供应商服务均不在本地 fixture 验证范围内。

## 验证与后续边界

Rust fixture 覆盖输入/金额、provider DTO、RPC 错误分类、simulation balance delta、轮询生命周期和 build 底价；本地 Anvil E2E 覆盖 HTTP → 仿真 → approval → build → swap。Foundry 继续覆盖合约资金不变量。

Redis/Postgres、多副本、IP/分布式限流、供应商熔断、链特定费用模型、其他链、跨链和公网身份系统均明确留到后续版本，不以空接口占位。
