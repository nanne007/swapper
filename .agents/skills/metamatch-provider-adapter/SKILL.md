---
name: metamatch-provider-adapter
description: 'Implement or review a MetaMatch quote-provider adapter for 0x, 1inch Classic, KyberSwap, or a newly approved EVM supplier. Use when changing src/providers/, provider schemas, upstream request fields, route normalization, provider timeout behavior, or provider fixture tests.'
---

# MetaMatch provider adapter

Read `AGENTS.md`, `docs/SOURCES.md`, and the current provider implementation before editing. Re-check the supplier's official documentation when endpoint or field behavior may have changed; do not infer undocumented execution semantics from a frontend request alone.

## Implement

1. Keep the `Provider` interface and `Route` model stable. Normalize all amounts as decimal strings and validate with Rust serde/domain checks before any route reaches simulation or transaction encoding.
2. Use only fixed, server-configured official URLs. Never accept a user URL, provider target, spender, selector, recipient, or RPC endpoint as configuration at request time.
3. The API requires a real taker. Production competition passes the configured MetaRouter as provider sender; preserve each adapter's recipient binding and simulate the full Holder/Router path with taker as sender/receiver. There is no preview account, direct-provider fallback or second build/requote stage. Fixed fixture addresses belong only in tests.
4. Enforce sell amount equality, buy amount positivity, native/ERC20 value rules, transaction `to` and calldata shape, expected spender, and any upstream-native deadline. Do not invent a route TTL; preserve provider-specific request/calldata deadlines. Reject unexpected target/spender changes with a safe typed error.
5. Treat provider state overrides as untrusted off-chain assumptions. Reject non-empty override payloads unless the project has a separately reviewed, fork-verified implementation for that provider and token.
6. Do not turn missing credentials, rate limits, malformed JSON, timeouts, or unsupported fields into fabricated quotes. Return `unavailable`, `error`, or `unsupported` while allowing other providers to finish.

## Test

Add or update Rust fixture tests under `tests/` for URL/query or body, authentication headers without real secrets, amount and recipient fields, response normalization, malformed responses, unexpected execution addresses, rate limits, and oversized/invalid upstream responses. Keep production provider modules free of test code. A fixture test is not live-provider validation; state that boundary in the handoff.

Run `cargo fmt`, then `cargo clippy`, and the focused provider tests before the full Cargo and Foundry gates. Do not spend production API quota or broadcast a transaction. Update the Rust serde/domain types and product/technical docs when a public route field or capability changes.
