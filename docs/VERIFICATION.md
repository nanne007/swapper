# 实现与验收记录

本记录描述当前 Rust runtime、本地验证和未验证生产边界，不表示服务已经过主网审计或可以直接投入真实资金。

## Rust 最小核心验收（2026-09-12）

根目录 Cargo crate 是唯一构建、启动和 CI 入口。

| 检查 | 结果 | 覆盖与边界 |
| --- | --- | --- |
| `cargo +1.94.1 fmt --all -- --check` | 通过 | Rust 源码格式 |
| `cargo +1.94.1 clippy --all-targets --all-features -- -D warnings` | 通过 | 所有 target、所有 feature，警告视为错误 |
| `cargo test --all` | 生产 lib/main 为 0 个测试；`tests/api.rs` 3/3、`tests/core.rs` 18/18、`tests/providers.rs` 15/15 通过；1 项 Anvil E2E 和 13 项 live provider test 默认 ignored | API rejection/header/cache 契约、domain/config、HTTP/provider/RPC fixture、ABI、仿真失败分类、轮询生命周期、build 边界和 live test discovery；测试与 fixture 全部位于 `tests/` |
| `cargo +1.94.1 check --all-targets` | 通过 | Alloy 2.x typed RPC 与所有 target 编译 |
| `cargo +1.94.1 build --release` | 通过 | Rust release binary |
| `cargo +1.94.1 test --test e2e_local -- --ignored --nocapture` | 1/1 通过 | 本地 Anvil：HTTP → `eth_simulateV1` → approval → rebuild → swap |
| `forge fmt --root contracts --check` | 通过 | Solidity 格式 |
| `forge build --root contracts --deny-warnings` | 通过 | Foundry 合约编译 |
| `forge test --root contracts` | 27/27 通过 | minimum output、授权、历史余额、sender 伪造、重入和嵌套 Holder |
| `git diff --check` | 通过 | 差异无空白错误 |

当前验证没有使用真实供应商 key、生产 RPC、钱包或主网广播；本地 E2E 不是主网 fork，也不证明正式 Holder、Router 或真实供应商可执行。

## v1 多链切片验收（2026-09-12）

本切片新增的确定性检查：

| 检查 | 结果 | 覆盖 |
| --- | --- | --- |
| `CHAIN_CATALOG` 生成 | 通过 | 17 条 EVM 链，不需要 chain 配置 |
| provider ID 与矩阵 | 通过编译/单测 | 13 个 Matcha provider；Monad 使用当前 mainnet `143`，不再保留历史 testnet `10143` |
| key-aware `supported_chains()` | 通过编译/单测 | 必需 key 缺失返回空集合并不进入索引；免 key provider 可进入 |
| `ProviderRegistry` 调度 | 通过编译 | 竞赛按 `chain -> provider list` 选择 provider |
| token 输入路径 | 通过现有测试 | 不再依赖服务端 token 白名单，地址直接进入 quote |
| 13 家 provider fixture | provider 测试 15/15 通过；全量 Rust 测试通过 | 0x spender/target 分离、Enso POST arrays、Barter minReturn、LiquidSwap token schema、Odos native marker、OKX v6 approval/minimum、OpenOcean gasPrice/native mapping 等官方契约 |
| Axum API rejection/header contract | 3/3 通过 | typed JSON/path/header extraction、统一 error envelope、404/405、`WWW-Authenticate`、`Cache-Control: no-store` |
| quote 排序 | 通过单测/编译 | 同一 chain + buy token 下按已仿真的整数 `quotedAmount`，不使用固定 decimals |

当前实现状态：13 个 provider 均有实际 HTTP adapter 和脱敏 fixture 契约测试；fixture 只证明本地解析、参数构造和安全拒绝路径，不证明真实上游额度、实时流动性或链上执行。

## Provider live smoke（2026-09-13）

新增 [tests/providers_live.rs](../tests/providers_live.rs)，为全部 13 个 provider 提供独立的 ignored live test。测试使用生产 adapter、真实 `ReqwestClient` 和固定的 provider endpoint；不使用 `MockHttp`，不签名、不广播、不写链。需要 key 的 provider 直接复用进程环境中的原生 key；缺少 key 时由 `supported_chains()` 产生明确 `SKIP`，免 key provider 仍会尝试访问。测试内提供 Ethereum/Optimism/Base/Arbitrum 的 WETH/USDC、HyperEVM 的 WHYPE/USDT0、Berachain 的 native/HONEY 默认输入；这些只服务于 live test，不进入产品 token registry。

执行命令：

```sh
METAMATCH_RUN_LIVE_PROVIDER_TESTS=1 \
  cargo test --test providers_live -- --ignored --nocapture
```

每个实际发出请求的测试都要求所有 HTTP response 为 2xx，并要求生产 adapter 返回通过金额、链、token、taker、target 和 calldata 校验的归一化 `Route`。403、其他非 2xx、仅触达但无报价、response 解析失败都会使测试失败。该测试不应放入默认 CI，因为上游实时流动性、限流、IP 策略、账户权限和 token 支持会变化。

