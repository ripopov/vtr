#!/bin/sh
# Build the pinned ext/verilator submodule with --trace-vtr support.
# Usage: integrations/verilator/build.sh <prefix> [build-dir]
# The default build directory is <prefix>/../build; sources stay in the submodule.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "usage: $0 <prefix> [build-dir]" >&2
  exit 2
fi
SRC=$HERE/../../ext/verilator
if [ ! -f "$SRC/configure.ac" ]; then
  echo "Initialize the pinned source with: git submodule update --init ext/verilator" >&2
  exit 1
fi
SRC=$(cd "$SRC" && pwd)
PREFIX=$(mkdir -p "$1" && cd "$1" && pwd)
BUILD_DIR=${2:-$PREFIX/../build}
mkdir -p "$BUILD_DIR"
BUILD_DIR=$(cd "$BUILD_DIR" && pwd)
(cd "$SRC" && autoconf)
cd "$BUILD_DIR"
# Host compiler for Verilator itself and, through verilated.mk, for every model it
# generates. clang is preferred: on the C910 model of the benchmark suite, gcc 15/16
# produce code that runs 3x slower than clang 19-22 from the same generated sources
# unless profile-guided optimisation is used (docs/BENCHMARKS.md, host compiler).
CXX=${CXX:-$(command -v clang++ || echo g++)}
export CXX
"$SRC/configure" --prefix="$PREFIX"
make -j"${JOBS:-$(getconf _NPROCESSORS_ONLN)}"
make install
echo "$PREFIX/bin/verilator"
