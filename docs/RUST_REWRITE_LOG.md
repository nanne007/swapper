# Rust 重写过程记录

本文件只记录本次迁移的可审阅事实、决策和验证结果，不记录密钥、RPC 凭据、钱包材料或原始供应商响应。

## 2026-09-11：基线与范围

- 工作目录：`/Users/caojiafeng/Documents/ChatGPT/metamatch`
- 分支：`main`
- Git 状态：仓库尚无 commit；文件显示为未跟踪初始化内容，没有可识别的已跟踪用户改动。
- 原运行时：Node.js 22 + TypeScript 5.9 + Fastify 5 + Zod 4 + viem 2。
- 保留边界：Solidity/Foundry 合约不改写；不部署、不广播、不访问生产 RPC；真实供应商 live end-to-end 不在本次可验证范围。
- 工具链事实：Rust 1.89.0、Cargo 1.89.0、rustfmt 1.8.0、Foundry 1.6.0-nightly 可用。

## 2026-09-11：迁移策略

- Rust crate 放在仓库根目录，设为 canonical runtime，避免与既有 `src/*.ts` 发生目录级破坏。
- 旧 TypeScript 暂时保留为逐行为比对和回滚材料；启动、Docker、CI 和文档会切换到 Rust。
- 使用 `U256` 做内部整数运算，公开 JSON 保持十进制字符串；不引入浮点金额计算。
- 采用 trait 注入 HTTP、Provider、Context 和 Simulator，方便 fixture 测试并隔离网络副作用。

## 2026-09-11：Rust 实现阶段完成

- 新增根 Cargo crate 及 `src/{domain,config,http,execution,providers,rpc,simulation,competitions,openapi,app,main}.rs`；Axum/Tokio 取代 Fastify 作为默认 HTTP runtime。
- 以 serde `deny_unknown_fields`、显式 `Fault`、`U256`、`alloy-sol-types::sol!` 和 trait 注入保留输入、金额、ABI、网络边界；没有把 TS 的 `bigint` 替换成 Rust 浮点类型。
- 迁移 0x、1inch Classic、KyberSwap fixture 适配器；Provider/RPC 错误不再作为成功报价或成功仿真返回。`eth_simulateV1` 不支持时明确映射到 `unsupported`。
- 迁移 demo/live 竞赛、TTL/capacity、token 鉴权、SSE replay/heartbeat、OpenAPI、重新报价 build 和 Base fee 未实现时的拒绝路径。
- 用 RAII `StreamPermit` 迁移 SSE 并发上限：同一竞赛最多 5 条流，流结束或客户端断开时释放；新增测试覆盖拒绝第 6 条和释放后重连。
- Rust 版本的 HTTP 测试通过内存 Axum Router 验证 demo 生命周期、鉴权、SSE replay 和 demo build 禁止；不把 fixture 误称为 live 或 E2E。

