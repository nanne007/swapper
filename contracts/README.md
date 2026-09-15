# MetaRouter

更新：2026-09-15。Solidity 0.8.25、Cancun EVM，无第三方 Solidity 依赖。当前为本地实现，未部署、未外部审计；部署没有自动化。

## 交易入口与资金边界

构造函数为 `(owner, allowanceHolder)`。`owner` 即管理员，建议使用多签；Holder 必须是所选链经过验证的正式 AllowanceHolder，且部署时已有合约代码。owner 不能为零地址、Router 自身、Holder 或 native sentinel。

```text
钱包 → AllowanceHolder.exec(router, sellToken, amount, router, executeCalldata)
     → MetaRouter.execute → provider target
```

路由必须由管理员显式登记 `(target, spender, selector)` 三元组，默认全部拒绝。`allowed(keccak256(abi.encode(target, spender, selector)))` 查询权限，`setAllowed` 启用或撤销，变更发出 `RoutePermission`；执行未登记元组时回滚 `RouteNotAllowed`。Rust 通过 `Rule` / `Provider::rules(chainId)` 做同样的预检，未命中返回 `ROUTE_NOT_ALLOWLISTED`。链下规则不能替代链上登记，也不得根据上游返回值自动加白。

保留以下执行约束：

- 只有配置的 Holder 可以调用 `execute`；真实 taker 从 Holder 追加的 ERC-2771 sender 解析，校验 calldata 长度防止伪造后缀。钱包授权给 Holder。
- sellAmount / minBuyAmount 必须非零，买卖 token 不同；buy token 必须为 ERC20。native sentinel 为 `0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE`，不使用零地址代表 native。
- native 输入的外层 `msg.value` 等于 sellAmount，provider value 不超过 sellAmount；ERC20 输入的两个 value 均为零。Rust 原有约束仍要求 provider native value 等于 sellAmount。
- target/spender 必须有代码且不能为 Router；target 不能为本次 sell/buy token，calldata 至少有 4 字节 selector。
- Router 对 target 调用 `balanceOf(address(router))`；若成功且返回至少 32 字节，则将其视为 ERC20 入口并拒绝。这样阻止通过路由直接 `transfer/approve` Router 中的第三种暂存 token。此检查也会拒绝具有相同余额接口的 vault/其他入口；它是执行形状限制，不是地址白名单，也不是完整 ERC20 识别或恶意 token 审计。
- 精确拉取 sellAmount，确认实际收到完整数量；给 provider spender 的额度仅为本次金额，并在外部调用后归零。
- provider 可以将输出发给 Router 或 taker。Router 仅转出本次 buy/sell/native 增量，保留历史余额；完成全部退款和回调后，检查 taker 的实际买入余额增量是否达到 minBuyAmount，不足则整笔回滚。
- pause、deadline、重入检查继续生效。fee-on-transfer/rebasing token 不在支持范围内。

0x 嵌套 Holder 保留固定调用形状：只要 target 或 spender 为 Holder，就必须两者均为 Holder，且 selector 为 `exec`。内层看到的 owner 是 Router，operator 为下游执行者，临时权限与外层 taker 的权限分开；该白名单只覆盖外层 `(Holder, Holder, exec)`，内层 target/operator 无管理员白名单，不能据此声称内层执行者已获审核。正式 Holder 还必须拒绝直接 ERC20 target，以保护其永久授权。

## 管理员接口

| 接口 | 权限与行为 |
| --- | --- |
| `owner()` / `pendingOwner()` | 查询当前管理员/待接任管理员；无人待接任时 pendingOwner 为零 |
| `setPaused(bool)` | 仅 owner；暂停或恢复 swap |
| `setAllowed(address target, address spender, bytes4 selector, bool enabled)` | 仅 owner；启用或撤销路由三元组，发出 `RoutePermission` |
| `allowed(bytes32 key)` | 查询三元组 hash 对应的权限，默认 false |
| `recoverToken(address token, address payable recipient, uint256 amount)` | 仅 owner；提取 Router 当前持有的 ERC20 或 native 指定数量 |
| `transferOwnership(address newOwner)` | 仅 owner；提名接任者，覆盖先前提名，当前 owner 此时不变 |
| `acceptOwnership()` | 仅 pendingOwner；接任并清空 pendingOwner，旧 owner 立即失去管理权限 |

### 路由白名单

