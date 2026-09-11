---
name: metamatch-domain-modeling
description: 'Model or review MetaMatch competition, quote, simulation, build, and verification states. Use when changing src/domain.rs, src/competitions.rs, public response semantics, ranking, TTL, taker binding, minimum output, or product acceptance behavior.'
---

# MetaMatch domain modeling

Read `AGENTS.md`, `docs/PRODUCT.md`, `docs/TECHNICAL.md`, and `docs/VERIFICATION.md` before changing behavior. Treat this skill as the semantic boundary between a provider quote, a verified simulation, and an executable build.

## State and invariants

- A competition is bounded by capacity and TTL. Provider work is isolated; one timeout or malformed response must not cancel valid competitors or become a fabricated fallback.
- A `Quote` may be `ready`, `unavailable`, or `error`; execution remains either `direct-preview` or `unified`.
- Only a successful, non-expired simulation with a known net output is verified for ranking. Raw provider `buyAmount`, expired output, and unknown-fee output must not outrank verified net output.
- Preview uses the configured non-zero preview taker. Build binds the real taker, re-quotes, re-simulates, and never lowers the accepted minimum output.
- Token quantities, gas, fees, prices, and ranking comparisons use decimal strings plus `U256`; never use floating point or silently assume USDC is worth one dollar.
- `unavailable`, `unsupported`, `error`, and `reverted` retain distinct meanings in the wire response and handoff. Do not turn missing RPC, credentials, fee models, or Router deployment into success.

## Change workflow

1. Describe the state transition and the invariant that should remain true before editing.
2. Reuse existing `Input`, `Route`, `Simulation`, `Quote`, and `Fault` types. Add a public field only with Rust serde/domain types, tests, and product/technical documentation updates.
3. Test externally observable behavior: ranking, expiry, provider isolation, taker binding, monotonic minimum output, and fee-unknown behavior.
4. Keep protocol/product claims separate from local fixture evidence and unverified live boundaries.

Do not add cross-chain intents, native-buy support, platform fees, persistent storage, or optimistic fallbacks as a “domain cleanup” unless the product scope explicitly changes.
