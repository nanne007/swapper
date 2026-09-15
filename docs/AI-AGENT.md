# MetaMatch AI agent 设置

这份说明定义仓库内 agent 的工作边界。Rust Cargo crate 是唯一后端 runtime；Foundry 是合约与本地 EVM 的验证入口。任何 agent 都不得访问密钥、签名交易或部署合约。

## 三层设置

1. `AGENTS.md` 是 coding agent 的总规则：产品范围、模块边界、资金安全不变量、禁止事项和质量门。
2. `.agents/skills/*/SKILL.md` 是按任务触发的低上下文工作流，只加载当前任务需要的 skill。
3. `.github/workflows/ci.yml` 调用 `scripts/check.sh` 执行与本地一致的质量门，不依赖 agent 是否记得执行命令。CI 不持有生产密钥，也不执行真实网络部署和广播。

## Skill 选择

| 任务 | 使用 skill | 主要约束 |
| --- | --- | --- |
| 修改 competition、quote、simulation、transactions、ranking、总 deadline 或公共状态 | `metamatch-domain-modeling` | simulated output、overridden funding、状态语义、taker 绑定、底价不可降低、fixture 边界 |
| 修改 0x、1inch、KyberSwap 或其他报价源 | `metamatch-provider-adapter` | 固定官方 endpoint、serde DTO、target/spender/selector/value 校验、fixture 契约 |
| 修改 RPC、`eth_call`、`eth_simulateV1`、state override、费用或重组检查 | `metamatch-evm-simulation` | Alloy typed RPC、固定 block context、顺序调用、余额差、gas、unsupported/reorg |
| 修改 MetaRouter、AllowanceHolder、Token、退款、receiver 或最小到账 | `metamatch-contract-safety` | permissionless、Holder sender 身份、精确输入与临时授权、receiver 最终余额差、净 sold、退款与重入、Foundry fuzz；无关暂存资产不受保护 |
| 修改测试、fixture、Foundry fuzz、Anvil E2E 或验收证据 | `metamatch-test-verification` | 行为/不变量、证据分层、避免过度 mock、区分 fixture 和 local EVM |
| 修改 Cargo、Clippy、Foundry、CI 或准备交付 | `metamatch-quality-gates` | fmt → clippy → test → release build → contract → E2E |

Skill 只提供流程和领域知识，不扩大权限。新增公共 API 时必须同步 Rust serde/domain 类型、测试和产品/技术文档。

## Plugin 策略

本项目不自动安装外部 plugin。固定 HTTP 适配器、服务端 RPC 和 Foundry/Anvil 已覆盖当前核心闭环；额外 plugin 可能引入第三方账号、私有数据、出站网络或部署能力。

只有明确提出对应需求时才评估 plugin，并记录权限、数据流、可撤销方式和替代方案。任何 plugin 都不能代替固定 URL、服务端校验、fixture 测试或 Foundry 质量门。

## Agent 运行建议

- 默认不生成 mock 报价；没有真实 RPC、API key 或 Router 时保持可启动，但明确返回 `unavailable` 或显式错误。
- 修改前读取 `AGENTS.md` 和相关产品/技术文档，运行 `git status --short`，保留用户已有改动。
- 对金额、gas、价格和余额使用 `alloy_primitives::U256` 与定点字符串，不使用浮点数。
- 供应商失败、RPC 不支持、费用未知和正式部署缺失必须显式标记，不得用估算冒充成功。
- 本地 fixture、mock、Anvil 和真实网络证据必须分开陈述。
- 质量门按最小相关范围执行，交付前执行完整 Cargo、Foundry 和本地 E2E 验证。
