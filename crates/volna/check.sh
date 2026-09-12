#!/bin/sh
set -eu
cd "$(dirname "$0")"
# The toolkit-free core, the GPUI frontend (this crate) and the egui frontend.
cargo fmt --package volna-core --package volna --package volna-egui -- --check
cargo clippy --locked -p volna-core -p volna -p volna-egui --all-targets --all-features -- -D warnings
cargo test --locked -p volna-core
cargo test --locked -p volna --all-features
cargo test --locked -p volna-egui

node --test vscode-ext/theme.test.mjs
