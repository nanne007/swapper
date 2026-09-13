# MetaMatch 项目架构与逻辑 v1

本文是当前 Rust 代码的 review 导航。代码、fixture 和测试是事实来源；本文记录模块职责、状态变化和必须保持的不变量。

最后核对：2026-09-13。

## 1. 一句话概览

MetaMatch 是一个单进程、短 TTL、非托管的多链 EVM exact-input 报价竞赛服务：provider 能力矩阵生成链 catalog 和 `chain -> provider` 反向索引，收到请求后只调度对应链的 provider，并发询价 → 固定 parent block 仿真 → 只按可验证结果排序 → build 时绑定真实 taker、重新询价和仿真 → 返回 unsigned transaction。

服务端不持有私钥，不签名、不广播；交易由用户钱包执行。

## 2. 组件关系

```text
调用方 / 钱包
 create → poll → build → sign → broadcast
                    │ HTTP/JSON
                    ▼
              Axum app
                    │
                    ▼
          Competitions + TTL state
             │                 │
             │                 ├─ ContextSource ── Alloy 2.x RPC
             │                 └─ Simulator ────── eth_call/simulateV1
             │
             └─ ProviderRegistry
                  chainId -> eligible providers
                    │
                    └─ fixed provider HTTP APIs

wallet → AllowanceHolder.exec → MetaRouter.execute → provider target
                                      │
                                      └─ minimum/output/refund/reentrancy
```

## 3. 文件和职责

| 文件 | 职责 | 不应放入 |
| --- | --- | --- |
| `src/main.rs` | 加载配置、启动 Axum、优雅退出 | 业务规则、签名、provider 逻辑 |
| `src/chains.rs` | 17 条链的 ID/name/slug catalog、RPC 环境变量映射 | 用户 token、动态远端发现 |
| `src/config.rs` | 服务参数、RPC URL、各 provider 原生 key | chain/provider policy、token 列表 |
| `src/domain.rs` | 输入、金额、route、quote、排序 | HTTP、RPC、provider 身份/能力、异步状态 |
| `src/providers/mod.rs` | Provider trait、registry/反向索引、共享 DTO/helper 和 route | 签名、广播、竞争状态 |
| `src/providers/*.rs` | 每家 provider 的独立 endpoint、认证、chain matrix 和 DTO 归一化 | registry、竞争状态、签名 |
| `docs/PROVIDER_INTEGRATION_GUIDE.md` | 13 家官方接入契约、申请前提、单位、native/spender/target 差异 | 运行时业务逻辑 |
| `src/competitions.rs` | capacity/TTL/auth、按索引调度、仿真编排、build | provider JSON 字段、ABI 细节 |
| `src/rpc.rs` | Alloy typed EVM RPC、block/gas/context | 资金不变量、route allowlist |
| `src/simulation.rs` | 固定 block 顺序仿真、余额/授权/到账/gas/reorg | provider schema、HTTP API |
| `src/execution.rs` | route 验证、ERC-20 ABI、Holder/Router calldata | 网络请求、状态生命周期 |
| `src/app.rs` | 路由、JSON 解析、body limit、错误 envelope、capabilities | 供应商协议、仿真细节 |
| `contracts/src/MetaRouter.sol` | 链上最终执行和资金边界 | 信任用户 calldata 或配置 |

## 4. 启动和能力发现

### 4.1 启动顺序

1. `main` 调用 `load_config_from_env`。
2. `configured_chains` 从静态 `CHAIN_CATALOG` 生成全部链；这不是用户配置的 enable list。
3. 读取 `RPC_URL_<chainId>`；无 RPC 的链仍进入 catalog，后续 context 明确返回 `RPC_NOT_CONFIGURED`。
4. `Services::production` 创建全部 13 个 provider，并调用 `ProviderRegistry::new`。
5. registry 遍历每个 provider 的 `supported_chains()`，与 catalog 求交集后写入 `by_chain`。
6. Axum 暴露 health、capabilities 和 competition API。

### 4.2 配置语义

| 配置 | 语义 |
| --- | --- |
| `HOST` / `PORT` | 监听地址 |
| `QUOTE_TTL_MS` | 竞赛 TTL，10 秒至 120 秒 |
| `PROVIDER_TIMEOUT_MS` | provider/RPC 操作超时，100 ms 至 30 s |
| `RPC_URL_<chainId>` | server-side EVM RPC；不是链开关 |
| provider 原生环境变量 | 只传给对应 adapter；必需 key 缺失时 `supported_chains()` 返回空集合，免 key provider 不受影响 |

当前没有 `CONFIG_PATH` 链配置、provider 配置或 token 配置。`config.example.json` 仅保留空 JSON 作为“无业务配置”的示例。

## 5. Provider 反向索引

