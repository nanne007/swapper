---
name: metamatch-test-verification
description: 'Design or review MetaMatch Rust tests, provider/RPC fixtures, Foundry fuzz tests, Anvil E2E, CI gates, and evidence boundaries.'
---

# MetaMatch test verification

Read `AGENTS.md`, `docs/PRODUCT.md`, `docs/TECHNICAL.md`, `docs/VERIFICATION.md`, and the relevant Rust implementation before editing tests. Test the contract and evidence boundary, not merely that a mock was called.

## Verification layers

- Rust unit tests cover domain arithmetic, ranking, provider isolation, HTTP, total timeout/capacity/cancellation, RPC errors, executable transaction invariants, and public envelopes.
- Provider fixtures verify URL, headers, request fields, normalization, malformed/oversized responses, timeout, rate limit, and unexpected execution addresses. They are not live supplier evidence.
- RPC fixtures verify per-provider `eth_simulateV1(latest)`, block context from the simulation response, absence of context/gas-price RPCs and call `gasPrice`, call ordering, typed `eth_createAccessList` mapping discovery, revert/unsupported handling, direct sell/native funding overrides, fixed ERC20 approval without allowance pre-read, absence of real-balance reads, and null fee behavior.
- Foundry tests cover the current permissionless Router: Holder caller/sender authentication, current target/spender structure, input amount/value, token returns and approval cleanup, receiver minimum, net sold/refunds, sell/buy balance isolation and reentrancy. Do not reintroduce retired allowlist/admin/pause/recovery behavior. Add fuzzing for numeric balance/refund/minimum invariants.
- Local Anvil E2E covers the HTTP → simulation → approval → same returned local swap path. It does not prove formal Holder bytecode, provider behavior, deployed Router safety, or production fee data.

## Workflow

1. Before changing a test, identify the externally observable behavior or invariant and the evidence tier: fixture, local EVM, fork, or live integration.
2. Reproduce a failure with the narrowest test. Add a regression assertion before broadening the implementation.
3. Avoid snapshot-only assertions, timing sleeps, over-mocked tests, fabricated provider output, and global skips. Keep test data free of keys, cookies, raw provider bodies, and wallet material.
4. Run the narrowest relevant command, then `sh scripts/check.sh` for the complete project gates, including the isolated Anvil E2E. Keep the command sequence in that shared script rather than copying it here.
5. Update `docs/VERIFICATION.md` when the evidence boundary or an unverified production assumption changes.

Do not call a green fixture, local mock, or Anvil test a live provider/mainnet validation. Do not weaken an assertion just to preserve a green test.
