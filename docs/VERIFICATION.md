# 实现与验收记录

本记录描述当前 Rust runtime、本地验证和未验证生产边界，不表示服务已经过主网审计或可以直接投入真实资金。

本文件按日期保留历史结果；旧章节中的环境变量、poll/build、管理员或白名单状态不代表当前实现。最近的维护成本清理见下节，Rust 优化与配置迁移记录保留在其后；当前合约以 [合约说明](../contracts/README.md) 为准。

## 非核心维护成本清理（2026-09-15）

本轮仅精简重复模型、应用组装、无调用辅助代码和开发流程；竞赛调度、报价归一化、整数计算、完整仿真及 Solidity 没有改动。没有新增依赖。

| 维护负担 | 已实施处理 |
| --- | --- |
| CreateCompetitionRequest 与 Input 重复六个字段，增加字段必须同步转换 | 合并为 Input 的 Serialize/Deserialize；Serde 保留结构与字段范围检查，显式 Input::validate 保留 token/taker 业务错误分类；不再逐字段转换 |
| App/AppState 各只包一个字段，调用方绕经 app.router/state.competitions | 应用构造直接返回 Axum Router，State 直接共享 Arc<Competitions>；保留 create_app_with_services 测试注入入口 |
| 旧 preview 常量和转发 helper 残留在生产接口或测试支持代码 | 地址移至 tests/support 的 FIXTURE_TAKER；删除仅供测试调用的 json_request 包装，测试直接使用 json_request_as；删除无调用的 ProviderRegistry::get、fixture_registry、parse_fixture_address，以及纯转发的 fixture_app |
| 本地、CI、README 和技能分别复制完整质量门，容易漏掉步骤 | scripts/check.sh 成为完整命令序列的唯一来源，CI 与文档共用，包含全部八项检查并遇错停止 |
| 合约技能仍要求管理员/白名单/recover，provider 技能仍要求 preview/build | 同步当前 permissionless、sender/receiver、净 sold、无关资产边界和单请求流程；修正 README 的二次重建描述，历史证据不删除 |

保留有实际职责的边界：ConfigDocument → Config 承载启动前完整校验；13 个 provider adapter 对应不同上游协议；内部 anyhow/ErrorKind 与公开 ApiError 保持独立。未为减少文件或行数强行合并这些职责。Provider trait、配置格式、HTTP JSON 字段和公开错误码保持不变；Rust 库调用点已迁移到 Input/Router，旧 App/CreateCompetitionRequest/helper 接口不再提供兼容别名。

新增 `input_wire_contract_preserves_defaults_ranges_and_error_kinds`，先在合并前实现上通过，再迁移到 Input 并补 round-trip 断言。覆盖默认滑点、u64/U256 最大值、非法类型/零/非规范金额/未知字段，以及 INVALID_INPUT 与 INVALID_TAKER 的区别。既有测试仅迁移类型、路径和测试常量名，不放宽原有断言。

