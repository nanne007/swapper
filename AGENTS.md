# MetaMatch coding-agent instructions

## Repository intent

MetaMatch is a non-custodial multi-chain EVM quote competition backend. It compares the configured-capable subset of 13 provider adapters, simulates them against a fixed block context with explicit funding overrides, ranks only simulated balance deltas, and returns unsigned transactions for the user's wallet. The service never stores private keys, signs, broadcasts, or silently changes a user's minimum output.

The backend runtime is the Rust Cargo crate at the repository root (`Cargo.toml`, `src/*.rs`).

## Before editing

1. Run `git status --short` and preserve unrelated user changes.
2. Read `docs/PRODUCT.md`, `docs/TECHNICAL.md`, and `docs/VERIFICATION.md` when changing behavior.
3. Keep edits scoped to the request. Do not deploy contracts, contact production RPCs, spend API quota, commit, push, or change remote settings unless the user explicitly authorizes that exact action.
4. Never place API keys, RPC credentials, wallet material, cookies, or provider response bodies in source, tests, fixtures, logs, or documentation.

## Architecture boundaries

- `src/domain.rs`: validated wire/domain types and `U256` integer arithmetic. Token quantities are decimal strings; never use floating point for amounts, prices, gas, or comparisons.
- `src/providers/`: one production module per provider; `mod.rs` owns the stable trait, registry, shared DTO/helpers, and route normalization. Fixed official provider endpoints only. Normalize provider responses and reject unexpected target, spender, selector, amount, value, or unsupported state overrides.
- `src/rpc.rs` and `src/simulation.rs`: server-configured RPC only. Preserve block number/hash and distinguish success/revert/unsupported/error. Every simulation overrides the taker's sell-token and native balances; resolve ERC20 mapping bases from configuration/cache or two fake-owner `eth_createAccessList` calls, without reading real balances or validating the override through a separate `eth_call`.
- `src/execution.rs` and `contracts/src/MetaRouter.sol`: enforce the AllowanceHolder/MetaRouter boundary. Rust and Solidity use permissionless routing, with no Rule/allowlist, admin, pause, recovery or ownership transfer. Preserve provider-specific protocol checks, input limits, exact temporary allowance and cleanup, receiver balance-delta minimum, refund-delta isolation and reentrancy protection. Unrelated idle assets are outside the Router's protection promise. Encoding requires a validated route and an explicit Router/Holder deployment; no direct-provider fallback.
- `src/competitions.rs`: isolate provider failures, enforce one total deadline and capacity, bind the real taker at input, and return the exact approval/swap transactions simulated; no stored competitions or artificial quote TTL.
- `src/app.rs`: single-request public competition API, safe error responses, and bounded request bodies. Do not expose upstream secrets or raw error bodies.

## Required quality gates

Use the narrowest relevant command while iterating, then run the full gate before handoff:

```sh
sh scripts/check.sh
```

`scripts/check.sh` is the single source of truth for the full local/CI sequence, including the isolated Anvil E2E. It does not enable production provider/RPC tests.

Run `cargo fmt` for Rust and inspect the diff afterwards. Never format generated `target` or Foundry output. `forge fmt` is the source of truth for Solidity formatting; do not introduce a second Solidity formatter.

## Project skills

Reusable task-specific skills live under `.agents/skills/`:

- `metamatch-domain-modeling`: competition, quote, simulation, executable results, ranking, total deadline, and public state semantics.
- `metamatch-provider-adapter`: provider endpoint/schema/normalization and Rust fixture-contract workflow.
- `metamatch-evm-simulation`: Alloy/JSON-RPC fixed-block context, `eth_call`, `eth_simulateV1`, overrides, reorgs, and fees.
- `metamatch-contract-safety`: MetaRouter, AllowanceHolder, token, refund, minimum-output, and Foundry safety workflow.
- `metamatch-test-verification`: Rust, provider/RPC fixtures, Foundry/fuzz, Anvil E2E, and evidence boundaries.
- `metamatch-quality-gates`: Cargo formatter, Clippy, tests, release build, Foundry, E2E, and CI workflow.

Use the smallest matching skill when a task fits one of these domains. For Rust runtime work, apply the same domain, HTTP, provider, EVM, test and quality boundaries to the corresponding `.rs` modules. Skills complement this file; they do not grant permission to deploy, broadcast, commit, push, spend production quota, or access secrets.

## Change-specific requirements

- New public fields require updates to Rust serde/domain types, tests, and the relevant product/technical documentation.
- New provider integrations must have a fixture contract test for URL, headers, body, response normalization, malformed responses, timeout, and unexpected execution addresses. Do not make an unverified provider response executable by default.
- New chain/provider support requires an official capability source, chain ID/slug, RPC and simulation behavior, and tests. v1 derives its 17-chain catalog from the 13-provider matrix; it does not add a token registry.
- Production code contains no test modules. All Rust tests, fixtures, mocks, and integration harnesses live under `tests/`; `src/` must remain free of `#[cfg(test)]` and test functions.
- Contract changes require Foundry tests for Holder caller/sender authentication, permissionless routing, current target/spender structure, amount/value, token return values, reentrancy, sell/buy balance deltas, receiver minimum, net sold and refunds. Do not reintroduce deleted admin/allowlist/recovery features.
- Test fixtures must stay visibly test-only and non-executable. Missing RPC/key/router/fee support must be `unavailable`, `unsupported`, or an explicit error; never fill it with fabricated quotes or partial fee estimates.

## Security and operations

Wallet approvals use Holder; 0x nested routes approve Holder, not Settler. Router provider approvals are exact and cleared after execution. The HTTP API accepts swap inputs, not user-supplied calldata, URLs, state slots, or RPC endpoints. MetaRouter is permissionless but still enforces its structural and balance checks. Rust must verify provider protocol constraints and simulate the full Holder/Router path before returning executable transactions. Neither layer protects unrelated idle assets. Keep temporary data bounded and errors sanitized. Production deployment, audits, real-provider E2E, and mainnet fork verification are separate release gates, not implied by a green local test.

## Handoff format

Report changed files, observable behavior, exact verification commands/results, assumptions, and remaining unverified boundaries. If a command cannot run because a tool or external service is unavailable, say so explicitly; do not downgrade the gate or call a fixture a live integration test.