## 2026-09-11：质量门与文档切换

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过。
- `cargo test --all -- --nocapture`：18/18 通过；包括仿真重组、RPC method unsupported、provider failure isolation 和 SSE 并发上限。
- `cargo build --release`：通过。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`：通过。
- `forge test --root contracts`：27/27 通过，3 项 fuzz 各运行 256 次。
- README、AGENTS、TECHNICAL、VERIFICATION、Dockerfile、CI 已改为 Rust 默认入口；`target/` 加入忽略列表。没有 commit、push、部署、钱包签名或广播。
- 运行时烟测：`cargo run` 首次因已有 Node 进程占用 3000 退出；未终止该非本次进程，改用 `PORT=3100 cargo run`，随后 `curl /health`、`/v1/capabilities` 和 demo `POST /v1/competitions` 均成功，最后正常停止 Rust 进程。
- `cargo test --test e2e_local -- --ignored --nocapture`：1/1 通过；本地 Anvil 中验证 Rust HTTP → `eth_simulateV1` → approval → rebuild → swap，买币余额为 200，Router 对 Provider 的授权为 0。

## 2026-09-11：最终复核

- 最终顺序复核：`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --all -- --nocapture`（18/18）、`cargo build --release`，全部退出状态 0。
- Cargo lint 配置新增 `unsafe_code = "forbid"`；加入后 Clippy、18 个 Rust 测试和 release build 仍全部通过。
- 最终合约复核：`forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`、`forge test --root contracts`（27/27，3 项 fuzz 各 256 次），全部退出状态 0；Foundry nightly/deprecation warning 仍仅为工具提示。
- 最终本地链复核：`cargo test --test e2e_local -- --ignored --nocapture`（1/1），退出状态 0；Docker daemon 不可用，Docker image build 未执行。

## 2026-09-11：未完成项

- 没有真实供应商 API key、生产 RPC、正式 Router/Holder 部署或主网 fork，因此 live provider、真实费用、地址和执行可用性仍未验证。
- 当前 Rust runtime 没有持久化、多副本事件总线、IP/分布式限流、用户身份系统或业务审计库；这些属于后续生产化范围，不在本次重写中暗加。

## 2026-09-11：Rust 生态复用 review

- 本轮仅做只读 review，除追加本条日志外未修改 Rust 实现、未新增依赖、未改变 API 或行为。统计到 `src/*.rs` 共 3451 行，`tests/e2e_local.rs` 635 行，最大手写区域是 `competitions.rs`、`simulation.rs`、`domain.rs`、`providers.rs`。
- 最高收益替代点是 `rpc.rs`、`simulation.rs` 和本地 E2E 的手写 JSON-RPC。候选是 Alloy Provider、`alloy-rpc-types` 的 typed `eth_simulateV1`/state types，以及 E2E 中的 Alloy contract/provider helpers；但必须保留 reorg、route allowlist、balance slot、错误码和响应体上限等业务/安全边界。
- 当前工具链声明 Rust 1.89；本机 `cargo info` 显示 Alloy Provider 1.3.0 的 MSRV 为 1.88，而 1.7.3 已要求 1.91。因此若不升级工具链，后续实验必须精确 pin 到兼容版本，不能使用宽泛的 `1.3` 版本范围。
- 不建议为减少代码引入通用 config/validator/cache、DashMap、另一套 RPC 客户端或 Tower HTTP middleware：现有配置校验、金额/地址校验、异步状态锁、Axum body limit、SSE replay/terminal 语义都包含项目特有契约，替换后通常不会更短。
- 低风险的本地简化候选：删除 `ContextService`/`SimService` 两层同签名 adapter，清理未使用 helper，移除重复的 dev `serde_json` 声明，并按实际使用收窄 Tokio features。这些不是功能迁移，须单独通过现有 Cargo/Foundry/E2E 质量门。
- 推荐顺序：先做 adapter/dead-code 清理；再用 Alloy 1.3.0 做隔离的 E2E provider spike 并比较代码行数、依赖树和测试可替换性；验证通过后再迁移 runtime RPC/simulation；最后再评估 provider response typed DTO。没有通过“代码更少且契约不变”门槛的库不引入。
- 本轮执行了 `rg` 依赖/调用图盘点、`wc -l src/*.rs tests/*.rs`、`cargo tree -i` 依赖路径检查和 `cargo info` 版本/MSRV 检查；未因 review 重跑测试，最近一次质量门结果保留在本文件上方记录中。

## 后续追加格式

每个阶段追加：

1. 变更文件和可观察行为；
2. 关键设计取舍及其原因；
3. 执行的命令、退出状态和结果；
4. 失败的根因与修复，或明确标记为环境限制；
5. 尚未验证的边界。

## 2026-09-12：Slice 5 至 Slice 10 完成记录

### Slice 7：metadata-dependent adapters

- 新增 LiquidSwap route adapter。原生币只使用协议约定的 18 decimals；ERC-20 decimals 通过对应链配置的 Alloy `eth_call` 读取，并将 raw `U256` 金额无损转换为上游要求的十进制字符串；RPC 未配置时明确失败，不猜 token metadata。
- 新增 Odos quote v2 -> assemble 两阶段 adapter，使用供应商原生请求/响应 DTO、x-api-key 和 path id；只转发 token 地址和整数金额，不引入 token registry 或浮点金额。
- 两家均增加 fixture，覆盖 metadata、两阶段请求顺序、输出金额和交易数据校验。

### Slice 8：链专用与签名 API

- 新增 OogaBooga 的 HyperEVM/Berachain host 选择、native zero address、Bearer key、状态和动态 router 校验。
- 新增 OKX DEX swap adapter：完整校验 API key、secret key、passphrase、project id 四项配置；按官方规则对 timestamp + method + path/query 生成 HMAC-SHA256/Base64 headers，并校验链、token、金额、交易目标和值。
- 新增 `base64`、`chrono`、`hmac`、`sha2` 直接依赖；没有在源码、fixture 或日志中写入真实 secret。

### Slice 9：核心精简与行为测试

- 删除 CoinGecko、固定 18 decimals 和 `net_output` 比较路径；quote 排序改为同一 swap 请求下，直接比较已仿真的整数 `quotedAmount`。因此不再为跨链 token 引入外部价格源或 token catalog。
- 保留核心安全不变量：token 透传但必须通过地址/交易边界验证，route target/spender/value/calldata/expiry 校验，固定 parent block 仿真，provider failure isolation，TTL/capacity，approval 后重新报价和 Router allowlist。
- 当前 13 家 provider 都有实际 HTTP adapter 和脱敏 fixture 契约测试；fixture 只覆盖本地协议行为，不等同于 live provider 或链上 E2E。
- 中间失败及修复：Alloy/DTO 初次编译缺少新字段；旧应用测试意外触发免 key provider 的真实网络请求；旧断言仍按三家 provider；固定 expiry 测试把 TTL 当绝对时间；fixture 响应栈顺序不符合两阶段请求；统一 helper 触发 Clippy `too_many_arguments`。分别通过补齐 DTO、注入空 services、更新 v1 断言、使用当前时间、修正 fixture 顺序和引入小型 `RouteCandidate` 修复。

### Slice 10：文档与质量门收口

- 更新 README、PRODUCT、TECHNICAL、ARCHITECTURE、SOURCES、VERIFICATION 和 RUST_REWRITE_PLAN；当前文档以 v1 provider-driven chain discovery 和 13 家 adapter 为准，并显式区分 fixture、local Anvil 和 live release gate。
- 当前最终验证命令将在本记录追加后重新执行：Rust fmt/Clippy/unit tests/release build、Foundry fmt/build/test、ignored Anvil E2E 以及 `git diff --check`。
- 尚未改变的边界：不持有用户私钥、不签名、不广播生产交易；没有真实 provider key、生产 RPC、正式 Router/Holder、主网 fork 或外部审计，因此不宣称 live quote、实时流动性或正式执行可用。
- 没有执行 commit、push、部署、生产 RPC 调用、钱包签名或真实网络广播。

## 2026-09-12：v1 多链文档与 provider-driven 实现切片

- 按最新产品决定落库 v1：不配置链、不配置 provider、不配置 token；固定聚合 Matcha Meta DEX Aggregation 页面列出的 13 个 provider，按 provider 支持能力生成链并集和 `chain -> provider list` 反向索引。
- 新增 `src/chains.rs`，将 17 条 EVM 链的 chain ID、展示名称、上游 slug 和 `RPC_URL_<chainId>` 基础设施映射集中管理；Monad Testnet `10143` 使用 Monad 官方开发者文档核对。
- `src/domain.rs` 扩展 13 个 provider ID 和支持矩阵；`Provider` trait 删除 `enabled()`，只保留 `supports_chain(chain_id)` 与 quote。需要 key 的 provider 缺 key 时返回 false，免 key provider 仍可进入索引。
- `src/config.rs` 删除链级 JSON、Router/rules/balanceSlots/token 配置和旧的三字段 credential；改为运行参数、catalog 生成的 chains、RPC 环境变量和各 provider 原生 key map。`config.example.json` 收敛为空对象。
- `src/providers.rs` 新增 `ProviderRegistry`，启动期构造反向索引；`src/competitions.rs` 和 capabilities 改为只读取该索引，不再遍历未经筛选的 provider 全集。
- 删除 token whitelist 依赖：输入 token 只做地址/交易边界验证，Context 不再从 token catalog 推导 decimals/price；没有可靠 token metadata 时 `netOutput` 保持空值，避免默认值伪造比较结果。（当时记录；随后 Slice 9 删除该字段和价格路径。）
- 0x、1inch、KyberSwap 的现有 adapter 保持真实 HTTP 归一化；其余 Barter、Bebop、Enso、HyperBloom、LiquidSwap、Odos、OogaBooga、OKX、OpenOcean、Velora 已注册并按 key/矩阵进入索引，但本切片明确返回 `PROVIDER_ADAPTER_PENDING`，未伪造 live quote。（当时记录；Slice 5 至 Slice 8 已完成这些 adapter。）
- 文档落库：`PRODUCT.md`、`TECHNICAL.md`、`ARCHITECTURE.md`、`RUST_REWRITE_PLAN.md`、`VERIFICATION.md`、`SOURCES.md`、`README.md` 更新为 v1；新增 [ADR-001-v1-provider-driven-chain-discovery.md](decisions/ADR-001-v1-provider-driven-chain-discovery.md)。
- 过程中的首次测试因旧断言仍期待 3 个旧 provider 而失败；根因是免 key provider 按 v1 规则现在会进入索引。断言改为验证当前真实调度结果，并新增 key-aware/chain-specific registry 测试。
- 中间验证：`cargo fmt --all`、`cargo check --all-targets`、`cargo test --all -- --test-threads=1`（20/20）、`cargo clippy --all-targets --all-features -- -D warnings` 均通过。
- 未完成边界：剩余 10 个 provider 还没有官方 response fixture、route target/spender 校验和真实 adapter；本次不宣称它们可 live 报价。没有 commit、push、部署、生产 RPC、钱包签名或广播。

## 2026-09-12：v1 切片最终验证

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过。
- `cargo test --all -- --test-threads=1`：21/21 Rust 单测通过，1 项 Anvil E2E 按默认规则 ignored，doc tests 通过；覆盖 catalog、key-aware reverse index、capabilities、非 catalog chain 拒绝、token 透传路径和旧核心安全边界。
- `cargo build --release`：通过。
- `cargo test --test e2e_local -- --ignored --nocapture`：1/1 通过；本地 Anvil 的 HTTP → Alloy 仿真 → approval → rebuild → swap 闭环未被本切片破坏。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`、`forge test --root contracts`：通过，27/27 测试，3 项 fuzz 各 256 次。
- `git diff --check`：通过。未执行 commit、push、部署、生产 RPC、钱包签名或广播。
- 随后将价格源请求收窄为仅 Ethereum 后再次执行 Rust fmt/Clippy/test、release build、Foundry 门禁和 ignored Anvil E2E，结果保持通过；非 Ethereum 请求不再访问固定 Ethereum 价格源。

## 2026-09-11：按 review 结论迁移可替换基础设施

- 删除 `ContextService`/`SimService` 同签名 adapter，`Services` 直接依赖 `ContextProvider`/`SimulationProvider`；同时移除未使用 helper、重复 dev `serde_json`，并将 Tokio feature 收窄到实际使用集合。
- 用 Alloy Provider 1.6.3 替换生产运行时的手写 JSON-RPC envelope：`eth_chainId`、区块、gas price、code、balance、`eth_call`、receipt 和 `eth_simulateV1` 均通过 typed provider/RPC types；请求级 timeout 由 Alloy 使用的 reqwest client 保留。
- 用 `TransactionRequest`、`StateOverride`、`SimulatePayload`、`SimulatedBlock` 取代 simulation 中的 JSON 拼装和结果字段链；保留 reorg 二次确认、balance slot 覆盖验证、审批校验、最小输出和 gas 余额不变量。
- Provider 适配器改为 serde DTO；只有 Kyber 的原样 `routeSummary`、OneInch 的扩展 `stateOverrides` 和 receipt 透传保留 `serde_json::Value`，避免为供应商不稳定字段增加无价值 schema。
- 本地 E2E 的标准 RPC 改用 Alloy；只保留 Anvil 专有 `anvil_setCode` 与解锁账户 `eth_sendTransaction` 两个测试边界的 `raw_request`，不把它们伪装成通用生产 API。
- 版本取舍：最初尝试 `alloy-provider`/`alloy-rpc-types-eth` 的 1.3.0 时出现 Alloy transitive core 与直接 types 版本不一致；改为精确 pin `=1.6.3` 后通过 Rust 1.89 编译，并保持 `alloy-primitives`/`alloy-sol-types` 的现有兼容版本范围。
- 验证：`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --all -- --nocapture`（18/18）和 `cargo test --test e2e_local -- --ignored --nocapture`（1/1）均通过；最终质量门仍需在本切片结束后重新执行 release build 与合约门。
- 未迁移边界：HTTP 响应体上限/错误映射、Axum body limit、SSE replay/heartbeat/terminal 语义、配置和领域校验、HashMap + Tokio Mutex 状态模型。这些包含项目契约，替换后不会减少正确实现代码。

## 2026-09-11：本切片最终验证

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过。
- `cargo test --all -- --nocapture`：18/18 Rust 单元测试通过，1 项需要 Anvil 的 E2E 按默认规则忽略；doc tests 通过。
- `cargo build --release`：通过。
- `cargo test --test e2e_local -- --ignored --nocapture`：1/1 通过；本地 Anvil 验证标准 Alloy RPC、`eth_simulateV1`、approval、rebuild 和 swap，买币余额为 200、Router 授权清零。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`、`forge test --root contracts`：通过，27/27 测试通过，3 项 fuzz 各 256 次。
- `git diff --check`：通过；未执行 commit、push、部署、生产 RPC 调用或钱包签名。
- 当前 `src/*.rs` 与本地 E2E 合计 4,887 行；迁移后仍保留旧 TypeScript 作为行为对照/回滚材料，不把“删除对照实现”混同为库替换收益。

## 2026-09-11：第二次 review 与最小核心重写

### Review 结论

- 直接依赖的当前事实由 `cargo info` 和官方 docs.rs 复核：`alloy-provider`、`alloy-rpc-types-eth` 最新为 2.4.2，二者 MSRV 为 Rust 1.94.1；`alloy-primitives`/`alloy-sol-types` 仍是独立的 1.7.3 系列；reqwest 最新 0.13.5。没有把 Alloy core 的独立版本号误判成 provider 版本不一致。
- P0/P1 安全边界不替换：`U256` 金额、route allowlist、余额增量、临时授权、固定 block/reorg 检查、请求/响应上限和错误清洗仍是项目特有逻辑，继续保留手写校验。
- 非核心复杂度确认：demo/mock 报价、Base 未完成费用分支、SSE replay/heartbeat/stream permit、OpenAPI runtime endpoint、receipt proxy 和 overquote 诊断不参与最小 quote → simulate → build 闭环。
- 配置 review 发现 Kyber 默认占位 client id 会在删除 demo 后意外触网；已改成 `Option<String>`，无凭据与 0x/1inch 一样返回 `unavailable`。

### 重写内容

- `Cargo.toml`/`Cargo.lock`：Alloy Provider/RPC types 升级到 2.4.2，reqwest 升级到 0.13.5，Rust MSRV/CI/Docker 同步到 1.94.1；适配 Alloy 2.x 新增的 `SimCallResult.max_used_gas`。
- `src/config.rs`：只保留 Ethereum chainId 1，删除 Mode/demo 与 Base 配置分支；供应商凭据缺失时不发起上游请求。
- `src/competitions.rs`：删除事件日志、SSE stream permit、demo 分支和 overquote 计算，保留轮询快照、并发报价、TTL/capacity、token 鉴权和 build 重新报价。
- `src/app.rs`：公开面收敛为 `/health`、`/v1/capabilities`、competition 创建/轮询/build 五类 endpoint，删除 SSE、OpenAPI 和 receipt proxy。
- `src/domain.rs`、`src/rpc.rs`、`src/simulation.rs`、`src/providers.rs`：移除 demo/receipt 运行时支撑，保留 typed Alloy RPC、价格净值、仿真失败分类和 provider 安全归一化。
- `src/openapi.rs`、`.env.example`、`config.example.json`、README、PRODUCT、TECHNICAL、PLAN、VERIFICATION、AI-AGENT、CI、Dockerfile：按最小核心契约同步；旧 TypeScript 仅保留作历史参照。
- `rust-toolchain.toml`：固定 Rust 1.94.1，使 Alloy Provider 2.4.2 的 MSRV 成为可复现的本地与 CI 工具链。

### 中间验证与环境记录

- 第一次安装 Rust 1.94.1 因 rustup 部分组件下载/重命名失败；设置 minimal profile、移除残缺 toolchain 后重试成功，`rustc +1.94.1 --version` 为 `1.94.1 (e408947bf 2026-03-25)`。
- 版本升级后的首次编译只发现 Alloy 2.x 的 `max_used_gas` 初始化缺失，补齐后 `cargo +1.94.1 check --all-targets` 通过。
- 删除核心非功能后，`cargo +1.94.1 test --all -- --nocapture`：17/17 通过；`cargo +1.94.1 clippy --all-targets --all-features -- -D warnings`：通过；`cargo +1.94.1 fmt --all -- --check`：通过。
- `wc -l src/*.rs tests/e2e_local.rs` 当前为 4,345 行；相较本文件上一次记录的 4,887 行，减少 542 行，主要来自删除 SSE/demo/Base/receipt/OpenAPI 运行时，而非用空抽象替代。
- 当前依赖树的直接 Alloy runtime 为 `alloy-provider 2.4.2`、`alloy-rpc-types-eth 2.4.2`、`alloy-primitives 1.7.3`、`alloy-sol-types 1.7.3`；Cargo 没有同时保留 Alloy provider 1.x。

### 未完成边界

- 完整 release build、本地 Anvil E2E、Foundry 门禁将在本切片末尾重新执行；中间验证尚不能代替最终质量门。
- 没有真实供应商 key、生产 simulate RPC、正式 Holder/Router 或主网 fork；删除 receipt proxy 后，交易回执由调用方或钱包侧负责。
- 只支持 Ethereum 是本轮明确的产品收敛，不是声称 Base 或其他链已实现；新增链必须重新评估费用、仿真和 route allowlist。

## 2026-09-11：清理旧工程

- 按用户要求删除旧的 TypeScript 源码、测试、Node 项目配置、npm lockfile、ESLint/Prettier 配置，以及仅服务于旧 runtime 的 agent skill。
- 删除的路径包括 `src/*.ts`、`test/**/*.ts`、`package.json`、`package-lock.json`、`tsconfig*.json`、`eslint.config.mjs`、`.prettierignore`、`.prettierrc.json` 和 `.agents/skills/metamatch-fastify-typescript/`。
- 从 `.gitignore`、`.dockerignore` 和 Dependabot 中移除 `node_modules`、`dist`、coverage 及 npm 更新配置；Docker、CI、README、AGENTS 和技术文档现在只描述 Cargo、Foundry 和 Rust E2E。
- 将剩余项目 skill 改为 Rust/Alloy/serde/Cargo 语义，移除对旧路径、旧测试命令和旧框架的依赖。
- `docs/VERIFICATION.md` 已改为当前 Rust-only 验收记录；本文件保留此前阶段记录，作为本次处理过程的审计留痕，不代表旧工程仍存在或可构建。
- 删除前检查确认工作树不存在 `node_modules`、`dist` 或 `coverage` 目录；未删除 Rust `target` 或 Foundry 产物以外的用户文件。

### 最终验证

- 清理后的第一次 Clippy 发现 `Cargo.toml` 仍声明 Tokio 不存在的 `test` feature；删除该陈旧 feature 后，后续完整门禁全部恢复通过。
- `cargo fmt --all -- --check`：通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过。
- `cargo test --all -- --nocapture`：17/17 Rust 测试通过，1 项 Anvil E2E 按默认规则 ignored，doc tests 通过。
- `cargo build --release`：通过。
- `cargo test --test e2e_local -- --ignored --nocapture`：1/1 通过；本地 Anvil 验证 Rust HTTP、`eth_simulateV1`、approval、rebuild 和 swap。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`：通过。
- `forge test --root contracts`：27/27 通过，3 项 fuzz 各 256 次。
- `git diff --check`：通过。
- `Cargo.toml` 的 `rust-version = "1.94.1"` 与 `rust-toolchain.toml` 保持一致；`cargo tree` 未发现 Alloy Provider 1.x 与 2.x 并存。
- 未执行 commit、push、部署、生产 RPC 调用、钱包签名或真实网络广播。

