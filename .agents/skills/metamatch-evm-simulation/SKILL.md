---
name: metamatch-evm-simulation
description: 'Implement or review MetaMatch Alloy and JSON-RPC simulation semantics. Use when changing src/rpc.rs, src/simulation.rs, execution encoding, latest block simulation, eth_simulateV1, state overrides, gas/fee calculation, or chain capability reporting.'
---

# MetaMatch EVM simulation

Read `AGENTS.md`, `docs/TECHNICAL.md`, `docs/VERIFICATION.md`, `src/rpc.rs`, `src/simulation.rs`, and `src/execution.rs` before editing. This is a low-freedom workflow: preserve the exact block context returned by each simulation and never present it as a future execution guarantee.

## Simulation contract

- Each provider owns one `route -> simulate` pipeline. Simulate with the explicit `latest` tag and derive the u64 block number, block hash, and timestamp from the returned `SimulatedBlock`; providers in the same competition may observe different latest blocks. Do not fetch a shared context or perform pre/post block probes.
- Do not query gas price and do not set `gasPrice` or block overrides on simulation calls. Report `gasFeeWei: null`; derive only gas used. Validate response IDs, block/call shape, call count, status, return data, and response size. Distinguish `success`, `reverted`, `unsupported`, and `error`.
- Do not pre-read wallet allowance. For every ERC20 sell, return and simulate exactly one `approve(Holder, sellAmount)`; native sells have no token approval. Use `eth_simulateV1` only for the reviewed sequential sequence: pre-balance, fixed ERC20 approval when applicable, swap, post-balance. A token that rejects the approval must fail simulation explicitly.
- State overrides are assumptions, not facts. `BalanceSlots::resolve` only returns a mapping base from configuration/cache or parallel typed `eth_createAccessList` calls for two fixed fake owners; each provider invokes it with `latest`, while successful concurrent discovery is coalesced by chain/token. Automatic detection is bounded to 0..1023; arbitrary uint256 bases require configuration. Simulation derives the real owner's storage key, overrides the sell-token balance, and sets native balance to `U256::MAX` so node-selected latest fees cannot fail the upfront gas check. Do not read real balances, run a separate override validation call, or invalidate a cached base because a later simulation failed.
- Derive bought output from the taker's balance delta. Sum gas from approval and swap calls only. Do not infer a fee without a gas price.
- Public competitions require the real taker for transaction and recipient binding, and always use overridden funding. Only return the exact approval/swap transactions that passed the full simulation. Report `funding: overridden` so callers do not treat success as proof of current wallet funds. Report each simulated block number/hash/timestamp; freshness is the caller's decision, not a service TTL.
- Routing is permissionless: no Rules or target/spender/selector allowlist. Keep provider-specific target, spender, selector, calldata, and value checks in `src/providers/`/`src/execution.rs`; do not make simulation a generic arbitrary-call executor.

## Change workflow

1. State the block, call sequence, funding mode, and accounting invariant the change preserves.
2. Add fixture tests for `latest`, absence of context/gas-price RPCs and call `gasPrice`, malformed/revert/unsupported responses, approval return values, balance deltas, gas used, and direct funding override construction.
3. For contract/envelope changes, run focused Rust tests plus `forge fmt --root contracts`, `forge build --root contracts --deny-warnings`, `forge test --root contracts`, and `cargo test --test e2e_local -- --ignored --nocapture`.

No live provider, production RPC, deployed Router, or mainnet fork is implied by a fixture or local Anvil pass. Report those boundaries explicitly.
