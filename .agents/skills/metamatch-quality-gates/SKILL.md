---
name: metamatch-quality-gates
description: 'Run or modify the MetaMatch Cargo, Clippy, test, release, Solidity, E2E, and GitHub Actions quality gates. Use when preparing a handoff, changing Cargo or CI, investigating a gate failure, or reviewing whether a change is ready to merge.'
---

# MetaMatch quality gates

Read `AGENTS.md`, `Cargo.toml`, `rust-toolchain.toml`, `contracts/foundry.toml`, and `.github/workflows/ci.yml` first. Keep local fixtures, the Anvil E2E, and any future live integration clearly separated.

## Local sequence

Use the smallest relevant command while iterating, then run the complete sequence:

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

`forge fmt` is the only Solidity formatter. Do not weaken Clippy, compiler warnings, assertions, or security checks to make a gate green.

## Review failures

- Format failure: run `cargo fmt --all` or the relevant Foundry formatter and inspect the diff. Keep generated `target` and Foundry output out of the source tree.
- Clippy/compiler failure: fix the Rust source or test contract; do not add broad lint suppression.
- Solidity warning: fix the expression or add a narrow explanation at the exact line when a conversion is proven safe. Keep `--deny-warnings` enabled.
- Test failure: reproduce the narrowest test, identify the product or environment cause, and add a regression assertion before broadening the change.
- E2E failure: distinguish missing Anvil/tooling, local contract behavior, and unverified production Holder/Router/provider behavior.
- CI-only failure: compare the pinned Rust/Foundry toolchains and Ubuntu behavior; a local macOS pass is not sufficient evidence.

When a gate depends on unavailable live API keys, RPC capabilities, Docker, or a deployed Router, mark it unverified. Never substitute fabricated live evidence.
