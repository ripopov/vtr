#!/bin/sh
set -eu
export VOLNA_WORKSPACE=off
cd "$(dirname "$0")"
# The toolkit-free core, the GPUI frontend (this crate) and the egui frontend.
cargo fmt --package vtr-server --package vtr-query --package volna-core --package volna --package volna-egui -- --check
cargo clippy --locked -p vtr-server -p vtr-query -p volna-core -p volna -p volna-egui --all-targets --all-features -- -D warnings
cargo test --locked -p vtr-query --all-features
cargo test --locked -p vtr-server
cargo test --locked -p volna-core
cargo test --locked -p volna --all-features
cargo test --locked -p volna-egui

cargo build --locked -p vtr-server
query_target_dir=$(cargo metadata --no-deps --format-version 1 | node -e '
  let input = "";
  process.stdin.on("data", chunk => input += chunk);
  process.stdin.on("end", () => console.log(JSON.parse(input).target_directory));
')
VTR_SERVER="$query_target_dir/debug/vtr-server" node --test \
  vscode-ext/theme.test.mjs vscode-ext/workspace.test.cjs \
  vscode-ext/relay.test.cjs vscode-ext/query-host.test.cjs vscode-ext/relay-server.test.cjs