## 2026-09-12：Axum API 边界最佳实践切片

### 实现内容

- `Cargo.toml` 新增 `axum-extra`，启用 `typed-header` 和 `with-rejection` feature；Cargo 解析到 `axum-extra 0.12.6`，没有引入 controller 或 API framework。
- `src/domain.rs` 新增 typed `CreateCompetitionRequest` 和验证入口；保留旧的 `parse_input(Value)`/`parse_build_request(Value)` 作为内部兼容测试入口，HTTP handler 不再接收 `Json<Value>` 请求。
- `src/app.rs` 使用 typed JSON/path/header extractor；JSON、path、typed header rejection 统一映射到 `AppError`，路由补充 JSON 404/405 fallback，认证失败带 `WWW-Authenticate: Bearer`。
- health/capabilities 也改为 typed serializable response DTO；response layer 为所有 API 响应设置 `Cache-Control: no-store`，避免 competition access token 和交易结果被缓存。
- 新增 `tests/api.rs`，验证 malformed typed JSON、缺少 JSON content type、无效 UUID、缺失 Authorization、404、405、`WWW-Authenticate` 和 `no-store`；测试仍全部位于 `tests/`。

### 验证

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过。
- `cargo check --all-targets`：通过。
- `cargo test --all -- --nocapture`：API 3/3、核心 18/18、provider 12/12；生产 lib/main 0 个测试，E2E 默认 1 项 ignored，doc tests 通过。
- `cargo test --test e2e_local -- --ignored --nocapture`：1/1 通过；typed API/auth 改造未破坏本地 Anvil 闭环。
- `git diff --check` 与 `git diff --cached --check`：通过。
- 未执行 commit、push、部署、生产 RPC 调用、钱包签名或真实网络广播。

