# 实现与验收记录

本记录描述当前 Rust runtime、本地验证和未验证生产边界，不表示服务已经过主网审计或可以直接投入真实资金。

## Rust 最小核心验收（2026-09-12）

根目录 Cargo crate 是唯一构建、启动和 CI 入口。

| 检查 | 结果 | 覆盖与边界 |
| --- | --- | --- |
| `cargo +1.94.1 fmt --all -- --check` | 通过 | Rust 源码格式 |
| `cargo +1.94.1 clippy --all-targets --all-features -- -D warnings` | 通过 | 所有 target、所有 feature，警告视为错误 |
| `cargo test --all` | 生产 lib/main 为 0 个测试；`tests/api.rs` 3/3、`tests/core.rs` 20/20、`tests/providers.rs` 15/15 通过；1 项 Anvil E2E 和 13 项 live provider test 默认 ignored | API rejection/header/cache 契约、domain/config、HTTP/provider/RPC fixture、ABI、仿真失败分类、轮询生命周期、build 边界和 live test discovery；测试与 fixture 全部位于 `tests/` |
| `cargo +1.94.1 check --all-targets` | 通过 | Alloy 2.x typed RPC 与所有 target 编译 |
| `cargo +1.94.1 build --release` | 通过 | Rust release binary |
| `cargo +1.94.1 test --test e2e_local -- --ignored --nocapture` | 1/1 通过 | 本地 Anvil：HTTP → `eth_simulateV1` → approval → rebuild → swap |
| `forge fmt --root contracts --check` | 通过 | Solidity 格式 |
| `forge build --root contracts --deny-warnings` | 通过 | Foundry 合约编译 |
| `forge test --root contracts` | 27/27 通过 | minimum output、授权、历史余额、sender 伪造、重入和嵌套 Holder |
| `git diff --check` | 通过 | 差异无空白错误 |

以上本地门禁没有使用真实供应商 key、生产 RPC、钱包或主网广播；另行授权的真实重放见下方 2026-09-14 记录。本地 E2E 不是主网 fork，也不证明正式 Holder、Router 或真实供应商可执行。

## v1 多链切片验收（2026-09-12）

本切片新增的确定性检查：

| 检查 | 结果 | 覆盖 |
| --- | --- | --- |
| provider 链并集生成 | 通过 | `ProviderRegistry::chain_ids()` 直接对当前实例 `supported_chains()` 去重排序；无独立 catalog |
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

- 支持从 Matcha provider 当前能力直接派生链并集、13 个 provider adapter、固定 parent block 仿真、polling 快照和重新报价 build。
- 不维护 token 白名单；未知 token 的最终可用性由 provider 和链上仿真决定。
- 缺少必需供应商凭据时不发起该 provider 请求，返回 `unavailable`；免 key provider 默认参与；没有 Router 时只允许 direct preview，build 明确失败。
- 服务只返回 unsigned approval/swap 交易，由调用方钱包完成签名和广播。
- 状态保存在单进程内存中，具备 TTL、容量限制、Bearer token、请求体上限、超时和错误清洗。
- Solidity 合约仍由 Foundry 维护和验证；本次未修改合约设计。

## alloy-chains 与 Alchemy RPC 兜底（2026-09-14）

