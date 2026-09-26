#!/usr/bin/env python3
"""Cost gates of the C910 pipeline tracer (docs/c910-verilator-tx-stream.html).

Runs one CoreMark iteration with each variant, interleaved, best of --runs, and
checks the proposal's gates, which are ratios and hold on any host:

  * pipeline only (obj_vtr_pipeline_only: no signal traced, --no-signals) within
    1.10x of the untraced model (obj_none);
  * full dump plus pipeline (obj_vtr_pipeline) within 1.03x of the full dump
    alone (obj_vtr).

Also reports file size and the read time of the pipeline stream (`vtr tx`).
Build the four models first (make model MODE=none / MODE=vtr / MODE=vtr
PIPELINE=1 / MODE=vtr PIPELINE=1 SIGNALS=0). Exits nonzero when a gate fails.

    pipeline_cost.py [--build DIR] [--runs N] [--out DIR] [--json FILE]

The results go to --json (default bench/results/latest/c910_pipeline.json),
from which `bench/run.py report` renders a section of docs/BENCHMARK_RESULTS.md.
"""
import argparse
import datetime
import json
import os
import pathlib
import platform
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[3]
STREAM = "TX.core0.pipeline"


def run(model, cwd, dump, *extra):
    args = [str(model)] + ([f"--dump={dump}"] if dump else []) + list(extra)
    out = subprocess.run(args, cwd=cwd, capture_output=True, text=True, check=False)
    line = [l for l in out.stdout.splitlines() if l.startswith("{\"result\"")][-1]
    result = json.loads(line)
    if result["result"] != "PASS":
        sys.exit(f"{model}: {result}")
    return result


def count_transactions(vtr, trace):
    out = subprocess.check_output([str(vtr), "tx", str(trace), "--stream", STREAM, "--max", "0"], text=True)
    last = out.strip().splitlines()[-1] if out.strip() else ""
    return int(last.split()[1]) if last.startswith("...") else 0


def machine():
    """The host, as the report states it."""
    cpu = platform.processor()
    if sys.platform == "darwin":
        cpu = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip() or cpu
    elif os.path.exists("/proc/cpuinfo"):
        for line in open("/proc/cpuinfo"):
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    return {"cpu": cpu, "os": f"{platform.system()} {platform.release()}", "arch": platform.machine()}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--build", type=pathlib.Path, default=ROOT / "bench/workloads/gen/c910_build")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--out", type=pathlib.Path, default=ROOT / "bench/build/c910-pipeline-cost")
    ap.add_argument("--json", type=pathlib.Path, default=ROOT / "bench/results/latest/c910_pipeline.json")
    args = ap.parse_args()
    b = args.build.resolve()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    cwd = b / "sw/coremark"
    variants = {
        "untraced": (b / "obj_none/Vtop", None, ()),
        "pipeline_only": (b / "obj_vtr_pipeline_only/Vtop", out / "pipeline_only.vtr", ("--no-signals",)),
        "full_dump": (b / "obj_vtr/Vtop", out / "full_dump.vtr", ()),
        "full_dump_pipeline": (b / "obj_vtr_pipeline/Vtop", out / "full_dump_pipeline.vtr", ()),
    }
    for name, (model, _, _) in variants.items():
        if not model.exists():
            sys.exit(f"{name}: {model} not built")
    best = {}
    for i in range(args.runs):
        for name, (model, dump, extra) in variants.items():
            if dump:
                dump.unlink(missing_ok=True)
            r = run(model, cwd, dump, *extra)
            print(f"run {i + 1} {name}: wall {r['wall_s']:.2f} s, cpu {r['cpu_s']:.2f} s, {r['bytes']} bytes", flush=True)
            if name not in best or r["wall_s"] < best[name]["wall_s"]:
                best[name] = r
    vtr = ROOT / "target/release/vtr"
    reads = {}
    for name in ("pipeline_only", "full_dump_pipeline"):
        t0 = time.perf_counter()
        subprocess.run([str(vtr), "tx", str(variants[name][1]), "--stream", STREAM, "--max", "0"],
                       stdout=subprocess.DEVNULL, check=True)
        reads[name] = time.perf_counter() - t0
    ratio_only = best["pipeline_only"]["wall_s"] / best["untraced"]["wall_s"]
    ratio_full = best["full_dump_pipeline"]["wall_s"] / best["full_dump"]["wall_s"]
    report = {
        "best_of": args.runs,
        "wall_s": {k: v["wall_s"] for k, v in best.items()},
        "cpu_s": {k: v["cpu_s"] for k, v in best.items()},
        "bytes": {k: v["bytes"] for k, v in best.items() if v["bytes"]},
        "pipeline_bytes_in_full_dump": best["full_dump_pipeline"]["bytes"] - best["full_dump"]["bytes"],
        "read_pipeline_s": reads,
        "gates": {"pipeline_only_vs_untraced": round(ratio_only, 4), "limit_1": 1.10,
                  "full_dump_pipeline_vs_full_dump": round(ratio_full, 4), "limit_2": 1.03},
        "cycles": best["untraced"]["cycles"],
        "pipeline_transactions": count_transactions(vtr, variants["pipeline_only"][1]),
        "machine": machine(),
        "date": datetime.datetime.now().isoformat(timespec="seconds"),
    }
    print(json.dumps(report, indent=1))
    args.json.parent.mkdir(parents=True, exist_ok=True)
    args.json.write_text(json.dumps(report, indent=1) + "\n")
    return 0 if ratio_only <= 1.10 and ratio_full <= 1.03 else 1


if __name__ == "__main__":
    sys.exit(main())
