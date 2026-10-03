#!/bin/sh
set -eu
export VOLNA_WORKSPACE=off
cd "$(dirname "$0")"
uv run --script tools/check-desktop.py
python3 ../../tools/check-volna-loading.py
# The toolkit-free core, the GPUI frontend (this crate) and the egui frontend.
cargo fmt --package volna-trace --package volna-core --package volna --package volna-egui --package volna-server -- --check
cargo clippy --locked -p volna-trace -p volna-core -p volna -p volna-egui -p volna-server --all-targets --all-features -- -D warnings
cargo test --locked -p volna-core -p volna-server --all-targets
cargo test --locked -p volna --all-features --lib --bins
cargo test --locked -p volna --all-features --doc
# Metal offscreen rendering is covered on macOS, explicitly in hierarchy.yml.
if [ "$(uname -s)" = Darwin ]; then
  cargo test --locked -p volna --all-features --test viewer
fi
cargo test --locked -p volna-egui

node --test vscode-ext/theme.test.mjs vscode-ext/workspace.test.cjs vscode-ext/trace-host.test.cjs vscode-ext/commands.test.cjs
node --test tools/table-clipboard.test.mjs