## 2026-09-11：新增代码 review 架构导航

- 新增 `docs/ARCHITECTURE.md`，以当前 Rust/Foundry 文件和测试为事实来源，记录启动装配、模块依赖、create → polling → build 生命周期、provider/RPC/simulation 数据流、unsigned transaction 编码、MetaRouter 链上不变量、错误语义、并发/TTL 边界和验证证据。
- 文档为 review 导航，不扩展产品范围；关键模块均附源码行号链接，并提供按阅读效率排序的 review 路径和检查清单。
- README 已增加架构文档入口；本次只增加文档链接和过程记录，没有修改 runtime 或合约逻辑。

## 2026-09-12：Slice 5 至 Slice 10 最终质量门

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过。
- `cargo test --all -- --nocapture`：30/30 Rust 单测通过；1 项需要 Anvil 的 E2E 按默认规则 ignored，doc tests 通过。
- `cargo build --release`：通过。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`：通过；Foundry 仅输出 nightly/deprecation 工具提示。
- `forge test --root contracts`：27/27 通过，3 项 fuzz 各运行 256 次。
- `cargo test --test e2e_local -- --ignored --nocapture`：1/1 通过；本地 Anvil 验证 Rust HTTP → `eth_simulateV1` → approval → rebuild → swap。
- `git diff --check`：通过。
- 依赖复核：Alloy Provider/RPC types 为 2.4.2，reqwest 为 0.13.5；没有真实 provider key、生产 RPC、钱包签名或生产网络广播。

## 2026-09-12：Provider 文件拆分与测试目录归位

### 处理内容

- 删除单体 `src/providers.rs`，改为 `src/providers/mod.rs` 加 13 个独立 provider 模块：`zero_ex`、`one_inch`、`kyber`、`barter`、`bebop`、`enso`、`hyperbloom`、`liquid_swap`、`odos`、`ooga_booga`、`okx`、`open_ocean`、`velora`。
- `src/providers/mod.rs` 只保留 `Provider` trait、provider 注册/反向索引、统一 route 校验和跨 provider 共享的基础类型；每家 provider 的 endpoint、认证、响应 DTO、amount/transaction 适配均归属自己的文件。
- 删除所有生产 Rust 文件中的 `#[cfg(test)]`、`mod tests`、`#[test]` 和 `#[tokio::test]`；核心行为测试移至 `tests/core.rs`，provider fixture 测试移至 `tests/providers.rs`，共享 mock/fixture helper 移至 `tests/support/`。
- 保留 `tests/e2e_local.rs` 作为独立的本地 Anvil E2E；它依赖本地链和合约产物，不混入生产模块。
- 同步更新 `AGENTS.md`、provider/simulation skill、`docs/ARCHITECTURE.md`、`docs/TECHNICAL.md` 和 README，使 review 路径与实际目录一致。

### 过程中的问题与修正

- 初次生成独立模块时，旧 provider 实现被重复拼接；通过逐文件检查 provider struct/trait 实现数量并删除重复尾部修正。
- 将 provider-specific DTO 从 `mod.rs` 移入对应 provider 文件后，`BebopToken` 的 helper 也一并归属 `bebop.rs`，避免共享层反向依赖某一家 provider 的类型。
- 二次 review 又将只被单一家使用的 `barter_*`、`ooga_*`、`raw_to_decimal`、`percentage_value`、Kyber 数量解析和 1inch state override 判断移入各自模块；`mod.rs` 只保留真正跨 provider 的基础校验和公共转换。
- 拆分后先执行 `cargo fmt`、`cargo check --all-targets`，再执行完整测试和 Clippy；未以“编译通过”替代警告和行为验证。

### 最终验证

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过，无 warning。
- `cargo test --all -- --nocapture`：生产 lib/main 为 0 个测试；`tests/core.rs` 18/18、`tests/providers.rs` 12/12 通过；E2E 默认 1 项 ignored；doc tests 通过。
- `cargo build --release`：通过。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`：通过。
- `forge test --root contracts`：27/27 通过，3 项 fuzz 各 256 次。
- `cargo test --test e2e_local -- --ignored --nocapture`：1/1 通过。
- `rg` 复核：`src/` 不含测试宏；13 个 provider 文件各只有一个 provider struct 和一个 `Provider` 实现；仓库不含 `.ts/.tsx` 文件。
- `git diff --check` 与 `git diff --cached --check`：通过。
- 未执行 commit、push、部署、生产 RPC 调用、钱包签名或真实网络广播。

## 2026-09-13：Provider 能力边界与 `supported_chains()`

### 处理内容

- 删除 `domain::ProviderId` enum、`ALL`、字符串转换、provider 链矩阵和凭证判断；`Route`、`Quote`、`BuildResponse` 和 registry 全部使用 provider 自有的静态字符串 ID。
- 将 `Provider` trait 收敛为 provider 自描述接口：`id() -> &'static str`、`requires_access_key()`、`supported_chains() -> Vec<u64>`、provider rules 和 quote。
- 将 13 个 provider 的支持链矩阵迁移到各自 `src/providers/*.rs`；必需 credential 缺失时由该 adapter 返回空 `supported_chains()`，免 key/optional key provider 不受缺 key 影响。
- `ProviderRegistry` 改为消费每个 provider 返回的有效链集合，与 17 条 catalog 求交集后构建 `chain -> provider string ID` 索引；增加重复 provider ID 的启动期 fail-fast 检查。
- 删除 `Chain.rules`；provider rules 通过 provider 接口传递到 route validation 和 simulation。Solidity `MetaRouter.allowed` 仍是最终链上执行边界，Rust 规则只做预校验。
- `Config.provider_keys` 改为字符串键，并把 optional key 传入免 key provider adapter；没有为不同认证形式创建通用 credential abstraction。
- 测试 fixture 中的 route rules 改由 mock provider 返回；新增 provider metadata 测试，验证必需 key、免 key provider、optional key 和字符串 ID 行为。
- 同步更新 `README.md`、`docs/PRODUCT.md`、`docs/TECHNICAL.md`、`docs/ARCHITECTURE.md`、`docs/VERIFICATION.md`、`docs/RUST_REWRITE_PLAN.md` 和 ADR-001，删除旧 `supports_chain`/`ProviderId` 架构描述。

