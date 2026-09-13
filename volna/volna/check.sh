#!/bin/sh
set -eu
export VOLNA_WORKSPACE=off
cd "$(dirname "$0")"
# The toolkit-free core, the GPUI frontend (this crate) and the egui frontend.
cargo fmt --package vtr-query --package volna-core --package volna --package volna-egui -- --check
cargo clippy --locked -p vtr-query -p volna-core -p volna -p volna-egui --all-targets --all-features -- -D warnings
cargo test --locked -p vtr-query --all-features
cargo test --locked -p volna-core
cargo test --locked -p volna --all-features
cargo test --locked -p volna-egui

node --test vscode-ext/theme.test.mjs vscode-ext/workspace.test.cjs