- `alloy-chains` 作为直接依赖升级并锁定为 `0.2.38`；运行时通过 `Chain::from_id` 解释 provider 返回的 chain ID，不维护 `CHAIN_CATALOG`。
- 按 Alchemy 官方 Chain API 列表核对当前 provider 并集 17 条链的 mainnet HTTPS base URL；`alchemy_rpc_url(chain_id)` 返回不含 key 的 URL，配置阶段再把 `ALCHEMY_API_KEY` 作为 path segment 编码。
- RPC 优先级测试覆盖 `RPC_URL_<chainId>` 高于 Ethereum 兼容别名和 Alchemy，兼容别名高于 Alchemy；无 key 时不生成 fallback，非法显式 URL 继续 fail fast。
- `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --all`、`cargo build --release` 全部通过；Rust tests 为 API 3/3、core 20/20、provider 15/15，生产 `src/` 仍为 0 个测试。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`、`forge test --root contracts` 通过，合约 27/27；`cargo test --test e2e_local -- --ignored --nocapture` 1/1 通过。
- capabilities 契约测试确认 response 不含 Alchemy host、API key 或 `rpcUrl` 字段；只保留 `rpcConfigured` 布尔状态。本轮没有读取、打印或请求用户的真实 Alchemy key，也没有消耗生产 RPC quota。endpoint 映射和 URL precedence 已确定性验证，但各链 Alchemy 账户权限、配额以及 `eth_simulateV1`/state override 能力仍需部署环境 live 验证。

## 真实请求重放与错误系统（2026-09-14）

用户明确要求重放真实请求并重新设计内部/API 错误后，使用本地配置的 provider key 和 RPC；没有输出 key、认证 header、access token 或完整 RPC URL。测试通过真实 Axum 路由和生产服务调用上游，不替换 HTTP/RPC 为 mock：

```sh
METAMATCH_RUN_LIVE_REPLAY=1 cargo test --test replay_live -- --ignored --nocapture
```

输入与用户故障报告一致：Ethereum，native ETH `0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee` → USDC `0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48`，sellAmount=`1000000000000000000`，slippageBps=`30`，不提供 taker。

### 首次重现和根因

首次加诊断后的重放时间为 `2026-09-14T11:34:42Z`，competition `0fcf219d-5d41-4765-923f-b7406373496e`，parent block `0x18c5a85`；没有一家仿真成功，测试按设计失败。

| 范围 | 实际上游证据 | 处理 |
| --- | --- | --- |
| 0x、Bebop、Kyber 的仿真 | `eth_simulateV1` JSON-RPC `-38014`，`insufficient_funds` | preview 旧注资按 2M gas，但实际 swap gas limit 为 8M。改为基于完整 payload 计算 value + 所有付费 call 的 gas-limit 预算，余额探针 gasPrice=0；不修改真实 taker 余额 |
| Enso | HTTP 400：`slippage must be a number string` | bps 数字改为字符串；随后真实 response 的同链 route leg 不含 `chainId`，改为可选且存在时必须匹配，明确跨链仍拒绝 |
| Velora | HTTP 400：`userAddress` validator 拒绝旧 `0x…0a11ce` preview 地址，checksum 单独转换也未解决 | 统一改用 `keccak256("MetaMatch preview account")` 的低 20 bytes 作为保留 preview 地址，provider 使用 EIP-55；不伪造真实用户身份 |
| OpenOcean | gasPrice HTTP 200，但 `data.standard` 为对象，旧整数解析失败；修正后 swap HTTP 403 | 接受 `standard.legacyGasPrice` 的 wei 值并保留 scalar 形态。403 对应公开 endpoint 的访问策略，不能仅凭此断定缺 key，也不通过换身份/IP 绕过 |
| Odos | HTTP 530，Cloudflare error 1033 | 上游 tunnel/edge 错误，未伪装成缺 key 或成功；保留明确 provider 局部失败 |

重放过程还观察到一次 Kyber HTTP 503，以及一次 Bebop 仿真到账低于 minimum；均保留真实失败，不削弱 minimum 校验，不代表最后快照中一定重复出现。

### 修复后真实快照

`2026-09-14T11:41:28Z`（北京时间 19:41:28），competition `3316c274-f0bb-47ab-942e-7e779ee388cf`，parent block `0x18c5aa7`，hash `0xa4a594c52e4992897de56532792c2af06920f1418e886aa954a5c14767613e03`。

金额以下均为 USDC **base units**，避免把显示精度引入产品 token registry：

| Provider | quotedAmount | 仿真 boughtAmount | gasUsed | 结果 |
| --- | ---: | ---: | ---: | --- |
| Kyber | 2513062029 | 2512742305 | 519000 | success / overridden |
| Velora | 2512806314 | 2512491031 | 307504 | success / overridden |
| Enso | 2512742305 | 2512742305 | 292801 | success / overridden |
| 0x | 2508970890 | 2508973192 | 222276 | success / overridden |
| Bebop | 2487381590 | 2487381590 | 303993 | success / overridden |
| Odos | — | — | — | HTTP 530 / Cloudflare 1033 |
| OpenOcean | — | — | — | swap HTTP 403 |

推荐 quote ID=`f12c31e4-17e7-497b-82a9-8dfaef893012`，provider=Kyber；测试 1/1 通过。重放测试的断言是整体竞赛至少有一家真实仿真成功，不是“所有 provider 接入都通过”；两家外部失败仍需单独处理。价格只是上述时刻快照，不是现在可成交报价。

这是生产 HTTP + 生产 RPC 的 **direct-preview** 证据，所有成功仿真 funding=`overridden`。没有 taker，因此不证明用户真实余额、授权或主网成交；没有部署、签名或广播真实网络交易，也没有验证正式 Holder/MetaRouter 路径。其他链与未参与本请求的 provider 不能据此视为已验收。

### 错误体系与回归门禁

- `Fault` 与 `ApiError` 分离：前者持有 `anyhow::Error` source/context/backtrace，不持有 HTTP status、不序列化；后者封闭映射公开 code/status，未知内部 code 只返回 `INTERNAL_ERROR`。
- HTTP 保留 status/选定 problem 字段，JSON 使用 `serde_path_to_error`，RPC 保留原始 Alloy 错误；逐笔 simulate revert 保留 call index、错误和返回 bytes。常规日志不展开私密 source，只记录安全上下文和可选 backtrace。
- `tests/errors.rs` 5/5：cause 经上下文/build 包装后仍可访问、启用后的 backtrace 确为 Captured、HTTP/JSON 路径与 RPC numeric code 保留、revert 信息不泄漏、provider deadline 不丢 quote。
- `tests/providers.rs` 17/17：新增 Enso 字符串滑点/省略 leg chainId 和 OpenOcean EIP-1559 gas 对象回归，明确跨链仍拒绝。
- fixture preview 账户原生余额改为 0，并验证每笔 call 的 upfront gas-limit 成本；原先富余额 fixture 掩盖了本次真实故障。新增断言是确定性回归，不替代以上 live 证据。
- 最新 `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`RUST_LIB_BACKTRACE=1 cargo test --all`、`cargo build --release` 通过。Rust 为 API 3 + core 20 + provider 17 + errors 5 = **45/45**；生产 src 无测试，15 项外部/Anvil 测试默认 ignored。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings` 通过；Foundry **27/27**，本地 `cargo test --test e2e_local -- --ignored --nocapture` **1/1**。Foundry 工具自身提示 nightly build 和 `--deny-warnings` 将弃用，本次没有 Solidity 编译警告或门禁降级。
- live 成功快照之后仅补充了错误元数据/revert 诊断和测试；最终质量门验证这些变更，未为刷新快照再次消耗生产配额。

## 直接使用 anyhow 的错误体系简化（2026-09-14）

用户确认删除通用 `Fault`，本轮仅重构错误表示与传播，没有再次调用生产 provider/RPC。

- 所有内部接口直接返回 `anyhow::Result<T>`；没有 `Fault` struct/alias、自定义 Result、`report()` 转发或复制的 diagnostic context 字符串。原始库错误使用 `?` 与 `Context` 传播，明确失败使用 `bail!`。
- `ErrorKind` 只是 thiserror enum，作为 typed context 放进 anyhow；API 通过 typed downcast 分类，保留原有公开错误码与 HTTP status。对迁移前后映射逐项比较，52 对 code/status（含默认 INTERNAL_ERROR）完全一致。仿真层不再引用 `api_error`。
- 已知 HTTP/RPC/revert 类型的原始信息保留，日志由 `diagnostics::log_error` 从 typed source 提取白名单字段；不会格式化 anyhow Debug 或任意字符串 context。`RpcMethod` 仅是操作元数据 context，不包装 source 或 report。
- `tests/errors.rs` 保留原有行为断言，并直接验证 anyhow 原始 cause/downcast、最外层 typed 分类优先、启用后的 backtrace、API envelope、真实捕获的日志脱敏。`anyhow::Error` 本身允许包含私密数据，因此脱敏断言位于实际日志/API 边界，不再要求它的原始 Debug 隐藏数据。
- 新增 `tests/startup.rs`：隔离子进程使用仅含虚构测试值的坏 `.env`，确认非零退出、输出 `INVALID_CONFIG` 且不输出变量名/值，不读取开发者配置。服务入口使用 `ExitCode` 显式记录安全诊断，避免 `main -> Result` 自动打印源错误。
- Rust fmt、Clippy、`RUST_LIB_BACKTRACE=1 cargo test --all`、release build 通过；测试 API 3 + core 20 + provider 17 + errors 5 + startup 1 = **46/46**。15 项 Anvil/生产网络测试默认 ignored，src 中没有测试。
- Foundry fmt/build/test 通过，**27/27**；显式本地 Anvil E2E **1/1**。没有添加或升级依赖，没有变更报价参数、资金 override、最低到账、allowlist、配置文件或公开 JSON schema。

## 简化内部日志（2026-09-14）

按用户最新要求，暂不实现安全日志；本节替代前述安全日志设计，旧验证记录仅描述当时行为。

- 删除 `diagnostics`、日志字段白名单、文本原因分类器、`RpcMethod` 元数据和自定义脱敏 Debug。失败边界直接由 tracing 格式化 anyhow 原因链，启动入口返回 `anyhow::Result<()>`；HTTP 错误保留 status/body，沿用原有响应大小上限。
- `tests/errors.rs` 验证原始 HTTP/RPC/revert cause、上下文、typed downcast、backtrace 与 API 详情隔离；不再维护自制日志捕获工具。`tests/startup.rs` 在隔离子进程中验证坏 dotenv 的上下文和非零退出，所有数据均为虚构测试值。
- Rust fmt、Clippy `--all-targets --all-features -- -D warnings`、`RUST_LIB_BACKTRACE=1 cargo test --all`、release build 全部通过；Rust **46/46**，15 项网络/Anvil 测试默认 ignored。Foundry fmt/build/test 通过，**27/27**；另行运行本地 Anvil E2E **1/1**。
- `git diff --check` 与 `git diff --cached --check` 通过，`src/` 无测试。未增加依赖，未修改开发者 `.env`，未调用生产 provider/RPC。公开 API 的 code/status/envelope 与报价、仿真资金规则保持不变。

## 不减功能精简 A–D（2026-09-14）

本轮基于已有工作区精简实现，保留全部 provider、异步竞赛、polling/build、公开响应字段、排序和资金保护。没有运行生产网络测试；以下是重新执行的本地证据，不复用前述历史 live 结果作为通过依据。

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --all-targets --all-features -- -D warnings` | 通过 |
| `RUST_LIB_BACKTRACE=1 cargo test --all` | **53/53**；API 3、并发 1、core 20、errors 5、provider 17、精简回归 6、startup 1 |
| `cargo build --release` | 通过 |
| `forge fmt --root contracts --check` | 通过 |
| `forge build --root contracts --deny-warnings` | 通过；无合约源码变更，复用构建缓存 |
| `forge test --root contracts` | **27/27**，含 3 项 fuzz（各 256 runs） |
| `cargo test --test e2e_local -- --ignored --nocapture` | **1/1**，隔离本地 Anvil 的 Rust HTTP → 仿真 → approval → swap |
| `git diff --check` / `git diff --cached --check` | 通过 |

