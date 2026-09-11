# Rust 重写计划

日期：2026-09-11。当前目标是最小核心产品。

## 已确认基线

- 根目录 Cargo crate 是唯一 runtime 和质量门入口。
- `contracts/` 仍是独立 Solidity/Foundry 工程，不改写为 Rust。
- 不部署、不连接生产 RPC、不使用真实供应商额度、不签名或广播交易。
- Alloy 2.4.2 要求 Rust 1.94.1；本仓库已同步 manifest、lockfile、CI 和 Docker 工具链。

## 最小核心设计

保留单链 Ethereum、三家报价适配器、固定区块仿真、净到账排序、轮询快照和真实 taker build。移除 demo/mock 数据、Base 未完成分支、SSE 事件系统、OpenAPI runtime endpoint、交易 receipt proxy 和非核心报价诊断字段。

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
- [x] 缺失 Kyber client id 与其他供应商凭据一样返回 unavailable，不访问真实 endpoint。

### Slice 3：验证

- [x] Rust fmt、Clippy、unit tests、release build。
- [x] Alloy 2.x 本地 Anvil E2E。
- [x] Foundry fmt、build、合约测试。
- [x] 更新产品、技术、验证和过程记录，明确 live/fork 未验证边界。

## 不提前增加的能力

不引入数据库、Redis、多副本事件总线、通用 RPC 客户端、DashMap、额外 HTTP middleware、其他链、跨链、intent、平台抽成或公网身份系统。只有出现对应产品需求和可测量瓶颈时再增加。

## 完成定义

Rust 是默认构建、启动和 CI 入口；最小 HTTP 闭环、供应商隔离、固定区块仿真、Router 资金不变量有 fresh 测试证据；真实供应商、生产 RPC、正式部署、主网 fork 和外部审计仍属于独立 release gate。
