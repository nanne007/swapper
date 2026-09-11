# MetaMatch coding-agent instructions

## Repository intent

MetaMatch is a non-custodial EVM quote competition backend. It compares 0x, 1inch Classic, and KyberSwap routes, simulates them against a fixed block context, ranks only verified net outputs, and returns unsigned transactions for the user's wallet. The service never stores private keys, signs, broadcasts, or silently changes a user's minimum output.

The backend runtime is the Rust Cargo crate at the repository root (`Cargo.toml`, `src/*.rs`).

## Before editing

1. Run `git status --short` and preserve unrelated user changes.
2. Read `docs/PRODUCT.md`, `docs/TECHNICAL.md`, and `docs/VERIFICATION.md` when changing behavior.
3. Keep edits scoped to the request. Do not deploy contracts, contact production RPCs, spend API quota, commit, push, or change remote settings unless the user explicitly authorizes that exact action.
4. Never place API keys, RPC credentials, wallet material, cookies, or provider response bodies in source, tests, fixtures, logs, or documentation.

## Architecture boundaries

- `src/domain.rs`: validated wire/domain types and `U256` integer arithmetic. Token quantities are decimal strings; never use floating point for amounts, prices, gas, or comparisons.
- `src/providers.rs`: fixed official provider endpoints only. Normalize provider responses and reject unexpected target, spender, selector, amount, value, or unsupported state overrides.
- `src/rpc.rs` and `src/simulation.rs`: server-configured RPC only. Preserve block number/hash, distinguish success/revert/unsupported/error, and use state overrides only after an `eth_call` validation or an explicitly fork-verified slot.
- `src/execution.rs` and `contracts/src/MetaRouter.sol`: enforce the AllowanceHolder/MetaRouter boundary. Keep exact spend, exact temporary allowance, route tuple allowlist, actual balance-delta minimum, refund-delta isolation, pause, and reentrancy protections.
- `src/competitions.rs`: isolate provider failures, enforce timeout/TTL/capacity, bind the real taker during build, re-quote and re-simulate, and reject any accepted minimum downgrade.
- `src/app.rs`: public API, bearer competition token, polling snapshots, safe error responses, and bounded request bodies. Do not expose upstream secrets or raw error bodies.

## Required quality gates

Use the narrowest relevant command while iterating, then run the full gate before handoff:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --release
forge fmt --root contracts --check
forge build --root contracts --deny-warnings
forge test --root contracts
cargo test --test e2e_local -- --ignored --nocapture
```

Run `cargo fmt` for Rust and inspect the diff afterwards. Never format generated `target` or Foundry output. `forge fmt` is the source of truth for Solidity formatting; do not introduce a second Solidity formatter.

## Project skills

Reusable task-specific skills live under `.agents/skills/`:

- `metamatch-domain-modeling`: competition, quote, simulation, build, ranking, TTL, and public state semantics.
- `metamatch-provider-adapter`: provider endpoint/schema/normalization and Rust fixture-contract workflow.
- `metamatch-evm-simulation`: Alloy/JSON-RPC fixed-block context, `eth_call`, `eth_simulateV1`, overrides, reorgs, and fees.
- `metamatch-contract-safety`: MetaRouter, AllowanceHolder, token, refund, minimum-output, and Foundry safety workflow.
- `metamatch-test-verification`: Rust, provider/RPC fixtures, Foundry/fuzz, Anvil E2E, and evidence boundaries.
- `metamatch-quality-gates`: Cargo formatter, Clippy, tests, release build, Foundry, E2E, and CI workflow.

Use the smallest matching skill when a task fits one of these domains. For Rust runtime work, apply the same domain, HTTP, provider, EVM, test and quality boundaries to the corresponding `.rs` modules. Skills complement this file; they do not grant permission to deploy, broadcast, commit, push, spend production quota, or access secrets.

## Change-specific requirements

- New public fields require updates to Rust serde/domain types, tests, and the relevant product/technical documentation.
- New provider integrations must have a fixture contract test for URL, headers, body, response normalization, malformed responses, timeout, and unexpected execution addresses. Do not make an unverified provider response executable by default.
- New chain/token support requires explicit addresses, decimals, fee model, simulation behavior, and tests. Ethereum is the only supported chain in the current product scope.
- Contract changes require Foundry tests for caller authorization, target/spender/selector allowlist, exact amount/value, token return values, reentrancy, historical balances, minimum output, refunds, and any nested Holder semantics.
- Test fixtures must stay visibly test-only and non-executable. Missing RPC/key/router/fee support must be `unavailable`, `unsupported`, or an explicit error; never fill it with fabricated quotes or partial fee estimates.

## Security and operations

Never approve a Settler or arbitrary spender. Never accept a user-supplied URL, calldata target, state slot, or RPC endpoint. Do not broaden the route allowlist as a workaround for a failing test. Keep temporary data bounded and errors sanitized. Production deployment, multisig administration, audits, real-provider E2E, and mainnet fork verification are separate release gates, not implied by a green local test.

## Handoff format

Report changed files, observable behavior, exact verification commands/results, assumptions, and remaining unverified boundaries. If a command cannot run because a tool or external service is unavailable, say so explicitly; do not downgrade the gate or call a fixture a live integration test.
