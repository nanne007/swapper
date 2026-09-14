# Rust 重写与 v1 多链计划

日期：2026-09-13。Rust 重写与 Slice 5 至 Slice 10 已完成；13 家官方接入复核和代码纠偏已纳入当前最小多链 v1。

## 已确认基线

- 根目录 Cargo crate 是唯一 runtime 和质量门入口。
- `contracts/` 仍是独立 Solidity/Foundry 工程，不改写为 Rust。
- 不部署、不使用钱包私钥、不签名或广播交易；live smoke 只访问供应商 HTTP 和公开 read-only RPC。
- Alloy 2.4.2 要求 Rust 1.94.1；本仓库已同步 manifest、lockfile、CI 和 Docker 工具链。

## v1 最小核心设计

聚合 Matcha Meta 页面列出的 13 个 provider，按各 adapter 基于当前官方 API 维护的 `supported_chains()` 直接生成运行时链并集和 `chain -> provider list` 反向索引，不维护独立 chain catalog。配置不包含链/provider 开关和 token 列表；必需 access key 缺失时 provider 返回空集合，其他 provider 默认参与。固定区块仿真、净到账排序、轮询快照和真实 taker build 保留。

安全边界不减：所有金额仍是十进制字符串与 `U256`；供应商/route 失败不伪造成功；Router 只接受服务端 allowlist；build 重新报价和模拟；服务不持有用户密钥。

## 迁移切片

### Slice 1：生态库复用

- [x] 用 Alloy 2.4.2 provider/RPC types 替换手写标准 JSON-RPC。
- [x] 用 alloy-sol-types 保留 ABI 编码，不手写 ABI offset。
- [x] reqwest 升级到 0.13.5，并保留自定义响应体上限和错误清洗。

### Slice 2：核心业务收敛

- [x] 配置只保留 Ethereum；删除 Mode/demo 和 Base fee 未实现分支。
- [x] 只保留 polling HTTP 闭环；删除 SSE replay/heartbeat/stream permit。
- [x] 删除 OpenAPI runtime endpoint、receipt proxy 和 overquote 诊断字段。
- [x] 必需凭据缺失时返回 unavailable；Kyber client ID、Bebop/Odos key 为 optional，不错误阻断公共 endpoint。

### Slice 3：验证

- [x] Rust fmt、Clippy、unit tests、release build。
- [x] Alloy 2.x 本地 Anvil E2E。
- [x] Foundry fmt、build、合约测试。
- [x] 更新产品、技术、验证和过程记录，明确 live/fork 未验证边界。

### Slice 4：v1 多链骨架

- [x] 增加 Matcha provider ID、provider 自有 chain slug 和支持矩阵；运行时链由矩阵并集派生。
- [x] 删除 chain/provider/token 业务配置；保留按 chain ID 注入 RPC 的基础设施配置。
- [x] 增加 key-aware `supported_chains()` 和启动期 `chain -> provider` 反向索引。
- [x] 竞赛、capabilities、provider quote 调度改为只读取反向索引。
- [x] 13 个 provider 均完成真实 adapter 注册；不使用 pending/mock quote。

### Slice 5：HTTP quote adapters

- [x] 实现 Barter route → swap 两阶段请求、Bearer/X-Request-Id、status/amount/value 校验。
- [x] 实现 Bebop RFQ、OpenOcean v4 swap、Velora v6.2 swap 的 response DTO 和统一 Route。
- [x] 为上述四家补脱敏 fixture，覆盖 endpoint、链参数、expiry、target/spender 和 native value。

### Slice 6：key-gated adapters

- [x] 实现 Enso `api.enso.build` route 请求、Bearer 认证、preTransactions spender 和跨链 route 拒绝。
- [x] 实现 HyperBloom HyperEVM quote、api-key 认证、chain/token/value 校验。
- [x] 缺少必需 key 时继续由 adapter 的 `supported_chains()` 返回空集合。

### Slice 7：metadata-dependent adapters

- [x] 实现 LiquidSwap route；仅支持文档明确的 ERC-20 contract address 输入，decimals 通过服务端配置 RPC 的 `eth_call` 获取。
- [x] 实现 Odos quote v2 → assemble 两阶段请求，不引入 token registry 或浮点金额。
- [x] 明确 RPC 未配置时 LiquidSwap 返回 `RPC_NOT_CONFIGURED`，不猜 decimals。
- [x] live 验证确认零地址不能表示 LiquidSwap 原生币输入；原生币卖出明确返回 `NATIVE_SELL_UNSUPPORTED`，不把上游 500 当作报价结果。

### Slice 8：chain-specialized and signed API

- [x] 实现 OogaBooga HyperEVM/Berachain endpoint、Bearer API key、native zero address 和动态 router。
- [x] 实现 OKX DEX Swap v6 HMAC-SHA256/Base64 认证；key/secret/passphrase 必需，project ID 可选。
- [x] 对签名请求只保留 header/参数验证，不在日志或 fixture 写入真实 secret。

### Slice 9：core simplification and behavior tests

- [x] 删除固定 18 decimals 的 CoinGecko/net-output 路径；同一 swap 请求按已仿真的整数 `quotedAmount` 排序。
- [x] 保留 token 透传、route allowlist、金额/value/calldata/expiry、simulation、TTL/capacity 和 provider failure isolation。
- [x] 13 家 adapter 均有 fixture 测试；registry/capabilities 测试验证 key-aware 反向索引。

### Slice 10：documentation and quality gates

- [x] 更新 README、产品、技术、架构、来源、验证和迁移日志，区分 fixture、local Anvil 和 live release gate。
- [x] `cargo fmt`、Clippy、Rust tests、release build、Foundry tests 和 ignored Anvil E2E 全部重新执行。
- [x] 不执行 commit、push、部署、生产 RPC、钱包签名或真实交易广播。

## 不提前增加的能力

不引入数据库、Redis、多副本事件总线、通用 RPC 客户端、DashMap、额外 HTTP middleware、跨链、intent、平台抽成、token registry 或公网身份系统。只有出现对应产品需求和可测量瓶颈时再增加。

## 完成定义

Rust 是默认构建、启动和 CI 入口；最小 HTTP 闭环、供应商隔离、固定区块仿真、Router 资金不变量有 fresh 测试证据；真实供应商、生产 RPC、正式部署、主网 fork 和外部审计仍属于独立 release gate。