### 验证

- `cargo fmt --all -- --check`：通过。
- `cargo check --all-targets`：通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过，无 warning。
- `cargo test --all -- --nocapture`：API 3/3、core 18/18、provider 13/13 通过；生产 lib/main 0 个测试，E2E 默认 1 项 ignored，doc tests 通过。
- `cargo build --release`：通过。
- `cargo test --test e2e_local -- --ignored --nocapture`：1/1 通过；本地 Anvil HTTP → 仿真 → approval → rebuild → swap 闭环通过。
- `forge fmt --root contracts --check`：通过。
- `forge build --root contracts --deny-warnings`：通过；仅有 Foundry nightly/deprecation 提示。
- `forge test --root contracts`：27/27 通过，3 项 fuzz 各 256 次。
- `git diff --check`：通过；`rg` 复核 `src/` 和 `tests/` 不含 `ProviderId` 或 `supports_chain`。
- 未执行 commit、push、部署、生产 RPC 调用、钱包签名或真实网络广播。

## 2026-09-13：13 家 provider 真实上游 smoke test

### 处理内容

- 新增 `tests/providers_live.rs`，为 `0x`、`1inch`、`kyber`、`barter`、`bebop`、`enso`、`hyperBloom`、`liquidSwap`、`odos`、`oogaBooga`、`okx`、`openOcean`、`velora` 各提供一个独立的 `#[ignore]` 测试入口。
- 测试使用生产 `create_providers`、生产 adapter 和真实 `ReqwestClient`；新增的 `ObservedReqwestClient` 只记录 HTTP status/transport fault 以证明是否收到上游响应，不替换、伪造或改写 response，因此不属于 mock provider。
- live 测试必须同时满足 `METAMATCH_RUN_LIVE_PROVIDER_TESTS=1` 和显式 `--ignored` 才会访问外部服务。缺少必需 key 时沿用 provider 自己的 `supported_chains()` 结果并 `SKIP`；没有 key 需求的 provider 不因缺少 key 被跳过。
- Ethereum、Optimism、Base、Arbitrum 使用公开 WETH/USDC 作为默认 quote 输入；HyperEVM、Berachain 和其他没有安全默认 token 的链通过 `METAMATCH_LIVE_BUY_TOKEN_<chainId>` 等环境变量注入，保持 v1 不维护 token registry。
- 默认模式只证明真实 HTTP response 已返回；需要 key 的 provider 将 401/403 与 transport failure 判为失败，免 key provider 的 HTTP 403 仍记录为上游已响应；`METAMATCH_LIVE_REQUIRE_QUOTE=1` 时要求返回可归一化的 `Route`。测试不持有私钥、不签名、不广播、不写链。

### 验证

- `cargo fmt --all`：通过。
- `cargo check --all-targets`：通过。
- `cargo test --test providers_live -- --list`：通过，发现 13 个独立 live test。
- 真实运行 `METAMATCH_RUN_LIVE_PROVIDER_TESTS=1 cargo test --test providers_live -- --ignored --nocapture`：13/13 测试进程通过；缺少必需 key 的 provider 明确跳过，LiquidSwap 因缺少链专用买入 token 跳过；Bebop 收到 HTTP 200 但没有可用报价，OpenOcean 收到 HTTP 403，Velora 收到并归一化真实报价。
- live smoke 暴露 Bebop 自执行请求缺少 `gasless=false`；按官方自执行 API 契约补入请求参数，并在 `tests/providers.rs` 增加请求断言。补丁后 fixture 13/13 通过，live 结果仍以实时上游状态为准。
- `git diff --check`：通过。
- 真实上游命令固定为 `METAMATCH_RUN_LIVE_PROVIDER_TESTS=1 cargo test --test providers_live -- --ignored --nocapture`；本轮在无 provider key、无链专用 token 的环境中执行，免 key provider 真实访问结果已记录，其他 provider 未因缺少配置而触网，没有把未配置的 key/token 伪造成 live 成功。

## 2026-09-13：live 报价硬门禁与 403 根因复核

### 发现与修正

- 将 live test 从“收到任何 HTTP response 即可”收紧为：实际请求的每个 response 都必须为 2xx，且生产 adapter 必须返回可归一化的真实 `Route`。删除 `METAMATCH_LIVE_REQUIRE_QUOTE` 双模式，避免把 403 或错误 body 记成通过。
- OpenOcean 公开 V4 的 `tokenList`、`gasPrice` 从当前出口返回 200，`quote`、`swap` 返回 403；空 `User-Agent` 仍为同样结果。官方错误表明确 401/402 才是 Pro key 问题，403 是 IP 白名单/安全策略。保留 `requires_access_key=false`，不猜测 header；需由 OpenOcean 加白或提供正式 Pro/Enterprise 接入契约。
- Bebop HTTP 200 无可用报价的根因是 token/taker 地址被共用 formatter 输出为全小写，而 Bebop 要求 EIP-55 checksum。adapter 改用 Alloy `to_checksum(None)`，fixture 同步断言 sell/buy/taker 和 `gasless=false`。
- LiquidSwap HTTP 500 的根因是 adapter 将 native 输入映射为零地址，而官方 route API 要求输入输出均为 contract address。v1 改为只支持 ERC-20 sell，native 明确返回 `NATIVE_SELL_UNSUPPORTED`；live 默认使用官方 WHYPE/USDT0，并通过 HyperEVM 公共 RPC 读取 decimals。
- 删除 LiquidSwap、OpenOcean、Velora 未被公开 API 消费的伪 optional-key 配置；Bebop 保留已验证的 optional Bearer。Pro/Enterprise 认证不在缺少官方契约时臆造。

### 真实报价快照

- `METAMATCH_RUN_LIVE_PROVIDER_TESTS=1 cargo test --test providers_live -- --ignored --nocapture`：12 个测试进程通过、OpenOcean 1 个失败；其中 9 个 key-gated provider 明确跳过且未触网，3 个免 key provider 返回并归一化真实报价，OpenOcean 因 HTTP 403 正确失败。
- Bebop/Ethereum：HTTP `[200]`，`0.01 WETH -> 24.985462 USDC`，minimum `24.985462 USDC`。
- Velora/Ethereum：HTTP `[200]`，`0.01 WETH -> 25.241777 USDC`，minimum `24.989359 USDC`。
- LiquidSwap/HyperEVM：HTTP `[200, 200]`（RPC decimals + route），`0.01 WHYPE -> 0.793763 USDT0`，minimum `0.785825 USDT0`。
- OpenOcean/Optimism：HTTP `[403]`；未把触达或错误 body 记作报价成功。
- 上述只是运行时瞬时 quote；没有签名、广播、写链或声称正式执行成功。

