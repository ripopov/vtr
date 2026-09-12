#!/bin/sh
set -eu
cd "$(dirname "$0")"
cargo fmt --package volna -- --check
cargo clippy --locked -p volna --all-targets --all-features -- -D warnings
cargo test --locked -p volna --all-features

node --test vscode-ext/theme.test.mjs
