# JSON 配置与启动校验

服务只读取一个 JSON 配置文件，默认路径是当前工作目录的 `config.json`。不加载 `.env`，不合并应用环境变量，也不查找父目录。文件缺失、不可读、JSON 或校验失败均以非零状态退出，不启动 HTTP listener。变更配置后重启生效，没有热加载。

```sh
cp config.example.json config.json
cargo run
# 或显式指定文件路径（单个位置参数）
cargo run -- /absolute/path/config.json
```

示例不含凭据和部署地址，复制后可以启动 health/capabilities，但不能执行交换。`config.json` 已被 Git 和 Docker context 忽略；其他自定义文件路径也必须自行保护，不要提交或写入镜像。服务读取明文凭据，按部署环境限制文件权限；不要把文件内容输出到日志。

## 字段

| JSON 字段 | 类型 / 默认 | 约束 |
| --- | --- | --- |
| `host` | IP 字符串 / `127.0.0.1` | 支持 IPv4/IPv6，不接受主机名 |
| `port` | 整数 / `3000` | `1..65535` |
| `competitionTimeoutMs` | 整数 / `6000` | `100..30000`；竞赛共用一个总预算；每条链 bootstrap 也使用此超时上限 |
| `maxActive` | 整数 / `20` | 正数且不超过 Tokio semaphore 容量 |
| `chains` | 对象 / `{}` | 非零 chain ID 字符串 → 链基础设施配置；不是链白名单 |
| `chains.<id>.rpcUrl` | 可选字符串 / `null` | 可信 HTTP(S) RPC，必须有 host |
| `chains.<id>.router` | 可选 20-byte 地址 / `null` | 已部署 MetaRouter；拒绝零地址/native sentinel |
| `alchemyApiKey` | 可选字符串 / `null` | 该链没有显式 RPC 时，用内置官方 endpoint 映射生成 RPC |
| `balanceSlots` | 对象 / `{}` | chain ID → token 地址 → uint256 十六进制 mapping base，如 `"0x0"` |
| `providerKeys` | 对象 / `{}` | 只接受下表 provider ID；不配置 provider enable/disable |
| `okxSecretKey` / `okxPassphrase` | 可选字符串 / `null` | 与 `providerKeys.okx` 一起决定 OKX 是否可参与 |
| `okxProjectId` | 可选字符串 / `null` | OKX 可选项目标识 |

可选字符串可省略或设为 `null`；空白字符串不作为“未配置”接受。数字必须是 JSON 整数，不能用字符串、负数或小数。Serde 使用 `rename_all`、`deny_unknown_fields`、默认值和 typed 地址/整数解析；`try_from` 校验范围和跨字段不变量。根对象、每条链和 providerKeys 的未知字段、重复结构字段均拒绝。错误保留字段路径/解析原因，但不要将内部诊断暴露给 API 调用方。

## 从环境变量迁移

| 旧环境变量 | 新 JSON 路径 |
| --- | --- |
| `HOST` / `PORT` | `host` / `port` |
| `COMPETITION_TIMEOUT_MS` | `competitionTimeoutMs`（`maxActive` 原来固定为 20，现在可配置） |
| `RPC_URL_<chainId>` | `chains.<chainId>.rpcUrl` |
| `ETHEREUM_RPC_URL` | `chains.1.rpcUrl`（不再保留旧别名） |
| `ALCHEMY_API_KEY` | `alchemyApiKey` |
| `BALANCE_SLOTS` | `balanceSlots`（对象，不是包着 JSON 的字符串） |
| `ZERO_EX_API_KEY` / `ONE_INCH_API_KEY` | `providerKeys.0x` / `providerKeys.1inch` |
| `BARTER_API_KEY` / `BEBOP_API_KEY` / `ENSO_API_KEY` | `providerKeys.barter` / `providerKeys.bebop` / `providerKeys.enso` |
| `HYPERBLOOM_API_KEY` / `OOGABOOGA_API_KEY` | `providerKeys.hyperBloom` / `providerKeys.oogaBooga` |
| `KYBER_CLIENT_ID` / `ODOS_API_KEY` | `providerKeys.kyber` / `providerKeys.odos` |
| `OKX_API_KEY` | `providerKeys.okx` |
| `OKX_SECRET_KEY` / `OKX_API_PASSPHRASE` / `OKX_PROJECT_ID` | `okxSecretKey` / `okxPassphrase` / `okxProjectId` |

