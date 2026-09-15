# MetaRouter

更新：2026-09-15。本文对照当前 [合约源码](src/MetaRouter.sol) 和 [Foundry 测试](test/MetaRouter.t.sol)，描述无管理员版本及最新 `execute` ABI。编译配置见 [foundry.toml](foundry.toml)：Solidity 0.8.25、Cancun EVM、optimizer 200 runs、via IR，无第三方 Solidity 依赖。

## 设计与边界

MetaRouter 是通过 AllowanceHolder 使用的轻量 swap 执行器，不是资产保管合约。它负责本次交易的输入拉取、临时授权、退款和最低实际到账检查，不负责审核 provider 或保护无关暂存资产。

当前没有 `owner`、`pendingOwner`、`paused`、路由白名单、管理员转移或 ERC20/native recovery。部署者没有特权，不能暂停交易、登记路由或找回误转资产。新 target、spender 和 selector 无需登记，但仍须通过下述结构及资金检查。

不要提前向 Router 存币。任意第三种暂存 token 可能被其他调用者通过路由转走或授权；误转资产也没有恢复保证。移除管理功能是主动缩小合约职责，不意味着任意路由或 token 都可信。

## 部署与公开接口

构造函数只接收一个地址：

```solidity
MetaRouter router = new MetaRouter(allowanceHolder);
```

构造函数只检查 Holder 地址已有合约代码，不验证其具体实现。必须由部署方确认其为可信且兼容的 AllowanceHolder；`allowanceHolder` 是 immutable，部署后不可更换。本合约没有代理升级接口。

除 `receive()` 外，公开函数只有：

- `NATIVE()`：返回原生币 sentinel `0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE`，不是零地址。
- `allowanceHolder()`：查询固定的 Holder 地址。
- `execute(...)`：经 Holder 执行交易。

`receive()` 可接收 provider 的 native 退款，也接受直接转入的 native；直接转入并不创建存款记录或提款权。

## execute ABI 与参数

```solidity
function execute(
    address sellToken,
    address buyToken,
    address receiver,
    uint256 sellAmount,
    uint256 minBuyAmount,
    uint256 deadline,
    address spender,
    address target,
    uint256 value,
    bytes calldata data
) external payable nonReentrant returns (uint256 boughtAmount)
```

规范签名为 `execute(address,address,address,uint256,uint256,uint256,address,address,uint256,bytes)`，selector 为 `0xf509c3a5`。注意：`receiver` 紧跟 `buyToken`，`deadline` 紧跟 `minBuyAmount`，`spender` 在 `target` 前面。

| 参数 | 含义与约束 |
| --- | --- |
| `sellToken` | ERC20 地址或 `NATIVE`；非 native 时必须有代码 |
| `buyToken` | ERC20 地址，必须有代码且不同于 sellToken；不支持 native 买入 |
| `receiver` | 买入 token 接收地址，可与 sender 相同；不能为零地址、Router、Holder 或 NATIVE；可以是 EOA 或合约 |
| `sellAmount` | 必须大于零；ERC20 精确拉入量和本次 spender 授权额度，native 时等于外层 msg.value |
| `minBuyAmount` | 必须大于零；本次执行结束后 receiver 的 buyToken 余额增量下限 |
| `deadline` | Unix 秒；仅当 block.timestamp > deadline 时过期，等于 deadline 仍可执行 |
| `spender` | 本次 sellToken 的授权接收者，可以不同于 target；即使 native 卖出也须有代码且不能为 Router |
| `target` | 接收外部调用的合约；须有代码，不能为 Router 或本次 sellToken/buyToken |
| `value` | 发送给 target 的 native 数量；native 卖出时不得超过 sellAmount，ERC20 卖出时必须为零 |
| `data` | 原样发送给 target 的 calldata，至少包含 4 字节 selector |

金额均为 token base units/native wei。Router 不读取 decimals、不查询价格，不验证 calldata 内的金额或 recipient 是否与外层参数一致；最终以余额检查判断是否成功。

返回值 `boughtAmount` 是全部转币、退款和回调结束后，receiver 的 buyToken 余额相对于执行前的净增量，不是 provider 的返回值。

## sender 与 receiver 的职责

sender 是可信 Holder 追加的真实调用者，不是 execute 的可选参数；receiver 是 sender 随交易指定的买入收款人。

| 行为 | 使用的地址 |
| --- | --- |
| 从钱包拉取 sellToken | sender |
| 退还剩余 sellToken/native | sender |
| 转出 Router 的 buyToken 增量 | receiver |
| minimum 与 boughtAmount 的余额基线/最终余额 | receiver |

receiver = sender 时沿用自收款行为，没有零地址代表 sender 的特殊规则。receiver 不需要授权 Holder，也不必能接收 native；但 sender 为合约且存在 native 退款时必须能接收。

