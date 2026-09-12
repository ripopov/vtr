#!/bin/sh
set -eu
cd "$(dirname "$0")"
# The toolkit-free core and the GPUI frontend (this crate).
cargo fmt --package volna-core --package volna -- --check
cargo clippy --locked -p volna-core -p volna --all-targets --all-features -- -D warnings
cargo test --locked -p volna-core
cargo test --locked -p volna --all-features

node --test vscode-ext/theme.test.mjs