必需 key 和 optional key 的 provider 参与规则不变。`RUST_LOG`、`RUST_LIB_BACKTRACE` 是诊断工具开关，仍可使用；ignored live 测试的 `METAMATCH_RUN_LIVE_*` 等开关仅控制测试，不是应用配置来源。live 测试也从 `config.json` 读取应用凭据。

## 配置 Router 与发现 Holder

例如 Base（下列地址仅为占位，必须替换为实际部署地址）：

```json
{
  "chains": {
    "8453": {
      "rpcUrl": "https://your-trusted-base-rpc.example",
      "router": "0x2222222222222222222222222222222222222222"
    }
  }
}
```

`Services::production` 先创建 provider registry，再对其链集合 bootstrap：

1. 未配置 Router 的链保留在 capabilities，`routerConfigured: false`；公开竞赛返回 `ROUTER_NOT_CONFIGURED`，不退回直接授权 provider。
2. 配置 Router 必须存在可用 RPC，并属于当前 provider 实例支持链的并集，否则启动失败。
3. 校验 `eth_chainId`；取一个区块高度，在同一高度检查 Router bytecode、调用 `allowanceHolder()`、检查 Holder bytecode。
4. getter 必须成功并返回严格的 32-byte address ABI；Holder 不能为零、native sentinel 或 Router 自身，且必须有代码。
5. 保存 `RouterDeployment { address, holder }`，HTTP listener 仅在所有配置 Router 校验通过后启动。没有固定 Holder 地址或降级 fallback。RPC 不可用、错误链、无代码、getter revert/错误返回和超时均失败。

代码存在和 getter 正确不等于通过合约审计，也不验证 RPC 的完整 simulation 能力。配置方负责确认可信 Router/Holder 实现；代理升级或部署变更后应重新核验并重启。正式 RPC 仍需支持 `eth_simulateV1`、state override，以及按需 `eth_createAccessList`。

Rust 使用发现的 Holder 做钱包 allowance 查询、approvals 和最外层 `exec` 交易入口；Router 作为 operator/inner target 和 provider quote sender。编码与当前 Solidity 一致：

```text
execute(sellToken, buyToken, receiver, sellAmount, minBuyAmount,
        deadline, spender, target, value, data)
```

公共 API 保持 `taker` 字段；它是交易发起者，且映射到合约 receiver，输入退款也归它。API 不新增独立 receiver。动态 Router/Holder 地址均不能作为 taker。

Rust 与 Solidity 都采用 permissionless 路由策略，不需要配置或登记 target/spender/selector。`Rule`、`Provider::rules()` 与 `ROUTE_NOT_ALLOWLISTED` 已移除。配置 Router 后仍需通过 provider 协议约束、route 校验和完整仿真；bootstrap 通过不等于生产执行已经验收。

## API 输入校验

`POST /v1/competitions` 使用 Serde typed request：非零整数 chainId、typed token/taker 地址（必须为 `0x` 前缀的 42 字符串，不接受 byte array）、正数 uint256 十进制 sellAmount 字符串、`slippageBps` 默认 30 且范围 `1..500`。拒绝未知字段、重复结构字段、非法类型、缺字段、金额溢出及非规范金额（前导零、小数、指数形式等）。跨字段检查继续拒绝同 token、native buy、零 token 和保留 taker；链能力和动态部署身份在竞赛入口检查。

这些错误通过现有公开错误 envelope 返回（通常 `400 INVALID_INPUT`；合法格式但保留的 taker 为 `400 INVALID_TAKER`），不泄露 serde、RPC 或配置细节。请求体仍限制为 16 KiB，不接受客户端指定 router、holder、RPC、calldata 或 storage slots。

## Docker

镜像不内置配置。将配置中的 `host` 改为 `0.0.0.0`，按需配置 `port`，以只读文件挂载 `/app/config.json`；运行用户 `nobody` 必须具有读取权限。不要把真实凭据 COPY 到镜像，也不要使用旧 `HOST` / `PORT` 环境变量覆盖。