provider 可以把输出交给 Router，再由 Router 转给 receiver；也可以直接付给 receiver。Router 不改写 provider calldata 的 recipient。若 receiver 与 sender 不同，而 provider 只向 sender 付款，且 receiver 未得到足够输出，则 minimum 检查失败并整笔回滚。调用方必须同步设置 provider recipient，不能只修改外层 receiver。

## 调用方式

```text
sender → AllowanceHolder.exec → MetaRouter.execute → target
                                      │
                                      └→ 清理授权、退还 sender 输入、检查 receiver 最终到账
```

钱包不能直接调用 `execute`，也不能自行拼接 sender 后缀绕过 Holder。外层 Holder 的调用形状是：

```solidity
// 以下为调用片段；holder 使用所选链的 AllowanceHolder exec 接口。
// ERC20 输入前，sender 先通过 token 的 approve 授权 Holder，而不是 Router。
bytes memory execution = abi.encodeCall(
    MetaRouter.execute,
    (
        sellToken,
        buyToken,
        receiver,
        sellAmount,
        minBuyAmount,
        deadline,
        spender,
        target,
        value,
        data
    )
);

// 由 sender 调用；Holder 负责在 execution 后追加真实调用者的 20 字节地址。
bytes memory result = holder.exec{value: nativeInput}(
    address(router), sellToken, sellAmount, payable(address(router)), execution
);
uint256 boughtAmount = abi.decode(result, (uint256));
```

其中 `nativeInput` 在 ERC20 卖出时为零，在 native 卖出时为 `sellAmount`。不要把它与传给 target 的 `value` 混淆：native 的 `value` 可以小于 `sellAmount`，差额参与最终退款。

如果通过另一个合约调用 Holder，被转发的 sender 是该合约，不自动等于发起交易的 EOA。资产拉取和输入退款都会绑定此 sender，买入资产则交给显式指定的 receiver；不要把 receiver 当作拉币 owner，也不要把此路径当作任意指定 sender 的接口。

## execute 的执行顺序

1. **加锁与验证入口。** 重入直接失败；调用者必须是固定 Holder；检查 deadline。从 calldata 末尾读取 Holder 追加的 20 字节 sender。总长度必须为 `4 + 10 * 32 + 32 + ceil(data.length / 32) * 32 + 20`。应使用标准 ABI 编码，不自行添加后缀；零地址、Router 自身或 Holder 不能作为 sender。
2. **验证交易参数。** 检查正数金额、不同的买卖 token、合法 receiver、target/spender 的代码及排除地址、至少 4 字节的 data。native 卖出要求 `msg.value == sellAmount` 且 `value <= sellAmount`；ERC20 卖出要求 `msg.value == 0 && value == 0`。
3. **记录本次交易的余额基线。** 记录 Router 的 sellToken（仅 ERC20）、buyToken 和 native 历史余额，以及 receiver 的 buyToken 初始余额。native 历史余额为 `address(this).balance - msg.value`，因此不包含本次入金。
4. **拉币与授权。** ERC20 路径通过 Holder 从 sender 拉入恰好 sellAmount，要求 Router 实际余额增加同样数量；随后先将对 spender 的 allowance 清零，再设为 sellAmount。native 路径不拉 ERC20、不设置 allowance。
5. **调用 target。** 执行 `target.call{value: value}(data)`，不是 delegatecall。调用失败时原样冒泡其 revert data；成功时不解析 provider 返回数据。普通 target 看到的 msg.sender 是 Router。
6. **清理 ERC20 授权及剩余输入。** 将 sellToken 对 spender 的 allowance 归零。Router sellToken 余额不得低于步骤 3 的基线；若高于基线，则将全部差额转给 sender。
7. **转出 buyToken 增量。** Router buyToken 余额不得低于其基线；将高于基线的差额转给 receiver。provider 也可以直接向 receiver 输出，两种路径都会反映在最终到账检查中。
8. **退还 native 增量。** Router native 余额不得低于其基线；将差额通过空 calldata 的 native call 退给 sender。sender 拒收则整笔回滚。此步骤也适用于 ERC20 swap 期间收到的 native。
9. **最后检查最低到账。** 在上述转币和退款回调完成后重新读取 receiver 的 buyToken 余额。若余额下降，或净增量小于 minBuyAmount，则回滚；否则返回增量并发出 Executed。退款回调不能通过重入再次执行 swap。

以上步骤中的失败使整笔调用回滚，包括此前的拉币、授权、provider 调用及退款效果；不是部分成交或静默跳过错误。

### 输入与输出的准确语义

`sellAmount` 是精确拉入量和授权上限，不是强制净消耗量。允许部分成交并退回未消耗输入；代码也不要求实际消耗大于零。只要其他检查通过且 receiver 收到足够 buyToken，完全不消耗输入也可以成功。

