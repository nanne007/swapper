#!/bin/sh
# Shared local/CI gate. Live provider and production RPC tests remain opt-in.
set -eux
cd "$(dirname "$0")/.."

cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --release
forge fmt --root contracts --check
forge build --root contracts --deny-warnings
forge test --root contracts
cargo test --test e2e_local -- --ignored --nocapture
