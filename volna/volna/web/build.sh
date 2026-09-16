#!/bin/sh
# Build the wasm bundle into web/dist and copy it into the VS Code extension.
set -eu
cd "$(dirname "$0")/.."
# zstd-sys needs a clang that targets wasm32; Apple's does not, Homebrew LLVM does.
if [ -z "${CC_wasm32_unknown_unknown:-}" ]; then
  for llvm in /opt/homebrew/opt/llvm /opt/homebrew/opt/llvm@20 /opt/homebrew/opt/llvm@19 /usr/lib/llvm-20 /usr/lib/llvm-19; do
    if [ -x "$llvm/bin/clang" ]; then
      export CC_wasm32_unknown_unknown="$llvm/bin/clang"
      export AR_wasm32_unknown_unknown="$llvm/bin/llvm-ar"
      break
    fi
  done
fi
# Query Cargo so workspace builds and CARGO_TARGET_DIR both work.
target_dir=$(cargo metadata --no-deps --format-version 1 | node -e '
  let input = "";
  process.stdin.on("data", chunk => input += chunk);
  process.stdin.on("end", () => console.log(JSON.parse(input).target_directory));
')
# Optional diagnostics are opt-in and absent from normal extension bundles.
cargo build --locked -p volna --profile viewer --lib --target wasm32-unknown-unknown \
  --features "${VOLNA_WEB_FEATURES:-}"
wasm-bindgen --target web --out-dir web/dist --out-name volna \
  "$target_dir/wasm32-unknown-unknown/viewer/volna.wasm"
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -Os -o web/dist/volna_bg.wasm web/dist/volna_bg.wasm
fi
mkdir -p vscode-ext/media
cargo build --locked -p volna-server --profile viewer
mkdir -p vscode-ext/bin
if [ -f "$target_dir/viewer/volna-server.exe" ]; then
  server_name=volna-server.exe
else
  server_name=volna-server
fi
# Replace the inode so rebuilding also works while a test editor owns a child.
server_tmp=$(mktemp vscode-ext/bin/.volna-server.XXXXXX)
trap 'rm -f "$server_tmp"' 0
cp "$target_dir/viewer/$server_name" "$server_tmp"
chmod 755 "$server_tmp"
mv -f "$server_tmp" "vscode-ext/bin/$server_name"
cp vscode-ext/theme.mjs vscode-ext/media/
cp web/dist/volna.js web/dist/volna_bg.wasm vscode-ext/media/
mkdir -p vscode-ext/media/licenses
cp ../volna-core/assets/fonts/IBMPlexSans-LICENSE.txt ../volna-core/assets/fonts/Lilex-OFL.txt vscode-ext/media/licenses/
cp ../volna-core/assets/icons/LICENSE vscode-ext/media/licenses/Lucide-LICENSE.txt
ls -la web/dist
