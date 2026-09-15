---
name: metamatch-evm-simulation
description: 'Implement or review MetaMatch Alloy and JSON-RPC simulation semantics. Use when changing src/rpc.rs, src/simulation.rs, execution encoding, fixed block context, eth_call, eth_simulateV1, state overrides, gas/fee calculation, reorg checks, or chain capability reporting.'
---

# MetaMatch EVM simulation

Read `AGENTS.md`, `docs/TECHNICAL.md`, `docs/VERIFICATION.md`, `src/rpc.rs`, `src/simulation.rs`, and `src/execution.rs` before editing. This is a low-freedom workflow: simulation evidence must be reproducible at one block context and must never be presented as a future execution guarantee.

## Simulation contract

- Resolve and preserve one parent `blockNumber`, `blockHash`, `timestamp`, and gas price for a competition. Check the parent hash before and after simulation; a reorg is a failed verification, not a retry with a new context.
- Validate `eth_chainId`, block shape, response IDs, hex quantities, call count, status, return data, and response size. Distinguish `success`, `reverted`, `unsupported`, and `error`.
- Use Alloy-backed `eth_call` to read allowance at the fixed block. Use `eth_simulateV1` only for the reviewed sequential sequence: pre-balance, required approval(s), swap, post-balance.
- State overrides are assumptions, not facts. `BalanceSlots::resolve` only returns a mapping base from configuration/cache or parallel typed `eth_createAccessList` calls for two fixed fake owners; it accepts no real owner/amount, constructs no StateOverride and can run independently before simulation. Cache bases by chain/token. Automatic detection is bounded to 0..1023; arbitrary uint256 bases require configuration. Simulation derives the real owner's storage key and directly overrides the sell-token balance plus native transaction/gas funding. Do not read real balances, run a separate override validation call, or invalidate a cached base because a later simulation failed.
- Derive bought output from the taker's balance delta. Sum gas from approval and swap calls only. Ethereum fee support may produce a value; unsupported fee models remain explicit errors.
- Public competitions require the real taker for transaction and recipient binding, and always use overridden funding. Only return the exact approval/swap transactions that passed the full simulation. Report `funding: overridden` so callers do not treat success as proof of current wallet funds. Report parent number/hash/timestamp and simulated execution timestamp separately; freshness is the caller's decision, not a service TTL.
- Keep provider target, spender, selector, calldata, and value checks in `src/providers/`/`src/execution.rs`; do not make simulation a generic arbitrary-call executor.

## Change workflow

1. State the block, call sequence, funding mode, and accounting invariant the change preserves.
2. Add fixture tests for RPC method order, malformed/revert/unsupported responses, reorg, approval return values, balance deltas, gas, and direct funding override construction.
3. For contract/envelope changes, run focused Rust tests plus `forge fmt --root contracts`, `forge build --root contracts --deny-warnings`, `forge test --root contracts`, and `cargo test --test e2e_local -- --ignored --nocapture`.

No live provider, production RPC, deployed Router, or mainnet fork is implied by a fixture or local Anvil pass. Report those boundaries explicitly.