默认套件仍有 15 项 ignored（13 个 provider live、1 个生产 replay、1 个另行运行的 Anvil）。生产 `src/` 中没有测试，未增加或升级依赖。Foundry 提示当前安装为 nightly，且 `--deny-warnings` 参数未来弃用；命令退出成功，没有为通过而放宽质量门。

新增/迁移的验证：

- `tests/concurrency.rs` 先在旧手工计数实现上复现 abort build 后名额泄漏，再验证 Semaphore permit 自动释放；暂停点使用 Notify，不依赖竞争时序。
- `tests/simplification.rs` 覆盖 build 普通失败后释放名额；`BUILD_REVERTED / BUILD_UNSUPPORTED / BUILD_ERROR` 分类及原始 anyhow cause 保留；RPC endpoint 客户端复用；固定 block、前后 probe gasPrice=0、preview 注资与 actual 资金不足拒绝；未配置/验证失败的 ERC-20 balance slot、approval false；共享交易 value/calldata/expiry 校验；create/snapshot/quote/build/simulation JSON 字段与 optional 行为。
- RPC fixture 由自定义 RPC trait 改为本地 HTTP JSON-RPC 服务，原有 core/errors 测试经过真实 Alloy 编解码。`tests/errors.rs` 继续检查原始 transport error、method、逐笔 revert data、typed downcast、backtrace 和 API 详情隔离。
- 对改动 DTO 的结构体字段逐项比较新旧 Serde wire name，未发现字段名变化；保留非标准 provider 字段名，不改变 adapter 请求参数或认证。归一化重用现有 calldata 上限，多个字段同时非法时首个错误可能因校验顺序改变。

本轮 Rust 源码净减少 303 行（4,690 → 4,387，以本轮开始时的工作区为基线，含注释/空行），不把测试迁移或既有改动计入生产源码精简。详见 [处理日志](RUST_REWRITE_LOG.md)。本地结果不证明 macOS 以外的 CI 环境、正式 Holder/Router、真实 provider 权限或主网成交。

## 未验证边界

本节之前的 live 记录是历史运行快照，不因后续结构重构自动成为新的 live 验证。

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