### 回归验证

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过，无 warning。
- `cargo test --all -- --nocapture`：API 3/3、core 18/18、provider 14/14 通过；生产 `src/` 仍为 0 个测试，Anvil E2E 和 13 个 live test 默认 ignored。
- `cargo build --release`：通过。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`：通过，仅有 Foundry nightly/deprecation 提示。
- `forge test --root contracts`：27/27 通过，3 项 fuzz 各 256 次。
- `cargo test --test e2e_local -- --ignored --nocapture`：1/1 通过；仅使用隔离本地 Anvil。
- `git diff --check`：通过。
- 最终完整 live suite：Bebop、Velora、LiquidSwap 返回真实报价；9 个必需 key provider 跳过；OpenOcean 因已定位的 403 唯一失败。这个非零退出码是严格门禁的预期结果，不能改写为全量通过。

## 2026-09-13：13 家官方 API 接入复核与代码纠偏

### 证据与处理方法

- 以 Matcha Meta DEX aggregation 只确定 v1 的 13 家 provider 集合；逐家重新阅读当前官方 API、认证、支持链、金额/滑点单位、native token、spender、transaction target、expiry 和限流说明。
- 新增 `docs/PROVIDER_INTEGRATION_GUIDE.md`，逐家记录能力、申请前提、接入步骤、风险、代码和测试入口；同步更新产品、架构、技术、来源、验证、环境变量与 README。
- provider chain matrix 改为“当前官方 API 支持链与 v1 catalog 的交集”。将历史 Monad Testnet `10143` 替换为当前 Monad mainnet `143`。

### 代码修正

- 0x：删除固定 Holder/target 判断；ERC-20 spender 从 `issues.allowance.spender`/`allowanceTarget` 获取，`transaction.to` 始终按响应使用，native 不生成伪 approval。
- 1inch：按当前 Classic v6.1 补 Monad、Sonic、HyperEVM、Linea 链能力。
- Kyber：公共 legacy gateway 改为免 key 参与，`KYBER_CLIENT_ID` 只作为 optional header；route expiry 收紧为 10 秒。真实测试发现空 User-Agent 会 403，于共享 Reqwest client 设置明确产品 User-Agent，随后 route/build 都返回 200。
- Barter：公开文档未形成稳定 native marker，v1 明确拒绝 native sell；`minReturn` 按官方要求收紧为至少 quote 的 98%。
- Bebop：按公开 `/pmm/chains` 收敛链矩阵；firm quote 缺少 `minimumAmount` 时拒绝，不再本地伪造 provider guarantee。
- Enso：旧 GET/query 改为当前 `POST /api/v1/shortcuts/route` JSON，token/amount 使用数组，保留 `router` strategy、动态 target 和跨链 route 拒绝。
- LiquidSwap：校验 `success` 以及 response tokenIn/tokenOut，继续通过 RPC decimals 把 human-readable input 与 base-unit response 对齐。
- Odos：公共 API 改为免 key 参与，optional key 只透传；native token 映射为官方零地址。
- OKX：从 v5 升到 v6，参数改为 `slippagePercent`；key/secret/passphrase 必需、project ID 可选；ERC-20 请求 approval data，并从 `signatureData.approveContract` 取 spender、从 `minReceiveAmount` 取最低输出。
- OpenOcean：swap 前读取 `/gasPrice`，不再写死 0；按链映射 `eeee`、零地址和 Polygon `0x...1010`；补 Ethereum 与 Monad 143。
- Velora、0x、Enso 等同步当前链矩阵；共享资金和 route 安全不变量未放宽。

### 验证与真实上游结果

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过，无 warning。
- `cargo test --all`：API 3/3、core 18/18、provider 15/15 通过；生产 `src/` 0 个测试，1 个 Anvil E2E 和 13 个 live test 默认 ignored。
- 无 key full live 首次运行：Bebop、LiquidSwap、Velora 返回真实 quote；Kyber 因空 User-Agent 403，Odos 530，OpenOcean `[200, 403]`；7 个必需 key provider 明确 skip。
- 增加共享 User-Agent 后单独重跑 Kyber：HTTP `[200, 200]`，route/build 成功并归一化 `0.01 WETH -> 25.198642 USDC`。
- Odos 530 body 为 Cloudflare error 1033；OpenOcean 403 仍与官方定义的 IP/安全策略一致。两者都不是缺 access key，不修改 `requires_access_key=false`，严格 live 门禁保持失败。
- 所有 live 调用均未签名、未广播、未写链；quote 数字只是 2026-09-13 的瞬时证据。
- 设置 User-Agent 后最终完整重跑 live suite：11 个测试进程通过、Odos/OpenOcean 2 个失败；Kyber 在 full suite 中继续保持 HTTP `[200, 200]`。最新瞬时 quote 记录在 `docs/VERIFICATION.md`。

## 2026-09-14：alloy-chains catalog 与 Alchemy RPC fallback

### 处理内容

- 将 `alloy-chains 0.2.38` 提升为直接依赖；`ChainSpec` 不再存手写数字 ID，改为保存 `alloy_chains::Chain`，并通过 `NamedChain` 常量构建当前 17 条链。为保持 capabilities API 稳定，产品展示名和 provider path slug 继续保留。
- 根据 Alchemy 官方 Chain API supported chains 表逐条核对 Ethereum、Optimism、BSC、Unichain、Polygon、Monad、Sonic、HyperEVM、Mantle、Base、Plasma、Arbitrum、Avalanche、Linea、Berachain、Blast、Scroll 的 mainnet endpoint slug。
- 新增 `ALCHEMY_API_KEY` fallback。解析顺序固定为 `RPC_URL_<chainId>`、Ethereum `ETHEREUM_RPC_URL`、Alchemy；因此用户显式 RPC 永远不会被通用 key 覆盖。
- 使用 `url::Url` 解析所有 RPC 并要求 HTTP(S) host；Alchemy key 作为 path segment 编码。首轮测试发现尾斜杠会产生空 path segment，修正为 `pop_if_empty()` 后再追加 key，避免双斜杠 URL。
- 新增 catalog 全链 Alchemy endpoint、key path 编码、无 key 行为和显式 RPC precedence 测试。测试位于 `tests/core.rs`，生产 `src/` 没有新增测试模块。
- 同步更新 `.env.example`、README、产品、技术、架构、ADR、provider 接入手册、来源与验证记录；没有写入真实 key。

### 验证

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过，无 warning。
- `cargo test --all`：API 3/3、core 20/20、provider 15/15 通过；生产 lib/main 0 个测试，1 个 Anvil E2E 和 13 个 live provider test 默认 ignored。
- `cargo build --release`：通过。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`、`forge test --root contracts`：通过；合约 27/27。
- `cargo test --test e2e_local -- --ignored --nocapture`：1/1 通过，仅访问隔离本地 Anvil。
- 未运行真实 Alchemy RPC：本轮没有读取、打印或消耗用户 key；Alchemy 账户权限、配额和每条链的仿真方法兼容性仍是部署环境 gate。

## 2026-09-14：删除 chain catalog，改为 provider 派生运行时链

### 用户纠偏与设计调整

- 用户指出不应维护 `CHAIN_CATALOG`；链 ID 列表应直接来自当前 provider 实例的 `supported_chains()`，RPC 再按 chain ID 注入。
- 删除 `ChainSpec` 和 `CHAIN_CATALOG`。`ProviderRegistry::new` 不再接收已知链列表或做 catalog 交集，而是直接建立 `chain -> provider IDs`，`chain_ids()` 对索引 key 去重排序。
- `Services::production` 只创建一次 provider 集合：先构建 registry，再把其 chain ID 并集交给 `configured_chains`。`Config` 不再保存或生成链集合，只保存服务参数、provider credential、显式 RPC map 和 Alchemy key。
- 公共 `Chain` 删除 provider path slug。Bebop、Kyber 的 chain path 映射移入各自 adapter，符合 provider-specific 属性由 provider 所有的既有边界。
- `alchemy_rpc_url(chain_id)` 现在公开返回不含 key 的静态 base URL；最终 RPC 解析再用 `url::Url` 把 API key 编码追加。没有官方 Alchemy 映射的新 provider 链仍可通过 `RPC_URL_<chainId>` 工作。
- capabilities 继续只返回 `rpcConfigured` 布尔值，不返回 RPC URL；新增响应测试明确拒绝出现 Alchemy host、fixture key 或 `rpcUrl` 字段。

