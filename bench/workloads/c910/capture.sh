#!/bin/sh
# Capture the pulp-c910 (openC910) CoreMark VTR+VDB example pair.
#
# Builds whatever is missing (the VTR C library, the CoreMark image, the
# Verilated model with --trace-vtr), runs one CoreMark iteration with the
# whole design dumped, and writes c910_coremark.vtr / c910_coremark.vdb.json
# into --out (default volna/volna/examples, where the viewer examples live).
# The model embeds the VDB document and writes the companion beside the
# recording, so no explicit export step is needed.
#
#   capture.sh [--out DIR] [--iterations N] [--max-cycles N] [--check]
#
# Requires cargo, a riscv64-unknown-elf- toolchain, and the pinned Verilator
# fork with --trace-vtr built by integrations/verilator/build.sh to
# bench/build/verilator/install. The model is not rebuilt when the Verilator
# binary changes; remove the obj_vtr build dir (or `make -C
# bench/workloads/c910 clean BUILD=bench/workloads/gen/c910_build`) after an
# upgrade before recapturing.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
OUT=${OUT:-"$ROOT/volna/volna/examples"}
ITERATIONS=${ITERATIONS:-1}
MAX_CYCLES=${MAX_CYCLES:-}
CHECK=${CHECK:-0}
VERILATOR=${VERILATOR:-"$ROOT/bench/build/verilator/install/bin/verilator"}
VTR_INCLUDE="$ROOT/core/vtr-capi/include"
VTR_LIBDIR=${VTR_LIBDIR:-"$ROOT/target/release"}
BUILD=${BUILD:-"$ROOT/bench/workloads/gen/c910_build"}
JOBS=${JOBS:-$(getconf _NPROCESSORS_ONLN)}

while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    --iterations) ITERATIONS="$2"; shift 2 ;;
    --max-cycles) MAX_CYCLES="$2"; shift 2 ;;
    --check) CHECK=1; shift ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

if [ ! -x "$VERILATOR" ]; then
  echo "verilator --trace-vtr not found at $VERILATOR; build it with:" >&2
  echo "  integrations/verilator/build.sh bench/build/verilator/install" >&2
  exit 2
fi
if ! command -v riscv64-unknown-elf-gcc >/dev/null; then
  echo "riscv64-unknown-elf- gcc is required to build the CoreMark image" >&2
  exit 2
fi

mkdir -p "$OUT"

# VTR C library (static archive linked into the model) and the VDB CLI.
if [ ! -f "$VTR_LIBDIR/libvtr.a" ] || [ ! -f "$ROOT/target/release/vtr-vdb" ]; then
  echo "[capture] cargo build --release -p vtr-capi -p vtr-vdb"
  (cd "$ROOT" && cargo build --release -p vtr-capi -p vtr-vdb)
fi

echo "[capture] building the CoreMark image ($ITERATIONS iteration)"
make -C "$HERE" sw BUILD="$BUILD" ITERATIONS="$ITERATIONS"

echo "[capture] building the --trace-vtr C910 model"
make -C "$HERE" model MODE=vtr BUILD="$BUILD" JOBS="$JOBS" \
  VERILATOR="$VERILATOR" VTR_INCLUDE="$VTR_INCLUDE" VTR_LIBDIR="$VTR_LIBDIR"

# The testbench $readmemh's inst.pat/data.pat relative to the working dir.
TRACE="$OUT/c910_coremark.vtr"
MAX_ARG=
if [ -n "$MAX_CYCLES" ]; then MAX_ARG="--max-cycles=$MAX_CYCLES"; fi
set +e
(cd "$BUILD/sw/coremark" && "$BUILD/obj_vtr/Vtop" --dump="$TRACE" $MAX_ARG)
rc=$?
set -e
if [ "$rc" -ne 0 ]; then
  echo "note: simulation exited $rc (expected TIMEOUT when truncated by --max-cycles)"
fi

companion="$OUT/c910_coremark.vdb.json"
if [ ! -s "$companion" ]; then
  echo "VDB companion not written next to $TRACE; something went wrong" >&2
  exit 1
fi
echo "== written: $TRACE ($(ls -lh "$TRACE" | awk '{print $5}'))"
echo "== written: $companion ($(ls -lh "$companion" | awk '{print $5}'))"

if [ "$CHECK" -eq 1 ]; then
  echo "[capture] vtr-vdb check (exit 2 on unrecorded trace entries is expected)"
  "$ROOT/target/release/vtr-vdb" check "$companion" "$TRACE" || true
fi