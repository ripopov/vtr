#!/bin/sh
# Builds the RSA256 design (ext/libfstwriter) with the benchmark driver RSA_bench_tb.sv and
# FST tracing. Requires verilator >= 5.
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
SRC=$HERE/../../../ext/libfstwriter/integration_test/tests/RSA256
OUT=${1:-$HERE/build}
mkdir -p "$OUT"
cd "$OUT"
verilator --cc "$HERE/RSA_bench_tb.sv" "$SRC/Pipeline.sv" "$SRC/PipelineCombine.sv" "$SRC/PipelineDistribute.sv" \
  "$SRC/PipelineFilter.sv" "$SRC/PipelineLoop.sv" "$SRC/Montgomery.sv" "$SRC/RSA.sv" "$SRC/RSAMont.sv" "$SRC/TwoPower.sv" \
  -Mdir verilated --trace-fst -Wno-lint --timing --top-module Testbench --prefix VRSA_tb --build -j 8 --exe "$HERE/tb.cpp" -o rsa_tb > verilator.log 2>&1
echo "$OUT/verilated/rsa_tb"
