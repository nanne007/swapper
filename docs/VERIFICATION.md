# 实现与验收记录

本记录描述当前 Rust runtime、本地验证和未验证生产边界，不表示服务已经过主网审计或可以直接投入真实资金。

## Rust 最小核心验收（2026-09-11）

根目录 Cargo crate 是唯一构建、启动和 CI 入口。

| 检查 | 结果 | 覆盖与边界 |
| --- | --- | --- |
| `cargo +1.94.1 fmt --all -- --check` | 通过 | Rust 源码格式 |
| `cargo +1.94.1 clippy --all-targets --all-features -- -D warnings` | 通过 | 所有 target、所有 feature，警告视为错误 |
| `cargo +1.94.1 test --all -- --nocapture` | 17/17 通过；1 项 E2E 默认 ignored | domain/config、HTTP/provider/RPC fixture、ABI、仿真失败分类、轮询生命周期和 build 边界 |
| `cargo +1.94.1 check --all-targets` | 通过 | Alloy 2.x typed RPC 与所有 target 编译 |
| `cargo +1.94.1 build --release` | 通过 | Rust release binary |
| `cargo +1.94.1 test --test e2e_local -- --ignored --nocapture` | 1/1 通过 | 本地 Anvil：HTTP → `eth_simulateV1` → approval → rebuild → swap |
| `forge fmt --root contracts --check` | 通过 | Solidity 格式 |
| `forge build --root contracts --deny-warnings` | 通过 | Foundry 合约编译 |
| `forge test --root contracts` | 27/27 通过 | minimum output、授权、历史余额、sender 伪造、重入和嵌套 Holder |
| `git diff --check` | 通过 | 差异无空白错误 |

当前验证没有使用真实供应商 key、生产 RPC、钱包或主网广播；本地 E2E 不是主网 fork，也不证明正式 Holder、Router 或真实供应商可执行。

## 当前最小核心

- 只支持 Ethereum、0x/1inch/KyberSwap、固定 parent block 仿真、净到账排序、polling 快照和重新报价 build。
- 未配置供应商凭据时不发起上游请求，返回 `unavailable`；没有 Router 时只允许 direct preview，build 明确失败。
- 服务只返回 unsigned approval/swap 交易，由调用方钱包完成签名和广播。
- 状态保存在单进程内存中，具备 TTL、容量限制、Bearer token、请求体上限、超时和错误清洗。
- Solidity 合约仍由 Foundry 维护和验证；本次未修改合约设计。

## 未验证边界

1. 没有真实供应商 API key、生产 simulate RPC、正式 Router/Holder 部署或主网 fork，因此真实报价、费用、地址和执行可用性仍未验证。
2. 本地 E2E 的 Holder 是测试 mock 写入固定地址，不是从主网读取的正式字节码；正式 Holder 与嵌套路由必须补 fork 验证。
3. Router 未部署、未外部审计；服务不签署、不广播真实网络交易。本次唯一广播发生在测试进程创建的隔离本地 Anvil，测试完成即关闭。
4. Base、其他链、跨链、原生币 buy、intent、平台抽成和公网身份系统不属于当前最小核心。
5. Redis/Postgres、多副本、分布式限流、业务审计库、供应商熔断和 RPC 容灾未实现；当前是有界单进程版本。
6. Dockerfile 已提供，但 Docker daemon 不可用，镜像 build/run 未验证。
7. 仿真、费用估算和报价都不是未来成交保证；必要 approval 后必须重新 build，最终仍由链上 minimum output 保护。

## 文件位置与交付

本次修改直接发生在 `/Users/caojiafeng/Documents/ChatGPT/metamatch`，没有创建 commit、push、部署或广播交易。生成的 `target/`、Foundry 构建物和本地环境文件不纳入交付。详细处理过程、删除范围和判断依据见 [RUST_REWRITE_LOG.md](RUST_REWRITE_LOG.md)。

## 上线前顺序

申请并核对供应商服务权限 → 配置专用 simulate RPC → 正式 Holder/Router/provider fork 测试 → 外部合约审计 → 多签部署与最小白名单 → 完整监控/访问控制 → 小额人工验收。其他链要另立产品范围并补齐费用模型、route 验证和测试。