2026-09-13 无 provider key 环境下，在官方接入复核与修正后的真实运行快照：

- Bebop：HTTP `[200]`，`0.01 WETH -> 24.954423 USDC`，minimum 同为 `24.954423 USDC`。地址使用 EIP-55，firm `minimumAmount` 由上游提供。
- Kyber：修复共享 Reqwest 缺少 User-Agent 后 HTTP `[200, 200]`，`0.01 WETH -> 25.213918 USDC`，minimum `24.961778 USDC`。同一个 URL 在空 User-Agent 时返回 403、带明确产品 User-Agent 时返回 200，因此该 403 不是 access key 问题。
- Velora：HTTP `[200]`，`0.01 WETH -> 25.219462 USDC`，minimum `24.967267 USDC`。
- LiquidSwap：公开 HyperEVM RPC 与 route endpoint 均 HTTP `[200, 200]`，`0.01 WHYPE -> 0.792895 USDT0`，minimum `0.784966 USDT0`；当前只接受文档明确的 ERC-20 输入。
- Odos：HTTP `[530]`，response 为 Cloudflare error 1033；这是上游 tunnel/edge 不健康，不是缺少 API key。严格 live test 正确失败。
- OpenOcean：gasPrice HTTP 200，随后 swap HTTP 403。官方错误表将 401/402 归类为 Pro key 错误，将 403 归类为 IP 白名单/安全策略，所以该结果不是“公开 endpoint 少了 access key”。需联系 OpenOcean 加白，或取得正式 Pro/Enterprise host 与认证契约后再接入。
- 0x、1inch、Barter、Enso、HyperBloom、OogaBooga、OKX：缺少必需 credential，按 provider 自身能力返回明确 `SKIP`，没有发出未认证请求。

最终 full live suite 结果为 11 个测试进程通过、2 个严格失败：7 个必需 key provider 以 `SKIP` 结束，Bebop/Kyber/LiquidSwap/Velora 返回真实报价，Odos/OpenOcean 因上述外部状态失败。非零退出码是 live 门禁的正确行为。

以上价格只是该次请求的瞬时 quote 证据，不等于成交保证，也没有经过生产 RPC 仿真或链上执行。

## 当前 v1 核心

- 支持 Matcha provider 支持链并集的 catalog、13 个 provider adapter、固定 parent block 仿真、polling 快照和重新报价 build。
- 不维护 token 白名单；未知 token 的最终可用性由 provider 和链上仿真决定。
- 缺少必需供应商凭据时不发起该 provider 请求，返回 `unavailable`；免 key provider 默认参与；没有 Router 时只允许 direct preview，build 明确失败。
- 服务只返回 unsigned approval/swap 交易，由调用方钱包完成签名和广播。
- 状态保存在单进程内存中，具备 TTL、容量限制、Bearer token、请求体上限、超时和错误清洗。
- Solidity 合约仍由 Foundry 维护和验证；本次未修改合约设计。

## 未验证边界

1. Provider live smoke 已落库，但真实报价、费用、地址和执行可用性仍取决于运行时提供的 key、链专用 token、上游限流和生产 simulate RPC；live smoke 不广播交易，也不等价于主网 fork 或正式执行验证。
2. 本地 E2E 的 Holder 是测试 mock 写入固定地址，不是从主网读取的正式字节码；正式 Holder 与嵌套路由必须补 fork 验证。
3. Router 未部署、未外部审计；服务不签署、不广播真实网络交易。本次唯一广播发生在测试进程创建的隔离本地 Anvil，测试完成即关闭。
4. 真实 provider endpoint 的访问路径已有可显式运行的 smoke test，但每个 key 的权限、实时流动性、生产 RPC、正式 Router/allowlist 和主网执行仍需逐环境验证；不能把 fixture 或 reachability 通过当作 live E2E。
5. 跨链、非 EVM、原生币 buy、intent、平台抽成和公网身份系统不属于当前最小核心。
6. Redis/Postgres、多副本、分布式限流、业务审计库、供应商熔断和 RPC 容灾未实现；当前是有界单进程版本。
7. Dockerfile 已提供，但 Docker daemon 不可用，镜像 build/run 未验证。
8. 仿真、费用估算和报价都不是未来成交保证；必要 approval 后必须重新 build，最终仍由链上 minimum output 保护。

## 文件位置与交付

本次修改直接发生在 `/Users/caojiafeng/Documents/ChatGPT/metamatch`，没有创建 commit、push、部署或广播交易。生成的 `target/`、Foundry 构建物和本地环境文件不纳入交付。详细处理过程、删除范围和判断依据见 [RUST_REWRITE_LOG.md](RUST_REWRITE_LOG.md)。

## 上线前顺序

申请并核对供应商服务权限 → 配置专用 simulate RPC → 正式 Holder/Router/provider fork 测试 → 外部合约审计 → 多签部署与最小白名单 → 完整监控/访问控制 → 小额人工验收。其他链要另立产品范围并补齐费用模型、route 验证和测试。