管理员只应登记经过审核的目标、spender 和函数组合。启用时 target/spender 必须有代码且不能为 Router；涉及 Holder 时只接受 `(Holder, Holder, exec)`。撤销不要求目标仍有代码，暂停期间也可管理白名单。登记不会绕过 `execute` 中的直接 ERC20 target、资金和重入检查。

```solidity
router.setAllowed(providerTarget, providerSpender, providerSelector, true);
// 撤销同一元组；其他 selector 或 spender 不会因此获得权限。
router.setAllowed(providerTarget, providerSpender, providerSelector, false);
```

白名单是信任边界，不是安全审计证明：已登记入口若允许任意下游调用，必须单独审核其权限和 calldata 约束；特别是嵌套 Holder 的内层身份不由外层元组验证。合约升级、代理实现变化和登记权限泄露也需独立管理。

### Recover token（包括 native）

```solidity
// 以下调用均由当前 owner 发起；amount 是 token base units/native wei。
router.recoverToken(tokenAddress, payable(recipient), amount);
router.recoverToken(router.NATIVE(), payable(recipient), amountWei);
```

允许部分或全部提取，数量须明确传入，没有“0 表示全部”的特殊语义。recipient 不能为零或 Router 自身，amount 必须大于零。非 native token 必须有代码。余额不足、ERC20 返回 false/revert/非法返回值或 native recipient 拒收均整笔回滚；兼容 transfer 无返回值的 ERC20。

recover 在暂停期间仍可使用，与 `execute` 共用重入锁：即使回调合约本身是 owner，也不能在 swap 途中提取资金，不能在 recover 的回调中再次 recover 或发起 swap。成功时发出 `TokenRecovered(token, recipient, amount)`。

管理员可以提取 Router 中的历史余额、误转资产及其他暂存资产；这些资产不再具有“永远不能被管理员移动”的保证。recover 只能从 Router 自身转出资产，不会通过 Holder 拉取用户钱包资产，也不提供任意 calldata 管理接口。Router 不应作为用户存款保管地址。

### 两步管理员转移

1. 当前 owner 调用 `transferOwnership(newOwner)`，产生 `OwnershipTransferStarted(previousOwner, newOwner)`。
2. newOwner 使用自己的账户或多签调用 `acceptOwnership()`，产生 `OwnershipTransferred(previousOwner, newOwner)`。
3. 核对 `owner()` 已更新、`pendingOwner()` 为零。接任前原 owner 保留 pause/recover/白名单管理/重新提名权限；接任后只有新 owner 有这些权限。

newOwner 不能为零地址、Router、Holder 或 native sentinel；可以是 EOA 或能调用接受接口的合约。没有 renounce 接口；提名填错时由当前 owner 重新提名正确地址。pendingOwner 尚未接受时不能执行 pause、recover 或白名单管理。所有权转移不会清空已登记的路由。合约部署也会产生从零地址到初始 owner 的 `OwnershipTransferred` 事件。

## 兼容性与验证

`execute`、Holder 外层编码和构造参数顺序保持不变；owner 从 immutable 改为可转移状态，新增 pendingOwner。保留原有白名单 ABI 和 Rust 的 `ROUTE_NOT_ALLOWLISTED` 错误。调用方应使用包含 recover/ownership 的 ABI，保留 setAllowed 操作，并接入两步管理流程。

此合约不是代理，已有部署不能原地升级到本版本。若已有旧地址，需另行部署并更新引用；本次没有部署操作。生产运行时的 `Chain.router` 仍为 None，正式地址接入尚未实现；当前生产 provider 沿用默认空 rules，需要补充经审核的链专属规则并同步链上登记，才能开放正式执行。

从仓库根目录运行：

```sh
forge fmt --root contracts --check
forge build --root contracts --deny-warnings
forge test --root contracts
cargo test --test e2e_local -- --ignored --nocapture
```

Foundry 覆盖路由默认拒绝、启用/撤销、target/spender/selector 精确匹配、管理员接任后的白名单权限、第三种 token 的 transfer/approve 拒绝、caller/后缀、金额/value、最低到账、历史余额、退款、授权清理、嵌套 Holder、管理员转移、ERC20/native recover 与交叉重入。Rust Anvil E2E 显式调用 setAllowed 并配置对应的 provider rules，再验证 HTTP → 仿真 → approval → swap。Mock Holder 是测试替身；正式 Holder/provider 的主网 fork 兼容性与外部审计仍需独立完成。