验证：`CARGO_NET_OFFLINE=true sh scripts/check.sh` 完整通过：Rust fmt/Clippy、**98 个默认 Rust 测试**、release build、Foundry fmt/build/**50 项测试**、显式本地 Anvil **1/1**。默认 Rust 套件仍有 15 项 ignored，其中 Anvil 在最后一步另行运行，13 个 provider live 和 1 个生产 replay 不开启。Foundry nightly 与旧 flag 提示仍存在，未降低警告门禁。另检查 shell 语法与 staged/unstaged 空白差异。

相对本轮开始的暂存基线，生产 Rust 为 **4,334 → 4,294 行，净减少 40 行**（含注释/空行）；维护收益主要是减少字段同步、空包装和过时入口，不声称生产吞吐或延迟提升。执行期间检测到部分 Rust 修改被外部暂存，保留该状态；本 agent 没有 stage/commit，也未读取或修改开发者配置、访问生产 provider/RPC、部署或广播真实网络交易。GitHub 托管 Ubuntu CI、Docker、正式 Holder/provider fork 和生产执行仍未重新验证。

## Simulation prepare 去除重复 code/allowance 读取（2026-09-15）

按当前运行时边界，Router 与 Holder code 仍在监听前由 bootstrap 在同一区块验证；`SimulationProvider::prepare` 不再为每轮重复读取 Router code。prepare 也不再调用 sell token `allowance(taker, Holder)`：ERC20 固定生成一笔 `approve(Holder, sellAmount)`，native 不生成 token approval。删除未再使用的 allowance ABI helper 和 fixture allowance 分支。

这笔固定 approval 与 swap 一起进入 `eth_simulateV1`，返回 false、revert 或不兼容的 nonzero-to-nonzero approve 仍使对应 route 失败。服务不会为了兼容这类 token 自动先发 `approve(0)`；因此每个 ERC20 quote 固定一笔而不是两笔 approval。已有足额或永久 allowance 不再省略 approval，执行后可能被替换为本次 sellAmount，这是本次明确的 API 行为变化。

确定性 fixture 中，5 条有效、已配置 balance slot 的 ERC20 routes 从 **16 次降为 14 次逻辑 RPC**：1 chainId、1 gasPrice、7 block 查询（context、一次 prepare pre-check、5 次独立 post-check）和 5 simulation；prepare 阶段 `eth_getCode=0`、allowance `eth_call=0`。冷 mapping base 仍可能增加至多 2 次 `eth_createAccessList`。每个 simulation 调用序列固定为 buy balance → approval → swap → buy balance。

回归 `five_permissionless_routes_share_preparation_but_not_simulation` 和 `shared_preparation_always_approves_erc20_without_an_allowance_probe` 先在旧实现上分别以 16≠14、allowance call 1≠0 失败，修改后通过；native/approval false、固定 block、funding override、call count 与独立 reorg 检查继续覆盖。

`CARGO_NET_OFFLINE=true sh scripts/check.sh` 最终完整通过：Rust fmt、Clippy、**98 个默认测试**、release build，Foundry fmt/build/**50 个测试**，以及隔离本地 Anvil E2E **1/1**。默认 ignored 的 13 个 provider live 和 1 个生产 replay 未开启；没有访问生产 provider/RPC、部署、签名或广播。Foundry nightly 与 `--deny-warnings` 弃用提示仍存在，未降低门禁。GitHub 托管 CI、正式 Holder/provider fork、生产 token 的 nonzero-to-nonzero approve 兼容性仍未验证。

## Rust 精简、共享准备与 permissionless 路由（2026-09-15）

本节记录上述 prepare 精简之前的性能基线；其中 Router code、Holder allowance 和 16 次 RPC 的描述已由上一节替代。

用户确认采用 permissionless 后，删除 Rust `Rule`、`Provider::rules()`、三元组白名单预检及 `ROUTE_NOT_ALLOWLISTED` 分类/映射。没有新增路由登记配置，也未修改 Solidity。完整 Holder/Router 仿真、provider 原生地址/状态覆盖限制、最低到账、固定区块、真实 taker 和单个总 deadline 保留。

- 同一轮在所有 routes 结算、公共 context 获取后，只进行一次模拟前 hash、Router code、Holder allowance 与卖出 token mapping base 准备；独立的准备读取并行进行。不可变准备数据供各 route 消费，每条 route 新建 state overrides/payload，模拟后分别检查 hash。准备错误分发给该轮有效 routes，不进行每 route 重试、不跨轮缓存失败。取消释放 future 与竞赛容量；成功项 latencyMs 仍计入自己的 quote、共享准备和 simulation 耗时，不把准备移出指标。
- Axum state 改为 Arc，共享 registry/chains；health 不提取 state。HTTP 成功路径直接解码 typed DTO，不构造整棵 Value；保留 HTTP status/body、Serde 路径、语法与结构错误分类及尾随 JSON 拒绝。重复的已知 DTO 字段现在被拒绝，不再经 Value 合并后取最后一个。
- `Tx.data`/ABI helper 使用 Bytes，provider hex 只解码一次；公开 JSON 的 data 仍是 hex 字符串。RPC 仅发送 input，不再同时复制 data。金额 JSON 仍是规范十进制字符串。
- 编码只接受不可变 `ValidatedRoute` 和明确的 RouterDeployment，删除 direct-provider fallback、可选 minimum、重复 taker 和 require_unified；编码时保留随时间变化的 deadline 重查。Kyber、1inch 也统一经过 normalize_route，同时保留原生 target/router/from/value 和 state override 约束。
- minimum 用商/余数分解消除 U256 中间乘法溢出；先以 2^255 和越界 bps 测试复现旧实现输出 0/panic，再修复并用 U512 作测试 oracle。LiquidSwap 用字符串定位/补零消除 10^decimals 溢出，测试 decimals 0/4/6/78/255。

确定性性能证据（`tests/optimization.rs`，不是生产延迟 benchmark）：

| 场景 | 优化前 | 优化后 |
| --- | --- | --- |
| 已配置或缓存 mapping base，N 条有效 ERC20 route 的逻辑 RPC | 3 + 5N | 6 + 2N |
| 上述 N=5 | 28 次 | 16 次 |
| 一份本地 fixture 的 eth_simulateV1 JSON 请求体，恢复重复 data 字段作对照 | 4,202 bytes | 2,650 bytes |

RPC 计数由实际本地 HTTP JSON-RPC 请求记录断言：1 次 chainId、1 次 gasPrice、1 次 code、1 次 allowance、7 次 block 查询（context、共享 pre-check、5 次独立 post-check）、5 次 simulation。冷 mapping 探测额外增加至多 2 次 access-list 请求。fixture 还覆盖 native、零/不足/充足 allowance 的 approve/reset 序列、每 route 独立 calldata/state overrides、共享准备失败与下一轮重试、模拟后 reorg、Serde 错误隔离和大整数边界。减少请求/字节不等于已经测得生产 p95 或吞吐提升。

最终验证：

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --all-targets --all-features --offline -- -D warnings` | 通过 |
| `cargo test --all --offline` | 97 个通过，15 个默认 ignored；含 7 个 optimization、20 个 provider、8 个 competition 测试 |
| `cargo build --release --offline` | 通过 |
| `forge fmt --root contracts --check` | 通过 |
| `forge build --root contracts --deny-warnings` | 通过；nightly/旧 flag 提示为工具提示 |
| `forge test --root contracts` | 50/50 通过 |
| `cargo test --test e2e_local --offline -- --ignored --nocapture` | 1/1 通过，无路由登记步骤；同一返回 approval/swap 在隔离 Anvil 执行成功 |
| `git diff --check` / `git diff --cached --check` | 通过 |

相对本轮开始的暂存版本，生产 Rust 源码为 4,295 → 4,334 行（净增 39 行，包含注释/空行）；增加主要来自共享准备边界、不可变 route 类型与严格错误分类。精简收益来自删除旧策略/分支、减少重复校验与序列化，以及上述可计数 RPC/请求体成本，不声称总源码行数减少。

本轮没有新增依赖，没有读取或修改开发者配置，没有访问生产 provider/RPC、部署或广播真实网络交易；本地 Anvil 使用 mock Holder。原有 staged 修改保留，新增工作没有 stage/commit。正式 Holder fork、生产 RPC 对单 input 字段和 eth_simulateV1 的兼容性、实际延迟/吞吐及主网执行仍需独立验收。

## JSON 配置、Router bootstrap 与 API 校验（2026-09-15）

- 应用改为读取当前目录 `config.json` 或单个 CLI 路径参数。移除 dotenvy 和应用环境变量读取；保留诊断及 ignored live 测试开关。模板改为 `config.example.json`，Git/Docker 忽略真实配置，未读取、修改或删除已有 `.env`。
- Serde typed 配置使用 camelCase、未知字段拒绝、默认值与 try_from 校验；API 使用 NonZeroU64、Address 和正整数金额/滑点反序列化校验。结构/范围/跨字段错误沿用稳定 API envelope，内部原因不进入响应。
- 配置每链 RPC/Router；启动时验证 chain ID，在同一区块高度验证 Router code、`allowanceHolder()` 与 Holder code。空/短/脏 ABI 返回、revert、保留/自身/无代码地址、错误链、缺 RPC、无支持链和超时均测试失败路径。正式 app 测试确认 bootstrap 完成后 capabilities 才显示已配置 Router，响应不含 RPC URL。
- Holder 不再硬编码；approval 与 outer exec 使用动态部署。Rust execute ABI 同步当前 receiver/参数顺序，API taker 映射为 sender/receiver。Solidity 源码和 Foundry 测试未改动。
- 本地 Anvil 部署独立 mock Holder 和当前无管理员 Router（单参数 constructor），从 getter 发现 Holder，完成 HTTP → 固定块仿真 → 返回 approval → 实际本地 swap。断言 approval spender 与 transaction.to 均等于发现的 Holder、到账 200 且 Router allowance 清零。
- Rust 独立 provider rules 策略未变；生产默认规则仍为空，配置 Router 不会绕过 `ROUTE_NOT_ALLOWLISTED`。是否移除该策略待用户单独确认。

验证命令（离线依赖缓存，RPC 仅限本地 fixture/Anvil）：

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --all-targets --all-features --offline -- -D warnings` | 通过 |
| `cargo test --all --offline` | 87 个测试通过，15 个默认 ignored（本地 E2E 另行运行）；包含拒绝地址数组和无 0x 前缀的 API 回归 |
| `cargo build --release --offline` | 通过 |
| `forge fmt --root contracts --check` | 通过 |
| `forge build --root contracts --deny-warnings` | 通过；工具提示 nightly 和旧 flag deprecation，不影响结果 |
| `forge test --root contracts` | 50/50 通过，含 fuzz |
| `cargo test --test e2e_local --offline -- --ignored --nocapture` | 本地 Anvil 1/1 通过 |
| `git diff --check` | 通过 |

未运行生产 provider/RPC、真实 Holder fork、主网签名/广播、部署或 Docker 镜像构建。代码存在/getter 正确不等于实现可信或具备全部 simulation 方法；生产部署与路由策略仍需独立验收。迁移操作见 [CONFIGURATION.md](CONFIGURATION.md)。

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

## 单请求竞赛与直接执行（2026-09-14）

本轮将原 create/poll/build 改为一次 POST 请求返回最终结果。真实 taker 必填，provider 完整 quote → 构建 → simulation 使用同一总 deadline；成功结果按模拟余额增量排序，并直接返回同轮 approvals 和 swap。移除存储、access token、独立 build、API expiresAt 和 route 元数据人工 TTL；上游签名及 calldata 自有期限继续生效。

本轮验证（生产网络测试未运行）：

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --all-targets --all-features --offline -- -D warnings` | 通过 |
| `RUST_LIB_BACKTRACE=1 cargo test --all --offline` | **61/61**；15 项默认 ignored |
| `cargo build --release --offline` | 通过 |
| `forge fmt --root contracts --check` | 通过 |
| `forge build --root contracts --deny-warnings` | 通过，复用合约构建缓存 |
| `forge test --root contracts` | **27/27**，含 3 项 fuzz |
| `cargo test --test e2e_local --offline -- --ignored --nocapture` | **1/1** |

关键行为证据：

- `tests/competitions.rs` 使用 Tokio 虚拟时间验证一个 deadline 覆盖 context/quote/simulation、提前结束、保留已完成结果、取消未完成 future、公共 context 超时与 provider 局部失败。排名覆盖大于 JS 安全整数的余额增量、原报价排名反转、平局稳定排序，以及未知 Gas 费用不妨碍到账数量排序。
- `tests/concurrency.rs` 验证取消请求 future 后立即释放并发容量；`tests/simplification.rs` 验证普通失败后容量释放与仿真失败分类。
- `tests/api.rs` 验证一次 POST 返回 200、真实 taker 必填、区块 number/hash/timestamp 与模拟时间、原样交易字段、旧轮询/build 路由 404 和 no-store。
- `tests/core.rs` 解码返回的 Holder/Router calldata，验证没有上游 deadline 时为 U256::MAX，有上游 deadline 时保留 Unix 秒，minimum/amount/provider calldata 不变。
- 本地 Anvil E2E 只调用一次竞赛，然后执行第一次返回的 approval 和同一笔 swap；最终买入余额为模拟的 200，Router 临时 allowance 清零。测试没有重新 build。

没有新增或升级运行时依赖；删除直接 subtle 依赖和 axum-extra typed-header feature，仅为确定性测试启用已有 Tokio 的 test-util。合约源码没有修改。15 项 ignored 包含 13 个 provider live、1 个 production replay 和另行执行的本地 Anvil。Foundry nightly/参数弃用提示仍存在，命令退出成功。

接口迁移与当前配置详见 PRODUCT/TECHNICAL/README。旧版 preview live 记录仅为历史证据，不能替代本次新接口的真实钱包、正式 Holder/Router 与生产执行验证。

## Quote 成功数据与失败分离（2026-09-15）

`Quote` 改为必填的 `route: Route`、`simulation: SimulationSuccess`、`approvals`、`transaction` 和 `latencyMs`，没有 status/error 或可缺失的成功数据。`SimResult.simulation` 同样只表示成功；仿真失败通过原有 anyhow + SimulationFailure 传播。竞赛返回独立 `quotes` 和 `failures`，只对成功结果按 boughtAmount 排序，全部失败时 quotes 为空。

字段迁移：provider/buyAmount/minBuyAmount 从 route 读取，成功 simulation 不再携带 status；失败的 status/error 和可选失败专用 simulation 仅在 ProviderFailure 中。Route 的 tx 是 provider 路由交易，钱包继续执行 quote 顶层的 approvals/transaction。

本轮验证：

- `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features --offline -- -D warnings`：通过。首次 Clippy 指出迁移后 Copy 类型多余借用，已修正并重跑通过。
- `RUST_LIB_BACKTRACE=1 cargo test --all --offline`：**62/62**；15 项默认 ignored。
- `cargo build --release --offline`：通过。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`、`forge test --root contracts`：通过，**27/27**；合约源码未修改，构建复用缓存。
- `cargo test --test e2e_local --offline -- --ignored --nocapture`：**1/1**。单请求返回的 approval 与同一笔 swap 直接在隔离 Anvil 执行，到账仍为 200。
- `git diff --check`：通过。原有 staged diff 的 SHA-256 在本轮前后相同，本轮修改未 stage/commit。

API 测试新增全失败时 quotes=[] 与独立 failures 的响应断言；混合成功/超时/上游错误测试检查成功结果与失败诊断不交叉携带字段。原有金额排序、统一 deadline、取消释放名额、真实资金、minimum、reorg、原始错误原因隔离测试继续通过。未新增依赖、未访问生产 provider/RPC、未部署合约。

## BalanceSlots 配置、探测与 mapping base 缓存（2026-09-15）

新增独立 `src/balance_slots.rs`，优先使用 `BALANCE_SLOTS` 配置，无配置时通过 prestateTracer 的实际 storage 访问与本地 base hash 匹配自动探测。缓存只保存 `(chainId, token) -> mappingBase: U256`，与 owner 无关；每次使用根据 owner 计算 storage key 并重新验证，旧 base 不匹配时失效重探测。配置错误不静默回退，失败不缓存为成功。

本轮证据：

- `tests/balance_slots.rs` **11/11**：完整 uint256 配置与非法输入、配置优先且错误不回退、跨链/token 隔离、不同 owner 复用 base、13 个 owner 并发仅一次探测、失效重探测、常量/packed/共享 scalar/歧义拒绝、RPC 错误分类与重试、候选上限、自动范围边界及配置补充、取消释放锁、1024 项 FIFO 淘汰，以及真实资金模拟不触发探测。
- `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features --offline -- -D warnings`：通过。
- `cargo test --all --offline`：**73/73**；15 项默认 ignored（13 provider live、1 production replay、1 本地 Anvil）。
- `cargo build --release --offline`：通过。
- `cargo test --test balance_slots --test e2e_local --offline -- --include-ignored --nocapture`：本地 Anvil **1/1**，两个未持有卖出 token 的地址依次通过自动发现/复用 mapping base、覆盖余额与完整 approval/swap 仿真；公开竞赛返回的同一笔 approval/swap 继续执行成功。隔离 Anvil 上的模拟覆盖没有写入真实余额。这不是主网或正式 Holder 证明。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`、`forge test --root contracts`：通过，合约 **27/27**，未修改合约源码。
- `git diff --check`、`git diff --cached --check`：通过。原有暂存区 diff 的 SHA-256 保持不变，未 stage/commit。

自动探测只匹配 mapping base `0..1023`，没有逐槽 RPC 扫描；任意 uint256/namespaced base 可通过配置提供。生产 RPC 的 debug namespace、state override 支持，以及特殊 token/proxy/rebase/外部记账布局仍需单独验证。公开竞赛保持实际资金语义。本轮未新增依赖、未请求生产 provider/RPC、未部署合约。

## MetaRouter 无白名单、资产 recover 与管理员转移（2026-09-15，历史记录）

本节记录当时的实现与验证；其中移除白名单的部分已按用户后续要求回退，当前状态见后文“恢复路由白名单”。recover 与两步 ownership 保留。

按用户明确要求移除链上 `allowed/setAllowed` 和 Rust `Rule/Provider::rules()`、`ROUTE_NOT_ALLOWLISTED`。交易无需登记路由；继续保留实际到账 minimum、精确临时授权、历史余额隔离、pause、重入和 Holder 调用形状检查。新增 owner-only ERC20/native `recoverToken`，与 swap 共用锁，支持暂停期间提取；owner 转移采用提名/接受两步，带事件且旧 owner 在接受后失权。

新增回归先复现了无白名单直接调用第三种暂存 ERC20 的风险：以该 token 的 `transfer` 为目标，在 native 退款回调中补足输出，原先预期拒绝的测试实际成功。补入 target 的 `balanceOf(router)` 检查后，transfer 和 approve 两条路径均被拒绝，测试转绿。该检查不是地址登记，也不是任意恶意 token 的完整证明；带相同余额接口的 vault/入口会被拒绝。

本轮新鲜验证：

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --all-targets --all-features -- -D warnings` | 通过 |
| `cargo test --all` | **73/73**；15 项网络/Anvil 测试默认 ignored |
| `cargo build --release` | 通过 |
| `forge fmt --root contracts --check` | 通过 |
| `forge build --root contracts --deny-warnings` | 通过 |
| `forge test --root contracts` | **42/42**；5 项 fuzz 各 256 runs |
| `cargo test --test e2e_local -- --ignored --nocapture` | **1/1**；无 setAllowed/route rules 的本地完整执行 |
| `git diff --check` / `git diff --cached --check` | 通过 |

行为覆盖包括：新 target/spender/selector 免登记、非法/直接 ERC20 target 拒绝、保留原有资金不变量、ERC20/native 部分提取与余额守恒、无返回值 token、false/revert/余额不足/拒收失败、非 owner 拒绝、pending owner 替换和接受、旧 owner 失权、swap → recover 与 recover → swap/recover 的交叉重入。Rust 检查移除白名单后的有效路由、零地址拒绝及 Holder/Router ABI 原样编码；Anvil 无登记步骤即可完成模拟和成交。

Gas 行为变化包含删除白名单 storage 查询、新增 target/spender 代码检查与 target 的 balanceOf 探测；实际增减依 provider 实现和调用路径而定，不把测试 helper 的 gas 作为主网成本。`execute` ABI 和构造参数顺序不变，旧白名单 ABI 移除；owner 不再 immutable，新部署采用可转移管理员。

更新了 contracts/README、产品/技术/架构/接入说明、根 README、AGENTS 和相关项目 skill。历史验收条目保留原记录。没有新增依赖、没有提交代码或访问生产 provider/RPC；本地测试使用 mock Holder。生产 Router 地址接入、正式 Holder/provider fork 兼容性和外部审计尚未完成。管理员可提取 Router 的暂存资产，这一权限变化已在文档明确说明。

## BalanceSlots 只读布局解析与 simulation 职责分离（2026-09-15）

`BalanceSlots::resolve(rpc, chainId, token, block) -> U256` 只解析 mapping base。配置和缓存命中直接返回，不接收真实 owner/amount，不构造或应用 StateOverride。无缓存时，两个由固定标签派生的假地址分别 trace balanceOf，在同一 block 上交叉匹配唯一 mapping base。发现流程可以独立于 simulation 运行；缓存仍按 chain/token 保存 base。

simulation 消费 base 后自行计算用户 storage key、构造和验证余额覆盖。配置错误不会被自动覆盖；缓存 base 验证失败时当次明确失败，并按旧值条件失效，下次请求重新解析。晚到的旧失败不能清除不同的新 base。只读 trace 识别布局，不证明该 word 可直接作为完整余额改写；packed/constant/shared-scalar 拒绝由 simulation 测试覆盖。

本轮验证：

- `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features --offline -- -D warnings`：通过。
- `cargo test --all --offline`：**77/77**；15 项默认 ignored。`tests/balance_slots.rs` **15/15** 覆盖独立探测、两个假地址、全程无 override、配置/缓存无 RPC、链/token 隔离、并发合并、条件失效、错误分类/重试、取消、容量淘汰、检测范围，以及 simulation 独立验证和不同真实 owner 消费预解析 base。
- `cargo build --release --offline`：通过。
- `cargo test --test e2e_local --offline -- --ignored --nocapture`：**1/1**。先在隔离 Anvil 上独立 resolve，再由两个未持币测试地址消费同一个 base 完成完整仿真；原有公开竞赛 → 返回 approval/swap → 本地执行继续通过。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`、`forge test --root contracts`：通过，当前合约 **42/42**；本轮未修改合约和管理员/路由逻辑。
- `git diff --check`、`git diff --cached --check`：通过，原有暂存区保持不变。

自动识别仍限定 base `0..1023` 与 token 自身 storage；完整 uint256 base 可配置。没有新增后台探测或公开 API，没有改变真实资金竞赛的资金规则。未增加依赖，未使用生产 provider/RPC，未部署或提交代码。

## 双 trace 并发与缓存结构简化（2026-09-15）

`BalanceSlots::resolve` 用 `tokio::try_join!` 并发运行两个只读 `trace_bases`；任一错误结束本次探测并丢弃另一侧 future，成功时仍取两个假地址候选的交集。使用本地 RPC barrier 测试要求两个请求到齐才响应，先在旧串行实现复现超时，再验证并发实现通过。

缓存改为 `Mutex<HashMap<(u64, Address), SharedBase>>`，`SharedBase` 为 `Arc<tokio::sync::Mutex<Option<U256>>>`，删除 CacheKey/CacheEntry 和 VecDeque。外层锁只保护索引，内层锁继续合并同一 key 的探测；1024 项容量、正在使用项不淘汰、条件失效和取消后重试保持。淘汰策略改为任意空闲项，不再保证 FIFO；对应测试检查容量淘汰及新插入项命中，不绑定被淘汰的具体 key。并发失败可能已有一到两个请求送达 RPC，测试保留错误分类与重试检查，不再假定只有一个请求发出。

本轮验证：`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features --offline -- -D warnings`、`cargo test --all --offline`（**78/78**，15 项默认 ignored）、`cargo build --release --offline` 全部通过。`tests/balance_slots.rs` 为 **16/16**。`forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`、`forge test --root contracts` 通过（**42/42**）；`cargo test --test e2e_local --offline -- --ignored --nocapture` **1/1** 通过。`git diff --check` 与 `git diff --cached --check` 通过，原有暂存区 SHA-256 不变。

未新增依赖、未修改公开接口或 simulation 资金边界、未调用生产 provider/RPC、未部署或提交代码。

## 恢复路由白名单，保留 recover/ownership（2026-09-15）

按用户要求仅回退移除路由白名单的部分：恢复链上 `allowed`、owner-only `setAllowed`、`RoutePermission`、`RouteNotAllowed`，以及 Rust `Rule`、`Provider::rules(chainId)`、竞赛/仿真/交易编码的三元组预检和 `ROUTE_NOT_ALLOWLISTED`（HTTP 422）。默认拒绝，规则不能从当次上游响应自动生成。`execute` ABI 不变。

保留 ERC20/native `recoverToken`、两步管理员转移、共用重入锁，以及直接 ERC20 target、Router 自调用和 Holder 形状检查。未增加前置最低到账或强制消耗全部输入的新规则。BalanceSlots、已有 Rust 重构和其他未提交变更保留；未 stage/commit。

本轮验证（以下通过结果对应并行仿真接口变更出现之前的白名单恢复快照，不代表最终混合工作区通过）：

- Rust `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features --offline -- -D warnings` 通过。首次 Clippy 指出 Anvil fixture 的 selector 多余借用，修正后重跑通过，没有放宽 lint。
- `cargo test --all --offline`：**79/79**；15 项默认 ignored（13 provider live、1 production replay、1 另行运行的本地 Anvil）。新增 target/spender/selector 不匹配及空规则拒绝测试，同时检查直接交易编码不可绕过预检；恢复竞赛层未登记 provider 的失败断言。
- Foundry fmt/build/test 通过，`forge test --root contracts`：**44/44**，含 5 项 fuzz、各 256 runs。覆盖新 target 默认拒绝、登记后成功、撤销后拒绝、selector/spender 不匹配、非法登记、旧/待接任/新 owner 的白名单权限和全部 recover 回归。
- `cargo test --test e2e_local --offline -- --ignored --nocapture`：**1/1**，本地 Anvil 显式登记测试路由并提供对应 rules，完整仿真与返回的同一笔 approval/swap 执行成功；保留独立 BalanceSlots resolve 和两个 preview owner 的覆盖测试。

最终复核发现并行工作区变更移除了 `SimulationRequest.actual`，并改变了 simulation 的资金覆盖语义。此时再次运行本地 E2E 编译失败：`tests/e2e_local.rs` 仍传 `actual: false`，产生 E0560；其他尚未迁移的测试也仍引用该字段。最终 Rust fmt 检查同时报告 `src/simulation.rs` 新改动的格式差异。此前 release build 已退出成功，但当前混合工作区不能据此标记全量门禁通过。白名单回退没有覆盖或接管这组无关的并行修改，需由该接口迁移完成后重新执行全量验证。

白名单恢复增加一次路由权限 storage 查询；未做主网 gas benchmark。登记 `(Holder, Holder, exec)` 仍只约束外层形状，不验证内层 target/operator，不能作为内层 provider 身份审核证明。生产 Router 地址仍未接入，各生产 provider 的 rules 默认为空，经审核的链专属规则、链上登记、正式 Holder fork 测试和外部审计仍是上线前置条件。本轮未访问生产 provider/RPC、未部署或广播真实网络交易。

## Simulation 统一使用资金覆盖（2026-09-15）

移除 `SimulationRequest.actual` 及其两套资金分支。所有 simulation 都不读取 taker 的真实 native 或卖出 token 余额：ERC20 通过 `BalanceSlots` 解析 mapping base 后，直接把 `storage_key(taker, base)` 写为 `sellAmount`；native balance 直接覆盖为 transaction value 与完整调用序列声明 gas budget 之和。allowance 仍按固定区块读取，以决定是否加入 reset/approve。

删除 `validate_balance_override`、反值 probe、另一地址不变检查和 `BalanceSlots::invalidate`。配置或缓存的 base 不再因一次 simulation 失败而清除；错误布局由后续完整 approval/swap 仿真失败暴露。成功结果的 `funding` 固定为 `overridden`，公共 competition 不再拒绝余额不足的钱包。`boughtAmount` 仍来自完整调用序列的买入 token 余额差，并继续执行 route minimum、固定区块 hash、approval 返回值和调用状态检查。成功只证明交易在假定资金与该 block context 中通过仿真，不证明钱包当前可发送；客户端需自行检查余额和决定是否重新 simulate。

本轮验证：

- `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features --offline -- -D warnings`：通过。
- `cargo test --all --offline`：**75/75**；15 项 live/Anvil 测试默认 ignored。RPC fixture 明确检查 native/ERC20 state override、无 `eth_getBalance`、固定区块、slot 探测失败和 approval false 分类；competition/API 检查 `funding: overridden`、模拟到账整数排序和 minimum 失败隔离。
- `cargo build --release --offline`：通过。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`、`forge test --root contracts`：通过，**45/45**；本轮未修改合约资金或路由逻辑。
- `cargo test --test e2e_local --offline -- --ignored --nocapture`：**1/1**。两个无卖出 token 的地址复用自动探测 base 完成覆盖仿真，公开 HTTP competition 返回 `funding: overridden`，有真实资金的本地账户执行同一 approval/swap 成功；state override 未改变链上余额。

本轮没有调用生产 provider/RPC，没有签名、部署或广播真实网络交易。生产 RPC 仍需逐链验证 `eth_simulateV1` state override 与按需 `debug_traceCall`；错误或不兼容的配置 base 可能让完整仿真失败，当前不会自动纠正配置或淘汰缓存。

## BalanceSlots 改用 eth_createAccessList（2026-09-15）

自动 mapping base 探测已从 `debug_traceCall/prestateTracer` 改为 Alloy typed `Provider::create_access_list(...).block_id(block)`。两个固定假 owner 的 `balanceOf` access list 仍通过 `tokio::try_join!` 并行生成；resolver 只读取 token 地址对应的 `storageKeys`，在本地匹配 base `0..1023` 后取交集并按 chain/token 缓存。全过程不使用 state override、真实 owner 或真实余额，也不需要 Alloy `debug-api` feature 或手写 `raw_request` JSON。

RPC transport/method 错误继续通过 `map_rpc_error("eth_createAccessList", ...)` 分类；HTTP 200 的 `AccessListResult.error` 保留原始内部原因并分类为 `RPC_CALL_FAILED`。token storage key 超过 32、无候选或多候选继续明确失败。配置命中不调用 RPC，缓存并发合并、1024 项容量、取消后重试和 simulation 直接覆盖资金的边界不变。

本轮验证：

- `cargo test --test balance_slots --offline`：**12/12**，覆盖 typed access-list 请求、固定 block、两个假 owner 并行、无 override、配置/缓存、错误字段与错误码、畸形响应、候选上限和取消/容量行为。
- `cargo test --test simplification --offline`：**5/5**，未配置 base 且 RPC 不支持 `eth_createAccessList` 时仍公开为稳定的 `RPC_METHOD_UNSUPPORTED`。
- `cargo test --test e2e_local --offline -- --ignored --nocapture`：**1/1**，本地 Anvil 的 access-list 自动探测、两个无卖出 token 地址的覆盖仿真，以及公开竞赛返回交易的本地执行全部通过。
- `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features --offline -- -D warnings`：通过。
- `cargo test --all --offline`：**75/75**；15 项 live/Anvil 测试默认 ignored。`cargo build --release --offline`：通过。
- Foundry fmt/build/test：通过，**45/45**；本轮没有修改合约。

尚未把生产 RPC 支持视为已验证：托管节点可能关闭或未实现 `eth_createAccessList`，部署前需逐链测试。配置错误或非标准余额布局仍可能让完整仿真失败，不会自动纠正配置或淘汰已缓存 base。

## Routes 先于统一 Block Context（2026-09-15）

竞赛调度改为批次两阶段：先并发获取并校验当前链全部 provider routes；所有 route future 成功、失败或超时结算后，读取一次公共 parent block context；随后基于该 context 并发 simulation 所有有效 routes。这样不会再用 provider 请求之前取得的 context 模拟后来生成的 route。route 错误仍只进入对应 `ProviderFailure`，公共 context 错误会使已取得的 routes 分别失败，Quote 仍只包含 route 与 simulation 均成功的数据。

三阶段继续共用同一个 `COMPETITION_TIMEOUT_MS` 绝对 deadline，不增加阶段配置或隐式续期。由此带来的明确边界是：若某个 route 请求一直运行到总 deadline，其他已取得的 routes 也没有剩余预算完成公共 context 和 simulation。本轮测试固定了该行为，避免把总预算悄悄变成每阶段预算。

本轮验证：

- `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features --offline -- -D warnings`：通过。
- `RUST_LIB_BACKTRACE=1 cargo test --all --offline`：**76/76**；15 项 live/Anvil 测试默认 ignored。`tests/competitions.rs` 为 **7/7**，在 context provider 内断言全部 route future 已结算，检查整轮只读取一次 context、所有有效 route 共享它、context 超时不启动 simulation、simulation 阶段保留 deadline 前完成的结果，以及 route 耗尽总预算时不产生 Quote。
- `cargo build --release --offline`：通过。
- `forge build --root contracts --deny-warnings`、`forge test --root contracts`：通过，当前并行合约工作区 **33/33**；本轮未修改合约。
- `forge fmt --root contracts --check`：未通过；当前未提交的 `contracts/test/MetaRouter.t.sol` 存在既有格式差异，本轮没有改写该并行文件。
- `cargo test --test e2e_local --offline -- --ignored --nocapture`：未通过，报 `preview discovery: ROUTER_NOT_DEPLOYED`。当前 `MetaRouter` 构造函数为一个 `allowanceHolder` 参数且无 `setAllowed`，而并行的 E2E fixture 仍按两个构造参数部署并调用 `setAllowed`；失败发生在进入 HTTP competition 前，不是本轮 route/context 调度路径的回归证据。

本轮没有调用生产 provider/RPC，没有签名、部署或广播真实网络交易。

## Provider 独立 Route + Latest Simulation（2026-09-15）

竞赛从“批量 routes → 公共 context/prepare → 批量 simulation”改为每个 provider 独立的 `route → validate → simulate` pipeline。某家 route 完成后立即在 `latest` 上开始 simulation，不等待其他 provider；所有 pipeline 仍共享竞赛创建时的同一个绝对 deadline。测试证明快 provider 会在慢 provider 的 route 尚未完成时进入 simulation，慢 route 或慢 simulation 的失败只影响自身。

删除请求期 `Context`、`ContextProvider`、`ContextSource`、`SimulationPreparation` 以及固定区块前后 hash probe。Simulator 不再调用 `eth_chainId`、`eth_getBlockByNumber` 或 `eth_gasPrice` 获取 context，也不在 `eth_simulateV1` calls 中设置 `gasPrice` 或 block override。`blockContext` 直接从返回的 `SimulatedBlock` 生成；`number` 直接保留为 u64，不再格式化为十六进制字符串。不同 provider 可能观察到不同 latest 区块。`gasUsed` 仍统计 approval/swap，`gasFeeWei` 固定为 null。

为避免未知 latest fee 触发节点按声明 gas limit 做 upfront balance 检查，taker 的 native state override 设为 `U256::MAX`；这仍只是 `funding: overridden` 的模拟假设，不表示真实钱包资金。ERC20 卖出余额 override、固定一笔 `approve(Holder, sellAmount)`、完整调用状态、approval 返回值、余额差和 minimum 检查保持不变。配置 mapping base 的 5-provider fixture 从本轮改造前的 14 次逻辑 RPC 降到 5 次 `eth_simulateV1`，并明确断言请求标签为 `latest`、没有 context RPC 和 call `gasPrice`。

本轮验证：

- `sh scripts/check.sh`：通过。
- `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`：通过。
- `cargo test --all`：**94/94**；15 项生产 provider/replay 与单独 Anvil 测试默认 ignored。
- `cargo build --release`：通过。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`、`forge test --root contracts`：通过，**50/50**；本轮没有修改合约。
- `cargo test --test e2e_local -- --ignored --nocapture`：**1/1**，隔离 Anvil 上完成自动 balance-slot 探测、latest simulation、HTTP competition，以及返回 approval/swap 的本地执行。

本轮没有调用生产 provider/RPC，没有签名、部署或广播真实网络交易。生产节点对 `eth_simulateV1(latest)`、省略 gas price、state override 和返回区块 header 的兼容性仍需逐链验收。

## 未验证边界

本节之前的 live 记录是历史运行快照，不因后续结构重构自动成为新的 live 验证。

1. Provider live smoke 已落库，但真实报价、费用、地址和执行可用性仍取决于运行时提供的 key、链专用 token、上游限流和生产 simulate RPC；live smoke 不广播交易，也不等价于主网 fork 或正式执行验证。
2. 本地 E2E 的 Holder 是测试 mock 写入固定地址，不是从主网读取的正式字节码；正式 Holder 与嵌套路由必须补 fork 验证。
3. Router 未部署、未外部审计；服务不签署、不广播真实网络交易。本次唯一广播发生在测试进程创建的隔离本地 Anvil，测试完成即关闭。
4. 真实 provider endpoint 的访问路径已有可显式运行的 smoke test，但每个 key 的权限、实时流动性、生产 RPC、正式 Router 和主网执行仍需逐环境验证。Router 已可在 JSON 中配置，Holder 在 bootstrap 读取；Rust 与 Solidity 均 permissionless，不再要求 rules 或链上登记。不能把 fixture、代码/getter 检查或 reachability 通过当作 live E2E。
5. 跨链、非 EVM、原生币 buy、intent、平台抽成和公网身份系统不属于当前最小核心。
6. Redis/Postgres、多副本、分布式限流、业务审计库、供应商熔断和 RPC 容灾未实现；当前是有界单进程版本。
7. Dockerfile 已提供，但 Docker daemon 不可用，镜像 build/run 未验证。
8. 仿真、费用估算和报价都不是未来成交保证；当前单请求流程由用户依序执行返回的 approvals/swap，并根据 blockContext 决定是否重新 simulate。没有 expiresAt 或强制重新 build；最终仍由链上 minimum output 与 provider 原生约束保护。

## 文件位置与交付

本次修改直接发生在 `/Users/caojiafeng/Documents/ChatGPT/metamatch`，没有创建 commit、push、部署或广播交易。生成的 `target/`、Foundry 构建物和本地环境文件不纳入交付。详细处理过程、删除范围和判断依据见 [RUST_REWRITE_LOG.md](RUST_REWRITE_LOG.md)。

## 上线前顺序

申请并核对供应商服务权限 → 配置专用 simulate RPC → 正式 Holder/Router/provider fork 测试 → 外部合约审计 → Router 部署与 JSON 地址接入/Holder bootstrap → 完整监控/访问控制 → 小额人工验收。当前 Router 无管理员，Rust/合约无路由白名单；仍需核对嵌套 Holder 与真实 provider 路径的执行语义，不能靠配置成功代替执行验收。其他链应补齐对应费用模型、RPC 兼容性和 route 测试。
