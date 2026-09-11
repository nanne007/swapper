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

## 2026-09-11：新增代码 review 架构导航

- 新增 `docs/ARCHITECTURE.md`，以当前 Rust/Foundry 文件和测试为事实来源，记录启动装配、模块依赖、create → polling → build 生命周期、provider/RPC/simulation 数据流、unsigned transaction 编码、MetaRouter 链上不变量、错误语义、并发/TTL 边界和验证证据。
- 文档为 review 导航，不扩展产品范围；关键模块均附源码行号链接，并提供按阅读效率排序的 review 路径和检查清单。
- README 已增加架构文档入口；本次只增加文档链接和过程记录，没有修改 runtime 或合约逻辑。