每个 provider adapter 保存“该家当前官方 API 支持链与 v1 catalog 的交集”；Matcha 页面只负责定义 provider 名单。`supported_chains()` 再叠加本实例的认证条件。例如 0x 的逻辑等价于：

```text
supported_chains() = has_0x_key ? matrix(0x) : []
```

免 key provider 则不需要认证条件；optional key 只透传，不改变能力集合。最终 `ProviderRegistry.by_chain` 是竞赛唯一调度来源：

```text
by_chain[8453] = [0x, 1inch, barter, bebop, enso, kyber, odos, ...]
```

具体内容取决于本次进程实际配置的 key。`capabilities` 直接暴露这个结果，不暴露静态 token 列表，也不暴露未参与本次运行的 provider。

## 6. HTTP 和竞赛状态

### 6.1 创建

`parse_input` 验证 JSON 字段、地址格式、正整数 sell amount、滑点、taker 保留地址和 sell/buy 不相同；它不验证 token 是否在服务端名单中。

`Competitions::create` 只检查 chain ID 是否在 catalog，然后保存 `Input` 和对应 `Chain`。状态受 `max_competitions=500`、`max_active=20` 和 TTL 约束。

### 6.2 quote

```text
create
  └─ context(chain RPC)
  └─ registry.for_chain(chain.id)
       ├─ provider A quote
       ├─ provider B quote
       └─ provider C quote
             └─ validate route → simulate → Quote
```

每个 provider 独立处理 timeout、HTTP 错误、非法响应和仿真失败；失败写入自身 quote，不取消整个竞赛。token metadata 不入配置，也不调用额外价格源；同一请求的买入 token 已固定，`rank` 直接比较经过校验的整数 `quotedAmount`，不伪造 gas/token 价格折算。

### 6.3 polling/rank

polling 带 competition access token。`rank` 只把未过期、仿真成功、有有效 `quotedAmount` 的 quote 作为 verified；其余结果仍可用于诊断，但不能成为推荐。

## 7. Build 和仿真

build 必须带真实 taker 和调用方接受的最低到账值，重新获取 context 与 route。流程检查：

1. competition/quote/token 身份和 TTL；
2. taker 不为保留地址，且与创建时 taker 一致（如果创建时已绑定）；
3. provider route 的 sell amount、min amount、expiry、target、spender、selector、native value；
4. route allowlist 和 Router 配置；没有 Router 直接返回 `ROUTER_NOT_CONFIGURED`；
5. 重新仿真必须实际成功，且输出不少于用户接受底价；
6. 返回 approval transaction（若需要）和统一 swap transaction。

固定 block 仿真在 parent block 上读取余额/授权、按顺序执行 approve/swap、读取余额差和 gas，并二次确认 block hash。`eth_simulateV1` 不支持是 `unsupported`；RPC 错误是 `error`；链上 revert 是 `reverted`。

## 8. 不变量

- 金额内部使用 Alloy `U256`，JSON 使用十进制字符串；不使用浮点金额。
- token 是不可信输入；不能因为 provider 返回了 target 就自动信任，必须经过 route 边界和 allowlist。
- 服务端不签名、不广播；provider response 不能把服务状态伪装成链上成功。
- Router 使用精确 spend、临时授权、实际到账 minimum、退款增量、pause 和 reentrancy 保护。
- 每条链的 provider 集合只能来自 registry；不能在竞赛中遍历未过滤全集。
- provider 部分失败不能吞掉其他结果，也不能用 mock/fallback 冒充 live 报价。

## 9. 当前实现状态与 review 入口

已实现：多链 catalog、13 个字符串 provider ID、各 adapter 自有支持矩阵、key-aware `supported_chains()`、反向索引、无 token 白名单输入路径和 13 家 provider route adapter。

已有 13 个独立的 ignored live HTTP 测试。无 key 环境下 Bebop、Kyber、LiquidSwap、Velora 返回并归一化真实报价；Odos 当前由 Cloudflare 530/1033 阻断，OpenOcean gasPrice 成功但 swap 被出口 IP 策略 403 阻断；缺必需 key 的其余 provider 未触网。fixture 和 reachability 都不能证明链上可执行，当前快照与未验证边界见 [VERIFICATION.md](VERIFICATION.md)，逐家接入契约见 [PROVIDER_INTEGRATION_GUIDE.md](PROVIDER_INTEGRATION_GUIDE.md)。

review 顺序建议：先看各 `src/providers/*.rs` 的 ID、supported chains、凭证要求和 rules，再看 `providers/mod.rs` 的 `ProviderRegistry` 与共享边界，然后按 provider 文件检查各自 endpoint，接着看 `Competitions::run_live` 的调度，最后看 `validate_route`、`simulation` 和 Solidity 资金不变量。所有测试从 `tests/core.rs`、`tests/providers.rs`、`tests/e2e_local.rs` 进入。