minimum 检查只证明所选 buyToken 报告的余额净增加，不证明输出来自哪个池子、是否由 sellToken 换得，或其市场价值是否合理。已有 receiver buyToken 余额不能拿来满足本次 minimum；sender 的余额也不能代替 receiver 到账，但 provider 直接付款、额外转账或回调期间的到账可以计入本次净增量。

### 历史余额与无关资产

sell/buy/native 的差额计算用于避免把历史余额自动当成本次退款或输出。对这些选定资产，在对应检查点要求 Router 余额不低于基线；这不是“保护 Router 全部资产”的承诺。

合约不枚举其他 token，也不探测 target 的 `balanceOf` 来拒绝 ERC20 入口。因此，第三种 token C 可以作为 target，被调用 `transfer` 或 `approve`。即使本次交易的最低到账达标，Router 中的 C 仍可能被转走或留下授权。清理逻辑只清理本次 sellToken 对本次 spender 的 allowance，不清理其他 token/地址组合。

这是当前明确接受的边界，测试 `testUnrelatedStoredTokenIsOutsideSwapGuarantees` 固定了该行为。不要依赖 Router 保存余额或恢复误转资产。

## 嵌套 AllowanceHolder

本合约不再强制“只要 target/spender 涉及 Holder，就必须同时为 Holder 且 selector 为 exec”的固定形状；也不审核内层 target/operator。

需要 Holder 转发的 provider 仍可使用以下常见路径，测试中的 MockSettler 覆盖了它：

```text
sender
  → Holder.exec(operator=Router, token=sellToken, amount=sellAmount, target=Router)
    → Router（拉取 sender 输入，给 Holder 精确授权）
      → Holder.exec(operator=下游执行者, token=sellToken, amount=内层额度, target=下游执行者)
        → 下游执行者（通过 Holder 从 Router 拉取输入）
```

此时 Router 的 `spender` 与 `target` 均为 Holder，`data` 是内层 exec 编码。外层临时权限的 owner 是 sender；内层 owner 是 Router，二者不是同一份权限。外层 Router 会清零对 Holder 的 ERC20 allowance，并继续执行退款及 minimum 检查。

Holder 的真实 sender 转发、operator/owner/token 临时权限隔离，以及其自身对危险调用的限制仍是独立的信任基础。删除 Router 的入口限制不等于删除 Holder 自身的限制。仅向 Holder 提供永久 ERC20 approval，不应让其他路由获得该钱包的临时执行权限；本地测试验证了 mock 中的这一隔离，但不证明任意部署的 Holder 都正确。

## 事件与错误

成功事件同时记录资金来源 sender 和买入收款地址 receiver。sender/sellToken/buyToken 保留 indexed，receiver 为非 indexed 字段：

```solidity
event Executed(
    address indexed sender,
    address receiver,
    address indexed sellToken,
    address indexed buyToken,
    uint256 sold,
    uint256 bought
);
```

`sold` 表示**扣除 Router 退还的同种输入资产后的净支出**，而不是传入的 sellAmount：

```solidity
// ERC20：sellRefund 是步骤 6 转回 sender 的 sellToken 数量。
// Native：sellRefund 是步骤 8 转回 sender 的 native 数量。
sold = sellRefund >= sellAmount ? 0 : sellAmount - sellRefund;
```

例如输入 100、Router 退回 10，则 sold 为 90。native 的退款包含没有转发给 target 的输入，以及 provider 退回 Router 的 native；native sold 不包含 gas。ERC20 swap 期间退还的 native 不抵扣 ERC20 sold。

额外到账可能使退款大于输入，此时 sold 记为 0，不因无符号减法下溢而回滚；多退部分仍会正常转给 sender，但 sold 不表达这部分净收益。没有新增“sold 必须大于零”的限制。

provider **直接退给 sender**、没有经过 Router 的 sellToken/native 不计入 sellRefund；后续回调中的钱包资产变化也不纳入此扣减。因此 sold 不是 sender 钱包完整净变化的保证。`bought` 仍等于返回的 boughtAmount，是最终 receiver 买入余额增量。两者统计口径不同，不能不加区分地用 bought / sold 当作实际成交价；sold 为 0 时也不能作除数。

计算复用现有 Router 余额和退款金额，不增加 sender 卖出余额查询或持久化存储，也不改变原有退款与最低到账规则。

| 自定义错误 | 触发条件 |
| --- | --- |
| `Unauthorized` | execute 的直接调用者不是配置的 Holder |
| `Reentrant` | execute 尚未结束时再次进入 |
| `InvalidInput` | Holder 构造参数没有代码，或 execute 的后缀总长度、sender、receiver、token、金额、target/spender、data/value 不满足约束 |
| `Expired` | block.timestamp 大于 deadline |
| `TokenCallFailed` | Holder 拉币返回 false；Router 的余额查询调用失败/返回长度不为 32；approve/transfer 调用失败、返回 false 或长度不合要求 |
| `BalanceInvariant` | ERC20 实收不等于 sellAmount，或检查时选定的 Router sell/buy/native 余额低于对应基线 |
| `InsufficientOutput` | 最终 receiver buyToken 余额下降，或增量低于 minBuyAmount |
| `NativeRefundFailed` | sender 拒收本次 native 退款 |

