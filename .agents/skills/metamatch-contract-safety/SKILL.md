---
name: metamatch-contract-safety
description: 'Implement or review MetaMatch Solidity execution and AllowanceHolder integration. Use when changing contracts/src/MetaRouter.sol, Foundry tests, permissionless routes, nested Holder calldata, receiver, approvals, refunds, minimum output, or reentrancy.'
---

# MetaMatch contract safety

Read `AGENTS.md`, `docs/TECHNICAL.md`, `docs/VERIFICATION.md`, `contracts/README.md`, and the current contract before editing. This is a security-sensitive low-freedom skill: preserve existing user funds and caller boundaries, and stop if a requested change conflicts with them.

## Invariants

- Only the configured AllowanceHolder may call `MetaRouter.execute`; derive the real sender only from its forwarded-sender suffix, preserving calldata-length and sender checks. The Holder implementation must be trusted independently.
- Rust and Solidity are permissionless: no admin, pause, allowlist, recovery, ownership transfer, or Rule. Do not reintroduce them implicitly. Preserve the current contract's target/spender checks, including code presence, Router self-call rejection, and exclusion of the selected sell/buy tokens as target. Other ERC20 targets and arbitrary nested Holder shapes are not categorically rejected. Neither outer nor inner route identity is audited by the Router.
- Pull exactly the requested sell amount. Require native `msg.value`/call value rules, reject input-tax shortfalls, and clear ERC20 allowance to zero after the external call.
- Measure the receiver's final buy-token balance delta after transfers and refund callbacks and enforce `minBuyAmount`. Do not count its historical balance toward output or substitute sender's balance when receiver differs.
- Refund current sell-token/native deltas to sender; transfer the Router's buy-token delta to receiver. Preserve the selected assets' baseline checks, false/malformed ERC20 return rejection, input limit and reentrancy protection. Unrelated idle assets are explicitly outside the guarantees and have no recovery promise.
- `Executed.sold` deducts only same-asset refunds made through Router, floored at zero; direct provider refunds to sender and gas are excluded. Partial or zero net input consumption remains allowed when minimum output and other checks pass.
- Keep IERC20/abi.encodeCall with the existing low-level return-value handling. Do not add arbitrary delegatecall/admin-call helpers or client-controlled RPC/slot inputs. For 0x nested routes approve Holder rather than Settler. Exact ABI, receiver exclusions and accounting examples live in `contracts/README.md`; update that source alongside behavior changes instead of copying the ABI here.

## Workflow

1. Characterize the behavior with a focused Foundry test, including adversarial Holder/sender, receiver, target/spender, native value, token returns, minimum output, refunds and callback cases. Preserve the third-token boundary test and absence of removed management interfaces.
2. Make the smallest contract change. Keep the compiler version and Foundry profile aligned with `contracts/foundry.toml`.
3. Add fuzz coverage when an amount, balance, refund, or calldata-length invariant has a meaningful numeric domain.
4. Run `forge fmt --root contracts`, `forge build --root contracts --deny-warnings`, and `forge test --root contracts`. Then run `cargo test --test e2e_local -- --ignored --nocapture` to verify the Rust envelope still matches the ABI.
5. Treat a local mock Holder or Anvil run as local evidence only. A deployed-address or mainnet Holder change requires an explicitly authorized fork test and later external audit; never deploy or broadcast from this skill.

Report gas/behavior changes, the exact invariant tested, and any unverified formal Holder or provider compatibility boundary.
