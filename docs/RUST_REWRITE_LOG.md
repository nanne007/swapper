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
