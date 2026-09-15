---
name: metamatch-domain-modeling
description: 'Model or review MetaMatch competition, quote, simulation, transactions, and verification states. Use when changing src/domain.rs, src/competitions.rs, public response semantics, ranking, total deadline, taker binding, minimum output, or product acceptance behavior.'
---

# MetaMatch domain modeling

Read `AGENTS.md`, `docs/PRODUCT.md`, `docs/TECHNICAL.md`, and `docs/VERIFICATION.md` before changing behavior. Treat this skill as the semantic boundary between a provider quote, a verified simulation, and executable transactions.

## State and invariants

- A competition is bounded by capacity and one total request deadline. Each provider independently executes `route -> validate -> simulate(latest)` and starts simulation as soon as its own route is ready. A malformed or slow route stays local to its provider and never delays another provider's ready simulation or becomes a fabricated fallback.
- `Quote` contains only required Route, SimulationSuccess, approvals, transaction and latency. Provider failures live in a separate failures array; never add error/status or optional success fields to Quote.
- Rank successful simulated bought balance deltas as U256 integers, not raw provider buyAmount. Funding is always explicitly overridden and is not proof of wallet funds. Report gas used separately; gas fee is null when simulation does not query or set gas price. Block context number and timestamps are JSON u64; the hash remains a hex string. No API expiresAt or artificial TTL. The caller decides freshness from each simulation's returned block context.
- Require the real taker at input. Simulate the full approval/swap sequence and return exactly those transactions. Preserve on-chain minimum output and upstream native deadlines.
- Token quantities, gas, fees, prices, and ranking comparisons use decimal strings plus `U256`; never use floating point or silently assume USDC is worth one dollar.
- `unavailable`, `unsupported`, `error`, and `reverted` retain distinct meanings in the wire response and handoff. Do not turn missing RPC, credentials, fee models, or Router deployment into success.

## Change workflow

1. Describe the state transition and the invariant that should remain true before editing.
2. Reuse existing `Input`, `Route`, `SimulationSuccess`, `Quote`, and `ErrorKind` types. Add a public field only with Rust serde/domain types, tests, and product/technical documentation updates.
3. Test externally observable behavior: ranking, total timeout/cancellation, provider isolation, taker binding, minimum output, block context, and fee-unknown behavior.
4. Keep protocol/product claims separate from local fixture evidence and unverified live boundaries.

Do not add cross-chain intents, native-buy support, platform fees, persistent storage, or optimistic fallbacks as a “domain cleanup” unless the product scope explicitly changes.
