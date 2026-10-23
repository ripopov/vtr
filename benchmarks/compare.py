#!/usr/bin/env python3
"""Compare two native VTR benchmark binaries with interleaved, verified runs."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import subprocess
import time

from run import HERE, PROFILES, metric_values


def render(data):
    lines = ["# Native VTR paired comparison", "",
             f"Status: **{data['status']}**. Measured {data['started_utc']}. "
             f"Elapsed: {data['elapsed_seconds']:.2f} s.", "",
             f"Host: {data['host']}. CPU affinity: {data['cpu']}. "
             f"Paired repetitions: {data['repeats']}. Full default workload sizes.", "",
             "Each pair runs both binaries in shuffled order on one CPU. Each binary writes its own trace, "
             "then runs the full read and all twelve probes of every applicable query. Every result is "
             "validated against both binaries' independent producer replays. "
             "Timings include complete writes, full reads, and fresh-reader queries; lower time and RSS are better. "
             "Changes below are candidate / baseline − 1. Small differences and process RSS variation "
             "are not evidence of a speedup or regression.", "",
             "Both executables use their own source replay and files. Preserve build provenance and "
             "compare every metric, including slower results. See the benchmark guide for "
             "the shared workload and timing boundaries.", ""]
    for label, binary in data["binaries"].items():
        lines += [f"{label.title()}: {binary['label']}. Binary SHA-256: `{binary['sha256']}`.", ""]
    for profile, oracle in data["oracles"].items():
        columns = [metric_values([r for r in data["measurements"]
                                  if r["profile"] == profile and r["variant"] == variant], oracle["all"])
                   for variant in ("baseline", "candidate")]
        lines += [f"## {profile}", "", "| Metric | Baseline | Candidate | Change |",
                  "|---|---:|---:|---:|"]
        for metric in dict.fromkeys(key for column in columns for key in column):
            before, after = [column.get(metric, "—") for column in columns]
            delta = "—"
            try:
                a, b = [float(x.replace(",", "")) for x in (before, after)]
                delta = f"{(b / a - 1) * 100:+.1f}%" if a else "—"
            except ValueError:
                pass
            lines.append(f"| {metric} | {before} | {after} | {delta} |")
        lines.append("")
    if "error" in data:
        lines += ["## Incomplete run", "", data["error"], ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("--candidate", type=Path, default=HERE / ".build" / "benchmark")
    parser.add_argument("--baseline-label", default="baseline")
    parser.add_argument("--candidate-label", default="candidate")
    parser.add_argument("--repeats", type=int, default=7)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error("repeats must be positive")
    cpu = min(os.sched_getaffinity(0)) if args.cpu is None else args.cpu
    os.sched_setaffinity(0, {cpu})
    out = args.output or HERE / "results" / ("paired-" + datetime.now().strftime("%Y%m%d-%H%M%S-%f"))
    out.mkdir(parents=True, exist_ok=False)
    paths = {"baseline": args.baseline.resolve(), "candidate": args.candidate.resolve()}
    data = {"status": "running", "started_utc": datetime.now(timezone.utc).isoformat(),
            "host": platform.platform(), "cpu": cpu, "repeats": args.repeats,
            "seed": 20261019, "binaries": {}, "oracles": {}, "measurements": []}
    for variant, path in paths.items():
        data["binaries"][variant] = {"path": str(path), "label": getattr(args, variant + "_label"),
                                     "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
    start = time.monotonic()
    deadline = start + 280

    def invoke(variant, operation, profile, path):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("paired comparison exceeded 280 seconds")
        result = subprocess.run([str(paths[variant]), operation, "vtr", profile, str(path), "1"],
                                capture_output=True, text=True, timeout=remaining, check=True)
        return json.loads(result.stdout)

    rng = random.Random(data["seed"])
    exit_code = 0
    try:
        for profile in PROFILES:
            trace = out / "trace"
            oracle = invoke("baseline", "oracle", profile, trace)
            if oracle != invoke("candidate", "oracle", profile, trace):
                raise RuntimeError(f"producer replays differ: {profile}")
            data["oracles"][profile] = oracle
            operations = ["write", "read"]
            if profile not in ("requests", "many-generators"):
                operations += ["search-signal", "find-signal", "point"]
            if profile in ("requests", "many-generators", "mixed"):
                operations += ["search-transaction", "find-transaction"]
            for repeat in range(args.repeats):
                variants = list(paths)
                rng.shuffle(variants)
                for variant in variants:
                    for operation in operations:
                        row = invoke(variant, operation, profile, trace)
                        if operation != "write":
                            expected = [oracle["all"]] if operation == "read" else oracle[operation]
                            if row["digests"] != expected:
                                raise RuntimeError(f"validation failed: {variant}/{profile}/{operation}")
                        row.update(profile=profile, variant=variant, operation=operation,
                                   repeat=repeat, file_bytes=trace.stat().st_size)
                        data["measurements"].append(row)
                    trace.unlink()
            print(f"verified {profile}: {args.repeats} pairs", flush=True)
        data["status"] = "complete; all checks passed"
    except (RuntimeError, TimeoutError, subprocess.SubprocessError, OSError, ValueError, KeyboardInterrupt) as error:
        data["status"] = "INCOMPLETE"
        data["error"] = str(error) or type(error).__name__
        exit_code = 1
    finally:
        data["elapsed_seconds"] = time.monotonic() - start
        (out / "raw.json").write_text(json.dumps(data, indent=2) + "\n")
        (out / "report.md").write_text(render(data))
        print(f"Report: {out / 'report.md'}")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
