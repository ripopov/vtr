#!/usr/bin/env python3
"""Run verified synthetic comparisons and write Markdown tables plus raw JSON."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import random
import statistics
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
PROFILES = {
    "core": ("vtr", "fst", "vcd"),
    "soc": ("vtr", "fst", "vcd"),
    "idle": ("vtr", "fst", "vcd"),
    "requests": ("vtr", "ftr"),
    "many-generators": ("vtr", "ftr"),
    "mixed": ("vtr", "fst+ftr", "vcd+ftr"),
}


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def metric_values(group, expected):
    metrics = {}
    writes = [r for r in group if r["operation"] == "write"]
    reads = [r for r in group if r["operation"] == "read"]
    count = expected["count"]
    if writes:
        def med(key):
            return statistics.median(r[key] for r in writes)

        size = med("file_bytes")
        metrics.update({
            "Write time (ms)": f"{med('seconds') * 1e3:.2f}",
            "Write throughput (Mobs/s)": f"{count / med('seconds') / 1e6:.3f}",
            "File size (MiB)": f"{size / 2**20:.3f}",
            "Bytes per observation": f"{size / count:.2f}",
            "Logical bytes / file bytes": f"{expected['logical_bytes'] / size:.2f}",
            "Write peak RSS (MiB)": f"{max(r['peak_rss_kib'] for r in writes) / 1024:.2f}",
            "Capture time (ms)": f"{med('capture_seconds') * 1e3:.2f}",
            "Capture time (ns/tick)": f"{med('capture_seconds') / writes[0]['ticks'] * 1e9:.1f}",
            "File bytes per tick": f"{size / writes[0]['ticks']:.1f}",
            "Close time (ms)": f"{med('close_seconds') * 1e3:.2f}",
            "Write CPU time (ms)": f"{med('cpu_seconds') * 1e3:.2f}",
            "Write min / max (ms)": f"{min(r['seconds'] for r in writes) * 1e3:.2f} / "
                                    f"{max(r['seconds'] for r in writes) * 1e3:.2f}",
        })
    if reads:
        elapsed = statistics.median(r["seconds"] for r in reads)
        metrics.update({
            "Read time (ms)": f"{elapsed * 1e3:.2f}",
            "Read throughput (Mobs/s)": f"{count / elapsed / 1e6:.3f}",
            "Read peak RSS (MiB)": f"{max(r['peak_rss_kib'] for r in reads) / 1024:.2f}",
        })
    for operation in ("search-signal", "find-signal", "point", "search-transaction", "find-transaction"):
        selected = [r for r in group if r["operation"] == operation]
        if not selected:
            continue
        latencies = [t for r in selected for t in r["latencies_seconds"]]
        metrics.update({
            f"{operation}: probes": f"{len(latencies):,}",
            f"{operation}: matches": f"{sum(d['count'] for r in selected for d in r['digests']):,}",
            f"{operation}: p50 (ms)": f"{statistics.median(latencies) * 1e3:.3f}",
            f"{operation}: p95 (ms)": f"{percentile(latencies, .95) * 1e3:.3f}",
            f"{operation}: peak RSS (MiB)": f"{max(r['peak_rss_kib'] for r in selected) / 1024:.2f}",
        })
    return metrics


def report(data):
    lines = ["# VTR synthetic format comparison", "", f"Status: **{data['status']}**. "
             f"Elapsed suite time: **{data['elapsed_seconds']:.2f} s** "
             f"(budget {data['budget_seconds']:.0f} s; dependency setup/build excluded).", "",
             f"Host: {data['host']}. CPU affinity: {data['cpu']}. Repeats: {data['repeats']}. "
             f"Scale divisor: {data['scale_divisor']}. Format-order seed: {data['seed']}.", "",
             "All backends share one logical CPU, synchronous producers, and LZ4 for binary formats. "
             "Writes include generation, capture, encoding, compression, metadata/indexing, final drain, and close. "
             "Buffered I/O is used; fsync and storage durability are not measured. "
             "Reads decode every observation and checksum all values/attributes. "
             "Read/query results are checked against an independently replayed producer. "
             "Fresh processes isolate peak RSS; OS page cache is warm after writing. "
             "Every query opens a fresh reader, including metadata/index preparation and close in its latency.", "",
             "Mobs/s means millions of signal changes or transactions per second; mixed workloads count both. "
             "Times and rates are medians of complete repetitions. RSS is the maximum per operation across repetitions. "
             "File size includes metadata and indexes (both files for paired formats). "
             "Logical bytes count fixed binary observation fields, not in-memory object sizes; "
             "logical/file is a defined normalization, not a claim about compression of identical input bytes. "
             "A dash indicates an unavailable measurement in an incomplete run.", "",
             "Capture is the entire producer loop including synchronous tracer work and any stalls; "
             "it is not an asynchronous enqueue-only measurement. Close includes final encoding and flushing. "
             "The null producer follows the same generation and lifetime schedule with a counting sink. "
             "CPU time includes process startup; wall timings start inside the executable.", "",
             "Twelve deterministic probes per operation and repetition include hits, guaranteed absent values, "
             "different sources, and bounded or whole-trace searches. Signal search returns matching transitions in an inclusive window. "
             "Transaction search returns all overlapping transactions for a generator whose address attribute equals the target. "
             "The find-signal and find-transaction operations apply the same predicates across the entire trace. "
             "Point queries return the value already in effect, including during idle windows. "
             "p95 is the nearest-rank percentile over all probes, not a confidence interval. "
             "All individual timings, counts, checksums, and build provenance are in raw.json.", ""]
    groups = {}
    for row in data["measurements"]:
        groups.setdefault((row["profile"], row["format"]), []).append(row)
    sections = (
        ("Waveform results", ("core", "soc", "idle")),
        ("Transaction results", ("requests", "many-generators")),
        ("Mixed signal and transaction results", ("mixed",)),
    )
    primary_metrics = (
        "Write time (ms)", "Write throughput (Mobs/s)", "Read time (ms)", "Read throughput (Mobs/s)",
        "File size (MiB)", "Bytes per observation", "Logical bytes / file bytes",
        "Write peak RSS (MiB)", "Read peak RSS (MiB)",
    )
    for heading, profiles in sections:
        available = [p for p in profiles if any((p, fmt) in groups for fmt in PROFILES[p])]
        if not available:
            continue
        lines += [f"## {heading}", ""]
        for profile in available:
            formats = PROFILES[profile]
            expected = data["oracles"][profile]["all"]
            columns = [metric_values(groups.get((profile, fmt), []), expected) for fmt in formats]
            labels = dict.fromkeys(primary_metrics)
            for column in columns:
                labels.update(dict.fromkeys(column))
            baselines = [r["seconds"] for r in data["baselines"] if r["profile"] == profile]
            summary = f"Observations: {expected['count']:,}."
            if baselines:
                summary += f" Null producer: {statistics.median(baselines) * 1e3:.2f} ms."
            lines += [f"### {profile}", "", summary, "",
                      "| Metric | " + " | ".join(fmt.upper() for fmt in formats) + " |",
                      "|---|" + "---:|" * len(formats)]
            for label in labels:
                if any(label in column for column in columns):
                    lines.append("| " + label + " | " + " | ".join(column.get(label, "—") for column in columns) + " |")
            lines.append("")

    lines += ["## Interpretation", "",
              "FST uses upstream libfst. FTR uses LWTR4SC's standalone upstream writer, bypassing SystemC. "
              "VCD uses the included buffered C++ writer and streaming subset reader. "
              "FTR reads use the included CBOR/LZ4 subset reader, with stream/time chunk pruning. "
              "These read timings compare the named implementations, not inherent format limits. "
              "VTR and FST use native signal/time selection; VCD scans from the beginning. "
              "FTR and VTR filter attribute values after selecting candidate transactions; no secondary value index is built. "
              "The FTR header scan/index is charged on every open. "
              "No result asserts asynchronous scaling, four-state/wide-vector performance, recovery, "
              "or rich owner/stage/relation navigation; those are outside these common-format workloads.", ""]
    if data.get("error"):
        lines += ["## Incomplete run", "", data["error"], ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="new output directory (must not already exist)")
    parser.add_argument("--report", type=Path, default=HERE / "report.md",
                        help="report destination for successful runs (default: benchmarks/report.md)")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--budget-seconds", type=float, default=280)
    parser.add_argument("--scale-divisor", type=int, default=1, help="divide default trace duration; 100 is a quick smoke run")
    parser.add_argument("--profiles", nargs="+", choices=PROFILES, default=list(PROFILES))
    parser.add_argument("--cpu", type=int, help="logical CPU (default: first CPU in current affinity)")
    parser.add_argument("--keep-traces", action="store_true")
    args = parser.parse_args()
    if args.repeats < 1 or not 1 <= args.scale_divisor <= 10000 or not 0 < args.budget_seconds <= 290:
        parser.error("repeats must be positive, scale divisor 1..10000, and budget in (0, 290]")
    binary = HERE / ".build" / "benchmark"
    build_metadata = HERE / ".build" / "build.json"
    if not binary.exists() or not build_metadata.exists():
        parser.error("run python3 build.py first")
    if not hasattr(os, "sched_setaffinity"):
        parser.error("this suite requires Linux for CPU affinity and RSS units")
    allowed = os.sched_getaffinity(0)
    cpu = min(allowed) if args.cpu is None else args.cpu
    if cpu not in allowed:
        parser.error(f"CPU {cpu} is outside current affinity")
    os.sched_setaffinity(0, {cpu})
    out = args.output or HERE / "results" / datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    out = out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    trace_dir = out / "traces"
    trace_dir.mkdir()
    start = time.monotonic()
    deadline = start + args.budget_seconds
    seed = 20261018
    data = {"status": "running", "started_utc": datetime.now(timezone.utc).isoformat(),
            "host": platform.platform(), "cpu": cpu, "repeats": args.repeats,
            "budget_seconds": args.budget_seconds, "scale_divisor": args.scale_divisor,
            "seed": seed, "build": json.loads(build_metadata.read_text()),
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "oracles": {}, "measurements": [], "baselines": [], "elapsed_seconds": 0}

    def invoke(op, fmt, profile, path):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("suite deadline exceeded")
        result = subprocess.run([str(binary), op, fmt, profile, str(path), str(args.scale_divisor)],
                                capture_output=True, text=True, timeout=remaining)
        if result.returncode:
            raise RuntimeError(f"{profile}/{fmt}/{op}: {result.stderr.strip()}")
        return json.loads(result.stdout)

    rng = random.Random(seed)
    exit_code = 0
    try:
        for profile in args.profiles:
            path = trace_dir / profile
            oracle = invoke("oracle", "null", profile, path)
            data["oracles"][profile] = oracle
            operations = ["write", "read"]
            if profile not in ("requests", "many-generators"):
                operations += ["search-signal", "find-signal", "point"]
            if profile in ("requests", "many-generators", "mixed"):
                operations += ["search-transaction", "find-transaction"]
            for repeat in range(args.repeats):
                baseline = invoke("write", "null", profile, path)
                if baseline["baseline_count"] != oracle["all"]["count"]:
                    raise RuntimeError(f"null producer count mismatch: {profile}")
                data["baselines"].append({**baseline, "profile": profile, "repeat": repeat})
                formats = list(PROFILES[profile])
                rng.shuffle(formats)
                for fmt in formats:
                    path = trace_dir / f"{profile}-{fmt}-{repeat}"
                    for op in operations:
                        row = invoke(op, fmt, profile, path)
                        if op != "write":
                            expected = [oracle["all"]] if op == "read" else oracle[op]
                            if row["digests"] != expected:
                                raise RuntimeError(f"round-trip/query mismatch: {profile}/{fmt}/{op}\n"
                                                   f"expected {expected}\nactual {row['digests']}")
                        files = [Path(str(path) + suffix) for suffix in (".wave", ".tx")] if "+" in fmt else [path]
                        row.update(profile=profile, format=fmt, operation=op, repeat=repeat,
                                   file_bytes=sum(f.stat().st_size for f in files))
                        data["measurements"].append(row)
                    if not args.keep_traces:
                        for file in files:
                            file.unlink()
                    print(f"verified {profile:16} {fmt:8} repeat {repeat + 1}/{args.repeats}", flush=True)
        data["status"] = "complete; all checks passed"
    except (RuntimeError, TimeoutError, subprocess.TimeoutExpired, OSError, ValueError, KeyboardInterrupt) as error:
        data["status"] = "INCOMPLETE"
        data["error"] = str(error) or type(error).__name__
        exit_code = 1
        print(data["error"], file=sys.stderr)
    finally:
        data["elapsed_seconds"] = time.monotonic() - start
        (out / "raw.json").write_text(json.dumps(data, indent=2) + "\n")
        rendered = report(data)
        (out / "report.md").write_text(rendered)
        if exit_code == 0:
            destination = args.report.resolve()
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(rendered)
            print(f"Report: {destination}")
        else:
            print(f"Incomplete report: {out / 'report.md'}")
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
