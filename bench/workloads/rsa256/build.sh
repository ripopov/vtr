#!/bin/sh
# Builds the RSA256 design (ext/libfstwriter) with the benchmark driver RSA_bench_tb.sv
# and the harness tb.cpp, traced to FST, VTR or not at all:
#
#   build.sh <outdir> <none|fst|vtr>      ->  <outdir>/obj_<mode>/rsa_tb
#
# Uses $VERILATOR (default: verilator on PATH; --trace-vtr needs the patched build
# from integrations/verilator). MODE=vtr needs VTR_INCLUDE and VTR_LIBDIR exported.
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
SRC=$HERE/../../../ext/libfstwriter/integration_test/tests/RSA256
OUT=${1:-$HERE/build}
MODE=${2:-fst}
case "$MODE" in
  none) TRACE="" ;;
  fst) TRACE="--trace-fst" ;;
  vtr) TRACE="--trace-vtr" ;;
  *) echo "unknown mode $MODE" >&2; exit 2 ;;
esac
mkdir -p "$OUT"
cd "$OUT"
${VERILATOR:-verilator} --cc "$HERE/RSA_bench_tb.sv" "$SRC/Pipeline.sv" "$SRC/PipelineCombine.sv" "$SRC/PipelineDistribute.sv" \
  "$SRC/PipelineFilter.sv" "$SRC/PipelineLoop.sv" "$SRC/Montgomery.sv" "$SRC/RSA.sv" "$SRC/RSAMont.sv" "$SRC/TwoPower.sv" \
  -Mdir "obj_$MODE" $TRACE -Wno-lint --timing --top-module Testbench --prefix VRSA_tb --build -j 8 --exe "$HERE/tb.cpp" -o rsa_tb > "verilator_$MODE.log" 2>&1
echo "$OUT/obj_$MODE/rsa_tb"