### 回归范围

- 重写 chain/config/provider registry 测试，使链集合断言以当前 provider 返回值并集为准，不再复制第二份 17-chain catalog。
- 自定义 `Services` 同样过滤掉当前 provider 不支持的链；零 provider 时链集合为空，对任意交易 chain ID 返回 `INVALID_INPUT`。
- 同步更新产品、技术、架构、ADR、provider 接入手册、来源、计划与验证文档，历史处理记录保留以说明纠偏过程。

### 验证

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过，无 warning。
- `cargo test --all`：API 3/3、core 20/20、provider 15/15 通过；生产 lib/main 0 个测试，1 个 Anvil E2E 和 13 个 live provider test 默认 ignored。
- `cargo build --release`：通过。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`、`forge test --root contracts`：通过；合约 27/27。
- `cargo test --test e2e_local -- --ignored --nocapture`：1/1 通过，仅访问隔离本地 Anvil。
- `git diff --check`：通过。未执行真实 Alchemy/provider 请求、签名、广播、commit、push 或部署。

## 2026-09-14：`cargo run` 加载可选 `.env`

### 根因与实现

- Cargo 只继承进程环境，不会自行解析仓库根目录的 `.env`；原入口直接调用 `load_config_from_env()`，因此本地文件中的 RPC 和 provider key 不可见。
- 增加稳定版 `dotenvy 0.15.7`，在读取配置前搜索当前目录及父目录中的 `.env`。已有 shell/部署变量保持优先，不使用 override。
- 只忽略 `NotFound`；文件存在但不可读或格式错误时启动失败，避免 `.ok()` 静默吞掉配置错误。
- dotenv 加载发生在 Tokio 多线程 runtime 创建之前；加载完成后才进入原有异步服务启动流程。生产环境仍可只注入进程变量而不提供 `.env`。
- `.env` 继续由 `.gitignore` 排除。本次没有读取、打印、复制或提交其中的 key。

### 验证

- `cargo check --all-targets`：通过，并将 `dotenvy 0.15.7` 写入 lockfile。
- `cargo fmt --all -- --check`：通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：通过，无 warning。
- `cargo test --all`：API 3/3、core 20/20、provider 15/15 通过；1 个 Anvil E2E 和 13 个 live provider test 默认 ignored。
- `cargo build --release`：通过。
- `PORT=39117 cargo run`：成功监听并通过 Ctrl-C 正常退出；已有 shell 变量覆盖 `.env`。默认端口 3000 当时被其他本地进程占用，未终止或修改该进程。
- 在独立临时目录创建只含 `HOST=127.0.0.1`、`PORT=39119` 的测试 `.env` 后启动 release binary：成功监听 `39119`，证明文件值已加载；临时文件和目录随后删除。
- 在另一个无 `.env` 的独立临时目录以 `PORT=39118` 启动 release binary：成功监听并正常退出，证明生产环境不提供文件时仍可只使用进程变量。
- 使用含未闭合引号的临时 `.env` 启动 release binary：以 `LineParse` 非零退出，证明格式错误不会被吞掉；临时文件和目录随后删除。
- `forge fmt --root contracts --check`、`forge build --root contracts --deny-warnings`、`forge test --root contracts`：通过；合约 27/27。
- `cargo test --test e2e_local -- --ignored --nocapture`：1/1 通过，仅使用隔离本地 Anvil。
- `git diff --check`：通过。未运行 live provider、真实 RPC、签名、广播、commit、push 或部署。

## 2026-09-14：真实 ETH → USDC 重放与双层错误系统

### 授权与处理顺序

用户明确要求重放真实请求、查上游根因，并将 API error 与内部 error 分开。本轮保留已有 staged 修改，在其上修改；使用本地配置的 provider/RPC key，仅查询报价与调用隔离状态仿真，不签名、不广播、不部署、不 commit/push。未改动 `.env`，不记录其内容或 access token。

1. 首先沿 `HTTP/RPC → provider → simulation → competition → API` 检查错误传播。旧 `Fault` 只有 code/status，底层 `map_err` 丢弃原始 cause，仿真又将失败压成字符串，无法从 `RPC_CALL_FAILED` 确定链路。
2. 先增加安全诊断，再以用户原始无 taker 请求重放；初次 0 家仿真成功，确认 RPC `-38014 insufficient_funds`，以及 Enso/Velora 400、OpenOcean schema 错误、Odos 530。
3. 对照官方 Enso、OpenOcean、Velora、Geth 文档和实际响应，修复有证据的请求/schema 与 preview funding 问题，不增加万能 fallback、自动重试或外部服务绕过。
4. 修复后完整重放：0x/Bebop/Enso/Kyber/Velora 5 家真实报价和仿真成功，推荐 Kyber；Odos 530/1033 与 OpenOcean 403 保留为 provider 局部错误。时间、parent block、quote/simulated amounts 和竞赛 ID 落在 `VERIFICATION.md`。
5. 加确定性回归检查，补全架构/接入/运行文档，再跑完整项目门禁；遵循项目验证技能区分 fixture、本地 Anvil 与生产 direct-preview。

### 实现取舍

- 新增 `src/error.rs` 内部 `Fault`，复用 `anyhow` source/context/backtrace；原始库 error 和上游诊断留在 report，常规 Display/Debug/log 不输出私密内容。
- 新增 `src/api_error.rs` 公开 `ApiError`，集中封闭 code/status 映射和 Axum rejection；原 `AppError` 删除，业务/provider 不再决定 HTTP status，未知 code 返回 `INTERNAL_ERROR`。不创建自研堆栈系统或 error registry。
- 复用 `serde_path_to_error` 保留 DTO 字段路径，`thiserror` 包装 HTTP status 和 Alloy 逐笔 simulate call result，`tracing`/`tracing-subscriber` 记录 competition/provider span、安全原因与按需 backtrace。默认不将原始 response/message/data 写日志。
- `SimResult` 携带内部 diagnostic，build 包装错误时保留 cause；provider timeout 进入普通失败分支并产生 quote，不再因早退从结果消失。
- preview 根据实际 payload 的 value + gas-limit 预算注资，而非固定 2M gas；余额 probe gasPrice=0。真实 taker 仍不覆盖资金，minimum/reorg/allowlist 不放松。
- Enso slippage 为字符串，同链 leg chainId 允许省略但明确异链仍拒绝；OpenOcean 从 EIP-1559 `standard.legacyGasPrice` 取 wei；Velora EIP-55，统一保留 preview 地址改用确定性 hash 低 20 bytes。
- `tests/replay_live.rs` 自动将可选 `.env` 解析到局部 map，进程变量优先；显式开关与 ignored 双保险，至少一家真实仿真成功才通过。单家严格 live smoke 仍用于判断各 adapter，不把整体成功解释为两家失败已解决。

### 最终验证

- Rust fmt、Clippy `--all-targets --all-features -- -D warnings`、release build 通过。
- `RUST_LIB_BACKTRACE=1 cargo test --all`：API 3/3、core 20/20、provider 17/17、errors 5/5，共 45 项通过；验证 source/downcast、实际 backtrace 捕获、HTTP/JSON/RPC/revert 脱敏与超时 quote。测试全部位于 `tests/`，生产 src 0 个测试。
- Foundry fmt/build/test 通过，27/27；本地 Anvil E2E 1/1 通过。
- 显式真实 replay 1/1 通过，5 家 preview 成功、2 家外部失败，详见 `VERIFICATION.md`；没有真实用户钱包执行证据。

## 2026-09-14：删除 Fault，直接使用 anyhow

### 原因与边界

用户指出 `Fault` 在 anyhow 之外重复包装；只读 review 后确认 source/context/backtrace 均可交给库。此次按用户批准方案实施：保留原有 API 行为、typed 分类和脱敏边界，删除通用容器。已有 staged/unstaged 工作全部保留，不调用生产上游、不读取或修改 `.env`、不提交或部署。

### 处理过程

1. 检查现有 Fault、API 映射、仿真/build 传播、日志、main 与测试；先明确 source 保留、API code/status、失败 quote 和无秘密输出等不变量。遵循代码简化技能的行为保持要求与项目质量门。
2. 对遍布 domain/config/provider/RPC 的签名与重复 map_err 进行机械迁移，并逐项检查编译结果：直接 `anyhow::Result<T>`，使用 `context`/`with_context`、Option context 和 `bail!`；没有新建 Error/Result 别名或 extension trait。
3. 删除 `Fault` 的 code/report/diagnostic 容器、自定义 Display/Debug/source/context/log 实现；新增仅分类的 `ErrorKind`，通过 anyhow typed context 和 downcast 实现程序判断。build 的外层分类覆盖不再构造嵌套 Fault，原始 cause/backtrace 保持可访问。
4. `ApiError` 只接收 anyhow 并做封闭映射，52 对公开 code/status 与重构前逐项一致；原始错误文本不参与分类。`simulation` 改为内部类型判断，不再依赖 HTTP/API 类型；`SimResult.diagnostic` 直接持有 anyhow error。
5. 将安全日志独立为 `diagnostics::log_error`：只读取已知 typed error/metadata 的安全字段，保留原始 HTTP/RPC/revert 详情但不打印任意 context 或原始 report。删除无错误出口的竞赛 worker Result/join 错误分支，timeout 仍产生失败 quote。
6. main 显式返回 ExitCode，dotenv 加载失败也经过安全 logger。补充真实日志捕获与隔离启动子进程测试；调整的是内部表示断言，没有放松 API/资金/脱敏行为断言。
7. 更新 README、架构与技术文档、验证记录。旧 Fault/live 记录保留为历史留痕，以本条和技术文档第 9 节为当前设计。

### 验证

- `cargo check --all-targets`、Rust fmt、Clippy `--all-targets --all-features -- -D warnings` 通过。
- `RUST_LIB_BACKTRACE=1 cargo test --all`：46/46，覆盖 typed context/source/backtrace、外层分类优先、未知字符串拒绝、日志/API/启动脱敏和 timeout quote。
- `cargo build --release` 通过；Foundry fmt/build/test 通过（27/27）；`cargo test --test e2e_local -- --ignored --nocapture` 本地 Anvil 1/1。
- `git diff --check` 通过；src/tests 无 Fault 或兼容包装，simulation 无 api_error 依赖；未增加依赖或执行 live 重放。

## 2026-09-14：进一步简化内部日志

用户明确要求暂不考虑安全日志、避免过多复杂度。本条替代上一条的内部日志脱敏设计；公开 API 错误契约不变。

1. 按代码简化技能检查日志调用、错误类型和测试，将改动限定在诊断表示与输出，不改 provider 请求、报价、仿真资金规则或配置。
2. 删除 `diagnostics` 专用模块、字段白名单、`upstream_reason` 文本分类器、`RpcMethod` 元数据类型和自定义脱敏 Debug。失败边界直接 `tracing::warn!(error = ?error, "操作失败")`，沿用 competition/provider span。
3. HTTP 错误仅保留 status/body，RPC 方法使用普通 anyhow context；原始 Alloy/revert/serde cause 继续保留。沿用已有 HTTP 响应大小上限，不增加日志框架、配置项或依赖。
4. `main` 返回 `anyhow::Result<()>`，保留 dotenv 在线程创建前加载和非 NotFound 错误失败退出。内部日志现在允许包含上游详情；不新增请求 header、配置或认证信息日志。
5. 移除自制日志捕获工具，测试直接检查 anyhow 原因链、typed downcast、backtrace、API 详情隔离和启动失败上下文；所有测试仍在 `tests/`。更新 README、架构和技术方案，保留旧记录作为历史。

验证结果见 [VERIFICATION.md](VERIFICATION.md) 的“简化内部日志”条目。本轮不读取或修改开发者 `.env`，不运行生产 provider/RPC 重放，不提交或部署。

## 2026-09-14：落实 review 第 3 节 A–D，不减少产品能力

### 范围与基线

用户批准的是代码精简，不是删产品功能。保留全部 13 个 provider、provider 派生多链、异步竞赛/polling/access token、重新 build、Router/Holder、balance-slot override、现有排序和费用字段。未实施 review 中需要削减产品能力的其他选项。

修改前将当前 `src/`、`tests/` 复制为临时基线，比较的是本轮开始时的工作区，不把之前的 staged/unstaged 重构计入成果。使用代码简化、工程实施和项目领域/仿真/验证技能：先固定行为边界，再用针对性回归证明修改，最后运行完整质量门；不新增依赖、日志框架、配置层或自动重试。

### 处理过程

1. **A — 生命周期交给 Tokio。** 先添加可控暂停的 build 回归：旧实现 abort 后仍占用唯一名额，后续 build 得到 `BUILD_CAPACITY_EXCEEDED`，测试失败。将竞赛与 build 的原子计数改成独立 Semaphore permit 后，同一测试通过；后续补充普通失败后释放名额的断言。删除只抑制状态更新、并不真正取消请求的 cancel flag；close/TTL 明确只清理快照，进行中的上游请求仍受已有 timeout 约束。
2. **B — 仿真只走一条失败通道。** 删除 `SimResult.diagnostic`、`Ok(failure(...))` 和失败时复制 approvals/transaction 的代码。失败统一返回 anyhow；用小型 `SimulationFailure` typed context 保留公开的 reverted/unsupported/error 及 reason，原始 Alloy/逐笔 revert cause 继续可 downcast。build 在同一错误链上追加公开分类，不重建错误容器。
3. **C — 直接复用 Alloy。** 删除 `RpcFactory`、`EvmRpc`、`AlloyRpcFactory`、`AlloyRpc` 和 `BlockInfo` 转发表示。context 与 simulator 共用 `RpcClients` 的 reqwest 连接池和按 URL 复用的 `DynProvider`，直接调用 Alloy typed RPC。保留 ContextProvider/SimulationProvider 业务测试边界；旧 RPC trait fixture 改成本地 JSON-RPC HTTP 服务，让测试实际经过 Alloy 请求编码和响应解码。
4. **D — 只合并相同知识。** provider 归一化与执行校验共用 expiry/native value/calldata 检查；保留正数解析、provider 金额错误和执行 allowlist 等不同职责。归一化提前应用执行端已有 calldata 长度上限；若同一响应同时存在多个非法字段，首个错误可能随校验顺序改变，合法请求和保护条件不变。删除 `parse_uint_checked`、`format_quantity`、未使用的 `parse_build_request`，将仅测试使用的 Value 解码 helper 移入测试。camelCase 字段交给 Serde，保留特殊 `routerAddr`、默认值、unknown-field 与 Option 规则。
5. 删除只有 `{}` 的 `config.example.json`，更新 README 和当前架构/技术说明；可由 Git 恢复，不涉及真实配置或数据。旧迁移和 live 记录继续作为历史，不改写为本轮验证。

### 结果与验证口径

生产 Rust 源码由 **4,690 行降到 4,387 行，净减少 303 行（约 6.5%）**；统计包含空行和注释，不包括测试、文档和合约。主要减少来自 RPC（224 → 116 行）、simulation（508 → 415 行）与 competitions（592 → 559 行）。并未为了行数压缩资金逻辑或移除测试。

新增 `tests/concurrency.rs` 与 `tests/simplification.rs`，覆盖取消/失败释放容量、公开 build 分类与原始 cause、真实 Alloy 本地编解码、固定 block/零 gas probe/资金 override 边界、ERC-20 资金/approval 拒绝、共享交易校验与公开 JSON 字段。所有 Rust 测试仍放在 `tests/`。

完整命令与计数见 [VERIFICATION.md](VERIFICATION.md)“不减功能精简 A–D”。本轮没有读取开发者 `.env`、访问生产 provider/RPC、提交、push 或部署；本地 Anvil 证据不代表主网执行或最新供应商可用性。
