---
name: metamatch-evm-simulation
description: 'Implement or review MetaMatch Alloy and JSON-RPC simulation semantics. Use when changing src/rpc.rs, src/simulation.rs, execution encoding, fixed block context, eth_call, eth_simulateV1, state overrides, gas/fee calculation, reorg checks, or chain capability reporting.'
---

# MetaMatch EVM simulation

Read `AGENTS.md`, `docs/TECHNICAL.md`, `docs/VERIFICATION.md`, `src/rpc.rs`, `src/simulation.rs`, and `src/execution.rs` before editing. This is a low-freedom workflow: simulation evidence must be reproducible at one block context and must never be presented as a future execution guarantee.

## Simulation contract

- Resolve and preserve one parent `blockNumber`, `blockHash`, `timestamp`, and gas price for a competition. Check the parent hash before and after simulation; a reorg is a failed verification, not a retry with a new context.
- Validate `eth_chainId`, block shape, response IDs, hex quantities, call count, status, return data, and response size. Distinguish `success`, `reverted`, `unsupported`, and `error`.
- Use Alloy-backed `eth_call` to read balances and allowances at the fixed block. Use `eth_simulateV1` only for the reviewed sequential sequence: pre-balance, required approval(s), swap, post-balance.
- State overrides are assumptions, not facts. Only use a configured token balance slot after validating the overridden `balanceOf` response. Never guess slots, override arbitrary storage, or use overrides to bypass actual build funding checks.
- Derive bought output from the taker's balance delta. Sum gas from approval and swap calls only. Ethereum fee support may produce a value; unsupported fee models remain explicit errors.
- Preview may use controlled funding overrides. Build must use the real taker's balance and only return unsigned approval/swap transactions after final re-quote and simulation.
- Keep provider target, spender, selector, calldata, and value checks in `src/providers.rs`/`src/execution.rs`; do not make simulation a generic arbitrary-call executor.

## Change workflow

1. State the block, call sequence, funding mode, and accounting invariant the change preserves.
2. Add fixture tests for RPC method order, malformed/revert/unsupported responses, reorg, approval return values, balance deltas, gas, and override validation.
3. For contract/envelope changes, run focused Rust tests plus `forge fmt --root contracts`, `forge build --root contracts --deny-warnings`, `forge test --root contracts`, and `cargo test --test e2e_local -- --ignored --nocapture`.

No live provider, production RPC, deployed Router, or mainnet fork is implied by a fixture or local Anvil pass. Report those boundaries explicitly.
