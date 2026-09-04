#!/bin/sh
# Builds Verilator with the --trace-vtr backend (trace-vtr.patch) from a pinned
# upstream tag and installs it into a prefix.
#
#   integrations/verilator/build.sh <prefix> [src-dir]
#
# <prefix>/bin/verilator is the result. The source is cloned (shallow) from
# GitHub into <src-dir> (default <prefix>/../src) unless that directory already
# exists; set VERILATOR_TAG to build another version (the patch targets v5.050).
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
PREFIX=$(mkdir -p "$1" && cd "$1" && pwd)
SRC=${2:-$PREFIX/../src}
TAG=${VERILATOR_TAG:-v5.050}
if [ ! -d "$SRC" ]; then
  git clone -q --depth 1 --branch "$TAG" https://github.com/verilator/verilator "$SRC"
fi
cd "$SRC"
if ! grep -q "trace-vtr" src/V3Options.cpp; then
  git apply "$HERE/trace-vtr.patch"
fi
autoconf
# Host compiler for Verilator itself and, through verilated.mk, for every model it
# generates. clang is preferred: on the C910 model of the benchmark suite, gcc 15/16
# produce code that runs 3x slower than clang 19-22 from the same generated sources
# unless profile-guided optimisation is used (docs/BENCHMARKS.md, host compiler).
CXX=${CXX:-$(command -v clang++ || echo g++)}
export CXX
./configure --prefix="$PREFIX"
make -j"$(nproc)"
make install
echo "$PREFIX/bin/verilator"