ERC20 操作通过 [IERC20](src/IERC20.sol) 和 `abi.encodeCall` 做编译期类型检查，不再手写 `encodeWithSignature` 字符串。执行层仍使用底层 call/staticcall，以保留 TokenCallFailed 错误边界和 approve/transfer 的空返回值兼容性；没有引入新的 Solidity 依赖。

这些不是全部可能的 revert：target 的失败原样冒泡，Holder 自身 revert 也会传播，算术溢出或非法 ABI 解码可直接失败。approve/transfer 接受空返回值或 ABI bool true；返回 false/非法结果会失败。`balanceOf` 必须返回恰好 32 字节。

不支持 fee-on-transfer/rebasing 等非标准余额语义。接口形状检查不能证明 token 诚实；恶意 token 伪造 balanceOf 或特殊转账逻辑不在通用安全保证内。

## 旧版本迁移

- 构造参数从 `(owner, allowanceHolder)` 改为 `(allowanceHolder)`。
- 删除 owner/pendingOwner/paused/allowed 查询及 setPaused、setAllowed、transferOwnership、acceptOwnership、recoverToken；对应管理事件和错误也不再存在。
- execute 当前参数为 `sellToken, buyToken, receiver, sellAmount, minBuyAmount, deadline, spender, target, value, data`。相比上一版，在 buyToken 后增加 receiver，selector 从 `0x0ed3c45c` 改为 `0xf509c3a5`，ABI head 从 9 个 word 变为 10 个；不能复用旧 calldata。更早版本还需迁移 deadline/spender/target 的顺序。原自收款调用需显式传入 receiver = sender。
- 外层 Holder exec 的接口形状不变，但其 data 中的 Router 调用必须使用新 ABI 重新编码；target 和 spender 都是 address 类型，误交换不会被类型检查自动发现。
- 原调用者命名统一为 sender。Executed 新增第二个 address 字段 receiver，事件签名变为 `Executed(address,address,address,address,uint256,uint256)`，topic0 与 data 布局改变；事件消费者需要更新 ABI，不能用旧结构解码。
- sold 沿用“扣除 Router 退给 sender 的同种资产退款、最低为零的净支出”口径，bought 改为 receiver 的最终买入余额增量。execute 仍只返回 uint256 boughtAmount；按部署版本区分历史事件语义。
- 此合约不能原地升级旧部署；若已有旧地址，需要单独部署并更新引用。本次文档更新没有执行部署。

本说明只确认 Solidity 实现，不表示 Rust、部署脚本或其他客户端已适配。集成方应独立更新 ABI/构造参数、移除旧管理调用并检查自身链下规则；不要把旧文档中的管理员白名单、pause/recover 或“execute ABI 不变”描述当作当前合约行为。仓库其他产品/技术及历史验收记录中的旧合约描述不覆盖本文。

## 本地验证与未验证范围

从仓库根目录运行：

```sh
forge fmt --root contracts --check
forge build --root contracts --deny-warnings
forge test --root contracts
forge inspect --root contracts MetaRouter methodIdentifiers
```

当前 Foundry 套件为 **50 项测试**，包含 5 项 fuzz、每项 256 runs。覆盖新 ABI selector、独立 spender、路由免登记、Holder 身份/临时权限、金额与 native value、输入额度上限、授权清理、退款和历史余额、最终最低到账、provider/native 退款回调重入、嵌套 Holder、已移除的管理接口及无关 token 不受保护。Executed 事件断言覆盖 ERC20/native 的净支出、无退款/全额退款/额外退款、native 未转发输入、ERC20 不扣 native 退款、直接向 sender 退款不计入 Router 口径，以及带历史余额的 fuzz 场景。

receiver 覆盖自收款及独立收款、Router 中转/直接 provider 付款、错误收款地址、历史余额与 minimum fuzz、非法 receiver、输入退款仍归 sender、嵌套 Holder、sender 退款回调后的最终 receiver 检查及事件双方地址。IERC20 回归覆盖无返回值 approve/transfer、false 返回值和畸形返回数据。

所有 Holder/provider 均为测试替身；这些测试不是正式 Holder/provider 的主网 fork 或外部审计，也不证明生产集成、真实钱包或实际市场成交。本轮不修改或运行 Rust。上线前仍需正式 Holder/provider 兼容性测试、调用方 ABI 适配验证及外部安全审计；没有管理员暂停作为上线后的补救手段。
