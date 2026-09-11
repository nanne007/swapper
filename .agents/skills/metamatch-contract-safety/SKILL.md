---
name: metamatch-contract-safety
description: 'Implement or review MetaMatch Solidity execution and AllowanceHolder integration. Use when changing contracts/src/MetaRouter.sol, Foundry tests, route allowlists, nested Holder calldata, token approvals, refunds, minimum output, reentrancy, or EVM execution boundaries.'
---

# MetaMatch contract safety

Read `AGENTS.md`, `docs/TECHNICAL.md`, `docs/VERIFICATION.md`, `contracts/README.md`, and the current contract before editing. This is a security-sensitive low-freedom skill: preserve existing user funds and caller boundaries, and stop if a requested change conflicts with them.

## Invariants

- Only the configured AllowanceHolder may call `MetaRouter.execute`; derive the real taker only from the Holder's verified forwarded-sender suffix.
- Allow only explicitly configured `(target, spender, selector)` tuples. Keep the narrowly reviewed nested Holder `exec` case separate from arbitrary external calls.
- Pull exactly the requested sell amount. Require native `msg.value`/call value rules, reject input-tax shortfalls, and clear ERC20 allowance to zero after the external call.
- Measure the taker's buy-token balance delta and enforce `minBuyAmount`; do not use the taker's historical balance to satisfy the current trade.
- Refund only the current transaction's native and token deltas. Preserve historical balances and reject false ERC20 return values, overspending, and reentrancy.
- Do not add rescue, arbitrary delegatecall, arbitrary admin call, Settler approval, or user-controlled target/slot/RPC behavior as a workaround.

## Workflow

1. Characterize the behavior with a focused Foundry test, including adversarial caller, value, token return, balance, and callback cases.
2. Make the smallest contract change. Keep the compiler version and Foundry profile aligned with `contracts/foundry.toml`.
3. Add fuzz coverage when an amount, balance, refund, or calldata-length invariant has a meaningful numeric domain.
4. Run `forge fmt --root contracts`, `forge build --root contracts --deny-warnings`, and `forge test --root contracts`. Then run `cargo test --test e2e_local -- --ignored --nocapture` to verify the Rust envelope still matches the ABI.
5. Treat a local mock Holder or Anvil run as local evidence only. A deployed-address or mainnet Holder change requires an explicitly authorized fork test and later external audit; never deploy or broadcast from this skill.

Report gas/behavior changes, the exact invariant tested, and any unverified formal Holder or provider compatibility boundary.
