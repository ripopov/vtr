#!/bin/sh
# Build the pinned ext/slang-server submodule (slang-server with Surfer's semantic
# tokens and per-instance generate-block queries) and install its binary.
# Usage: integrations/slang-server/build.sh <prefix> [build-dir]
# The default build directory is <prefix>/../build; sources stay in the submodule.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "usage: $0 <prefix> [build-dir]" >&2
  exit 2
fi
SRC=$HERE/../../ext/slang-server
if [ ! -f "$SRC/CMakeLists.txt" ] || [ ! -f "$SRC/external/slang/CMakeLists.txt" ]; then
  echo "Initialize the pinned source with: git submodule update --init --recursive ext/slang-server" >&2
  exit 1
fi
SRC=$(cd "$SRC" && pwd)
PREFIX=$(mkdir -p "$1" && cd "$1" && pwd)
BUILD_DIR=${2:-$PREFIX/../build}
mkdir -p "$BUILD_DIR"
BUILD_DIR=$(cd "$BUILD_DIR" && pwd)
# slang requires a C++20 compiler; clang is preferred when present.
CXX=${CXX:-$(command -v clang++ || echo g++)}
CC=${CC:-$(command -v clang || echo gcc)}
export CXX CC
cmake -S "$SRC" -B "$BUILD_DIR" -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_CXX_COMPILER="$CXX" -DCMAKE_C_COMPILER="$CC" > "$BUILD_DIR/configure.log" 2>&1 \
  || { cat "$BUILD_DIR/configure.log" >&2; exit 1; }
cmake --build "$BUILD_DIR" -j"${JOBS:-$(getconf _NPROCESSORS_ONLN)}" --target slang_server
mkdir -p "$PREFIX/bin"
cp "$BUILD_DIR/bin/slang-server" "$PREFIX/bin/slang-server"
echo "$PREFIX/bin/slang-server"
